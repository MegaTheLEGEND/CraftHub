# CraftHub

A self-hosted "Creative Cloud"-style launcher for the open-source
[storytold](https://github.com/storytold) Crafting Apps (PhotoCraft, VectorCraft, FilmCraft, LightCraft, …).
One Rust binary in one small Docker image that:

- **Watches GitHub releases** of each app and **downloads the web build**
  (`<app>-web-<version>.zip`), verifies it against the release's `SHA256SUMS.txt`, and unpacks it atomically.
- **Hosts** the installed web build at `/apps/<app>/` (brotli/gzip pre-compressed, correct wasm MIME, sane caching).
- **Manages versions**: auto-update, pin/roll back, keep the last N, delete old ones, per-app pre-release toggle.
- **Acts as a launcher**: app grid with Launch buttons, plus links to the desktop installers (Windows, macOS, Linux)
  for each release. Each card says which commercial app it replaces (e.g. PhotoCraft: *Replaces Photoshop*).
- **Sits behind SSO**: OpenID Connect (authorization code + PKCE) built for Authentik; exercised against a mock provider, not yet a live Authentik. Group-based
  access (`ALLOWED_GROUPS`) and admin (`ADMIN_GROUPS`) roles.

## Quick start

```sh
cp .env.example .env     # fill in OIDC_CLIENT_ID / OIDC_CLIENT_SECRET / SESSION_SECRET (openssl rand -hex 32)
# edit PUBLIC_URL and OIDC_ISSUER in docker-compose.yml
docker pull megathelegend/crafthub:latest
# then start with compose (no build needed)
docker compose up -d
```

For a first look without SSO: `AUTH_MODE=none PUBLIC_URL=http://localhost:8080` (everyone is admin; local use only).

## Setting up SSO with Authentik

CraftHub starts with authentication **off** (a banner says so). You set up SSO in two parts: create the app in
Authentik, then paste its details into CraftHub and turn it on. Examples below use `https://craft.example.com`
for CraftHub and `https://auth.example.com` for Authentik; substitute your own.

### Part 1: in Authentik (admin interface)

1. **Create the app and provider.** Go to **Applications → Applications → Create with Provider**.
   - *Name*: `CraftHub`. *Slug*: `crafthub` (the slug becomes part of the issuer URL, so keep it simple).
   - *Provider type*: **OAuth2/OpenID Provider**.
2. **Configure the provider.**
   - *Authorization flow*: the default (`default-provider-authorization-implicit-consent` skips the consent screen).
   - *Client type*: **Confidential**.
   - *Redirect URIs/Origins*: add **`https://craft.example.com/auth/callback`** and set the matching mode to **Strict**.
     This must match what you type as the Public URL in CraftHub, character for character (https vs http, no trailing slash).
   - *Signing Key*: select **authentik Self-signed Certificate** (RS256). If you leave it empty, CraftHub still works (HS256).
   - *Scopes*: leave the defaults (`email`, `openid`, `profile`). The `profile` scope carries the `groups` claim
     used for roles.
3. **Copy three values** from the provider's page once it is saved: the **Client ID**, the **Client Secret**
   (click the eye icon) and the **OpenID Configuration Issuer**. The issuer looks like
   `https://auth.example.com/application/o/crafthub/` and **the trailing slash matters**.
4. **Create groups and add yourself** (optional but recommended). Under **Directory → Groups**, create
   `crafthub-admins` (may install, update, roll back and remove versions) and, if you want to restrict who can
   sign in at all, `crafthub-users`. Add your user to `crafthub-admins`.
5. **Control who can open the app** (optional). On the application, open **Policy / Group / User Bindings** and bind
   the groups that may use it. With no bindings, every Authentik user may sign in to CraftHub.

### Part 2: in CraftHub

1. Open CraftHub and click **Settings → Authentication**.
2. Fill in: **Public URL** (`https://craft.example.com`, prefilled from your browser), **Issuer**, **Client ID**,
   **Client secret**, and the group names from step 4 (leave *Admin groups* empty and every user becomes an admin;
   leave *Allowed groups* empty and any authenticated user may sign in). Leave scopes and groups claim at their defaults.
3. Press **1. Save**. The issuer is checked immediately; a typo is reported right away.
4. Press **2. Test sign-in**. A new tab opens, signs you in through Authentik, and must report
   **"Test sign-in worked"** as an administrator. If the account would not be an admin, it tells you why (usually the
   admin group name or the groups claim) and refuses to proceed.
5. Back in Settings, press **Refresh status**, then **3. Enable SSO**. You are sent to Authentik to sign in.

The test requirement exists so a wrong setting can't lock you out. **Turn authentication off** is in the same place.
If you do get locked out anyway, start the container once with `AUTH_RECOVERY=true` (ignores saved auth and runs
open), fix the settings, then remove the variable.

You can instead set `OIDC_ISSUER`, `OIDC_CLIENT_ID` and `OIDC_CLIENT_SECRET` as environment variables to enable SSO at
boot; anything saved in the Settings page takes precedence over them.

Alternative: Authentik's **proxy outpost** in front of the container with `AUTH_MODE=headers`
(reads `X-authentik-username`, `X-authentik-email`, `X-authentik-groups`). Only expose the container to the
proxy in that mode.

### Troubleshooting sign-in

When sign-in fails, CraftHub shows a "Sign-in problem" page with the reason, and the same message is in the container
log (`docker logs <container>`).

| Message or symptom | Fix |
|---|---|
| Authentik says *redirect_uri mismatch* / "Redirect URI Error" | The redirect URI in the provider must be exactly `<Public URL>/auth/callback`, mode Strict |
| *could not read …/.well-known/openid-configuration* on Save | Wrong issuer. Copy it from the provider page, with the trailing slash. The CraftHub container must be able to reach that address (DNS and TLS from *inside* Docker) |
| *token endpoint returned 400/401* | Client ID and secret don't belong to the same provider, or the secret was re-generated |
| *id_token failed validation* | Issuer doesn't match the provider, or the server clock is off by more than a minute |
| *login session expired or cookies are blocked* | Cookies are blocked, or the Public URL differs from the address in the browser's address bar |
| *Test signed in but not as an admin* | The user isn't in the admin group, or the groups claim isn't in the token. Check the group name and that the `profile` scope is enabled |
| Plain **502 Bad Gateway** page (not a CraftHub page) | Your reverse proxy can't reach the container, or the container crashed; check `docker logs` |
| Authentik uses a private CA | Mount the CA file and set `EXTRA_CA_FILE=/certs/ca.pem` |

## Your data and updates

Everything CraftHub remembers lives in the **`/data` volume**: the SSO settings (including the client secret) and
GitHub token in `settings.json` (mode 600), the login-cookie key in `session.key`, the installed app versions and
their `state.json`. Updating the **image** does not touch it, so SSO settings, installed apps and signed-in sessions
survive:

```sh
docker compose pull && docker compose up -d
```

They are lost only if the volume is removed (`docker compose down -v`, `docker volume rm`) or if you never mounted
`/data`. Keep the `volumes:` line from the compose file. If you use a bind mount instead of a named volume,
make the folder writable for the container user: `chown -R 65532:65532 ./data`. Back up by copying the volume;
treat the backup as secret, since it holds the client secret.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `PUBLIC_URL` | `http://localhost:8080` | External URL; used for the OIDC redirect URI and the `Secure` cookie flag |
| `AUTH_MODE` | auto | `oidc`, `headers` or `none`. Unset = `oidc` if the three OIDC variables are set, else `none` (configure in the UI) |
| `AUTH_RECOVERY` | `false` | Ignore UI-saved auth and run open, to get back in after a bad config |
| `OIDC_ISSUER` / `OIDC_CLIENT_ID` / `OIDC_CLIENT_SECRET` | | Optional; the UI can set these instead |
| `OIDC_SCOPES` | `openid profile email` | Add `groups` etc. if your provider needs it |
| `OIDC_GROUPS_CLAIM` | `groups` | Claim holding group names |
| `ALLOWED_GROUPS` | empty | Only these (and admins) may sign in; empty = any authenticated user |
| `ADMIN_GROUPS` | empty | May install/roll back/remove. **Empty makes every user an admin** |
| `SESSION_SECRET` | auto | Optional (≥32 chars). If unset, one is generated once and kept in `/data/session.key` |
| `SESSION_HOURS` | `12` | Session lifetime |
| `GITHUB_TOKEN` | | Optional fallback. Normally set it in the UI: **Settings → GitHub token** (admins; stored in `/data/settings.json`, mode 600, and overrides this variable) |
| `UPDATE_INTERVAL_MINUTES` | `30` | How often CraftHub checks GitHub for new releases (and installs them if `AUTO_INSTALL` is on). `0` disables background checks. `UPDATE_INTERVAL_HOURS` is still honoured if set |
| `AUTO_INSTALL` | `true` | Install the newest release automatically (per-app switch in the UI) |
| `KEEP_VERSIONS` | `3` | Versions kept per app (the active one always stays) |
| `INCLUDE_PRERELEASE` | `true` | Follow pre-releases (PhotoCraft currently ships `-rc` tags) |
| `EXTRA_CA_FILE` | | PEM file to trust, for an Authentik behind a private CA |
| `DATA_DIR` | `/data` | Volume: installed apps, `state.json`, optional `apps.toml`, `custom-apps.json` (apps added from the UI) |

### Adding or overriding apps

The catalog is compiled into the image: the storytold apps plus [SolveCraft](https://github.com/bherbruck/solvecraft)
(`bherbruck/solvecraft`). Building the image (`docker build`) includes all of them.

**From the UI (no rebuild):** admins get an **Add app** button. Paste `owner/repo` or any github.com URL of a repository
that publishes releases in the same format (a `...-web-<version>.zip` asset with an `index.html` at its root, and
optionally `SHA256SUMS.txt`). CraftHub checks that the repository exists, lists it with a *custom* badge and remembers it
in `/data/custom-apps.json`. A newly added app is **not installed automatically**: press Install once, and turn on
*Update automatically* in its Versions dialog if you want it to follow new releases. Custom apps can be removed again
from their Versions dialog (this also deletes their installed versions). Built-in apps can't be removed from the UI;
hide one with `disabled = true` in `apps.toml`.

**From a file:** add or change entries in `/data/apps.toml`:

```toml
[[app]]
id = "myapp"
name = "My App"
repo = "someone/myapp"
description = "Whatever"
web_asset_contains = "-web-"   # substring identifying the web zip among release assets
# disabled = true               # hide a built-in app
```

An app only becomes launchable once a release has a web zip with an `index.html` at its root
(a single top-level folder is stripped automatically). Apps without one still show their desktop downloads.

## Security notes

- Downloads must match `SHA256SUMS.txt` (or GitHub's asset digest); a mismatch aborts the install. If a release
  publishes no checksum the install proceeds and is marked **unverified** in the UI.
- Zip extraction rejects path traversal, skips symlinks and caps the unpacked size.
- Session cookies are encrypted, `HttpOnly`, `SameSite=Lax`. State-changing API calls need an admin session
  plus an `X-Requested-With` header.
- **Trust boundary:** the hosted apps are served from the same origin as the launcher, so their JavaScript runs with
  the signed-in user's session. That is fine for code you trust (checksummed releases from your chosen repos), but it
  is not a sandbox. If you need hard isolation, serve apps from a separate origin.
- Put it behind HTTPS (Traefik, Caddy, nginx…).

## Development

```sh
cargo test
AUTH_MODE=none DATA_DIR=./data PUBLIC_URL=http://localhost:8080 cargo run
```

`GITHUB_API_URL` can point at a mock GitHub API for offline testing.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

CraftHub is an independent project and is not affiliated with or endorsed by storytold, Adobe, Microsoft,
Autodesk or any other vendor. Product names are used only to describe what each app is an alternative to,
and belong to their respective owners.
