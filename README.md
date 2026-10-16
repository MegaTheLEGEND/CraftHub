# CraftHub

A self-hosted, Docker-ready launcher for the Craft App Suite, styled after the ArtCraft Launcher. It finds,
downloads, verifies and hosts the **web builds** of the suite apps (PhotoCraft, VectorCraft, ...),
keeps them updated, and gives visitors the right desktop download for their platform. Rust/axum,
one small container, optional Authentik/OIDC SSO.

<img width="1920" height="1694" alt="image" src="https://github.com/user-attachments/assets/5ea90e54-5d05-4c05-8078-e804ff4de812" />


## Quick start

```sh
cp .env.example .env      # optional
docker compose up -d --build
```

Open http://localhost:8080. Without SSO configured, anyone who can reach the port is an
administrator, so turn on SSO in Settings (or put it behind your reverse proxy) before exposing it.
Data lives in the `craft-data` volume (`/data`).

Apps are hosted at `/apps/<id>/`; any installed version can be previewed at `/versions/<id>/<version>/`.

## Features

- Catalog of the 13 suite apps plus SolidCraft, and custom GitHub apps (Add app).
- Install, activate, roll back, remove versions; cancel running installs; uninstall; Update all.
- Auto-update loop with per-app auto-update and pre-release switches, `KEEP_VERSIONS` pruning,
  SHA-256 verification against GitHub digests / `SHA256SUMS.txt`.
- Desktop downloads chosen per visitor platform with the launcher's own logic.
- Uses ArtCraft's release feed as a fast path, falling back to the GitHub API.
- Launcher-style UI: grid and table views, filters, sort, search, details drawer with
  screenshots, hide apps, dark/light/system theme, 10 languages (server-only strings are English).
- SSO: Authentik/OIDC with test-before-enable, admin vs user groups, proxy-header mode, recovery mode.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `PUBLIC_URL` | `http://localhost:8080` | External URL (OIDC redirect: `<PUBLIC_URL>/auth/callback`) |
| `BIND` / `DATA_DIR` | `0.0.0.0:8080` / `/data` | Listen address / storage |
| `GITHUB_TOKEN` | – | Raises the GitHub rate limit (also settable in the UI) |
| `UPDATE_INTERVAL_MINUTES` | `30` | Update check interval |
| `AUTO_INSTALL` | `false` | Also install apps that are not installed yet (default: first install is manual; installed apps still update automatically) |
| `KEEP_VERSIONS` | `3` | Installed versions kept per app |
| `INCLUDE_PRERELEASE` | `true` | Offer pre-releases |
| `SHOW_INSPIRED_BY` | `false` | "Like Adobe ..." lines |
| `AUTH_MODE` | auto | `oidc`, `headers` or `none` |
| `OIDC_ISSUER`, `OIDC_CLIENT_ID`, `OIDC_CLIENT_SECRET`, `OIDC_SCOPES`, `OIDC_GROUPS_CLAIM` | – | OIDC settings |
| `ALLOWED_GROUPS`, `ADMIN_GROUPS` | – | Comma-separated; empty admin groups = every user is admin |
| `HEADER_USER`, `HEADER_EMAIL`, `HEADER_GROUPS` | `X-authentik-*` | Headers mode |
| `SESSION_SECRET`, `SESSION_HOURS` | generated / `12` | Cookie key (min 32 chars) / lifetime |
| `AUTH_RECOVERY` | `false` | Ignore saved auth (lock-out recovery) |
| `FEED_URL` | ArtCraft's feed | Empty disables the feed |
| `GITHUB_API_URL`, `GITHUB_DOWNLOAD_URL` | GitHub | Mirrors / testing |
| `EXTRA_CA_FILE` | – | Extra PEM CA for outbound HTTPS |

## SSO with Authentik

1. Create an OAuth2/OpenID provider and application; redirect URI `<PUBLIC_URL>/auth/callback`.
2. Make sure the `groups` claim is in the token (the default `profile` scope mapping does this).
3. In CraftHub: Settings, enter issuer (`https://auth.example.com/application/o/<slug>/`),
   client ID/secret, admin group; **Test**, then **Enable**.
Behind an Authentik proxy outpost instead? Set `AUTH_MODE=headers` and only expose the container to the proxy.
Locked out? Start once with `AUTH_RECOVERY=true`.

## Upgrading

Same env vars, `craft-data` volume and `/data` layout as before. If you ran the interim "ArtCraft Server"
build, copy its `artcraft-data` volume (command in `docker-compose.yml`); everyone signs in once more.
`solvecraft` is now `solidcraft` (old entries keep working).

## Development

```sh
cargo test --workspace
GITHUB_API_URL=http://127.0.0.1:9100 GITHUB_DOWNLOAD_URL=http://127.0.0.1:9100 FEED_URL= cargo run
```

## Name

The name is CraftHub (`src/brand.rs`). The look follows the ArtCraft Launcher, but no ArtCraft logo or
wordmark is used or shipped. See `NOTICE` for third-party attribution.
