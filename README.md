# CraftHub

<img width="1920" height="988" alt="image" src="https://github.com/user-attachments/assets/61bab7b8-0fe5-4383-aef2-7d1355c2cb81" />

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
docker compose up -d --build
```

For a first look without SSO: `AUTH_MODE=none PUBLIC_URL=http://localhost:8080` (everyone is admin; local use only).

## Turning on SSO from the UI

The container starts with authentication **off** (a banner says so). Open **Settings → Authentication**, enter the
Authentik details, then:

1. **Save** (the issuer is checked immediately; the secret is stored in `/data/settings.json`, mode 600, never shown again).
2. **Test sign-in** opens a real login with those settings in a new tab. It must succeed *as an administrator*.
3. **Enable SSO**. This is only possible after a passing test of exactly the saved settings (valid 30 min), so a typo can't lock you out.

**Turn authentication off** reverts it. If you do get locked out anyway, start once with `AUTH_RECOVERY=true`
(ignores saved auth and runs open), fix the settings, and remove the variable. Setting `OIDC_ISSUER`,
`OIDC_CLIENT_ID` and `OIDC_CLIENT_SECRET` in the environment still works as before and enables SSO at boot.

## Authentik setup

1. **Applications → Providers → Create → OAuth2/OpenID Provider**
   - Client type: *Confidential*
   - Redirect URI (strict): `https://craft.example.com/auth/callback`
   - Scopes: `openid`, `profile`, `email`. For group roles the ID token (or userinfo) must carry a `groups` claim;
     if yours doesn't, add a scope mapping returning `{"groups": [g.name for g in request.user.ak_groups.all()]}`.
   - Signing key: either works (RS256 via JWKS, or HS256 signed with the client secret).
2. **Create an Application** using that provider, with slug `craft-hub`.
3. Create groups `craft-users` and `craft-admins` and bind them to the application (policy binding) as needed.
4. Set `OIDC_ISSUER` to `https://auth.example.com/application/o/craft-hub/` (the trailing slash matters; it is the
   *Issuer* shown on the provider page), plus the client id and secret.

Alternative: Authentik's **proxy outpost** in front of the container with `AUTH_MODE=headers`
(reads `X-authentik-username`, `X-authentik-email`, `X-authentik-groups`). Only expose the container to the
proxy in that mode.

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
| `UPDATE_INTERVAL_HOURS` | `6` | `0` disables background checks |
| `AUTO_INSTALL` | `true` | Install the newest release automatically (per-app switch in the UI) |
| `KEEP_VERSIONS` | `3` | Versions kept per app (the active one always stays) |
| `INCLUDE_PRERELEASE` | `true` | Follow pre-releases (PhotoCraft currently ships `-rc` tags) |
| `EXTRA_CA_FILE` | | PEM file to trust, for an Authentik behind a private CA |
| `DATA_DIR` | `/data` | Volume: installed apps, `state.json`, optional `apps.toml` |

### Adding or overriding apps

The catalog ships with the storytold apps. Add or change entries in `/data/apps.toml`:

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
