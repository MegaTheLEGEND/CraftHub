# CraftHub: status and TODO

## Original goal
Take **CraftHub** (self-hosted, Docker, Rust/axum web-build host with SSO) and:
1. make it **look and feel like the ArtCraft Launcher**,
2. **rebrand it to "ArtCraft Server"**,
3. **keep every CraftHub feature**,
4. **reuse the launcher's applicable infrastructure** instead of CraftHub's own,
5. **keep it running in Docker**.

## Done (snapshot state: WORK IN PROGRESS, not release-ready)

### Launcher infrastructure reused
- [x] Launcher core crate vendored in `crates/core` (MIT OR Apache-2.0, untouched): catalog, versions, release parsing with URL allow-list, SHA256SUMS parsing, platform/asset choice, status model, release feed, product-page text
- [x] Catalog = the 13 suite apps from the launcher (CraftHub had 12) + SolidCraft (see note 1)
- [x] Desktop downloads picked per visitor platform with the launcher's `pick_asset`
- [x] Release feed used as a fast path for update checks (GitHub fallback), plus feed announcements/alerts and repo renames
- [x] Launcher's 10 translation files (`i18n/*.tsv`) served to the UI; same language detection rules
- [x] App icons bundled in the binary (no GitHub icon fetch for suite apps); product pages + screenshots (proxied and cached by the server)

### CraftHub features carried over
- [x] Install / activate / roll back / remove versions, hosting at `/apps/<id>/` and preview at `/versions/<id>/<v>/`
- [x] Auto-update loop, per-app auto-update + pre-release switches, `KEEP_VERSIONS` pruning, checksum verification
- [x] Add / remove custom GitHub apps, GitHub token in the UI
- [x] Authentik/OIDC SSO with test-before-enable, admin vs user groups, proxy-header mode, recovery mode
- [x] Same env vars and `/data` layout (cookie names changed, so users sign in again once)

### New (launcher-style)
- [x] Server: Cancel install, Uninstall, server-side "Update all" with queue, rate-limit tracking, `solvecraft -> solidcraft` rename handling
- [x] UI: launcher header/tabs, filter chips with counts, sort, search, grid with numbered cards + table view, group headers, badges, drawer with details + screenshots, hide/unhide apps, dark/light/system theme, safe built-in Markdown renderer (no CDN)
- [x] Fonts (Inter, Archivo, Geist Mono) subset to woff2, 430 KB total
- [x] 53 unit tests pass (`cargo test --workspace`); `cargo build` succeeds

## TODO (to reach the goal)

### Done in the last session
- [x] UI opened in a browser for the first time (dark/light, desktop/mobile): found and fixed a JS crash (`t` declared twice: inline theme script vs app.js). Layout looks right.
- [x] Docker: `Dockerfile` (dependency-cache layer, distroless cc, healthcheck), `docker-compose.yml` (`artcraft-data`, migration note), `.env.example`, `.dockerignore`
- [x] `ldd` of the binary: only libgcc_s, libm, libc, all in distroless/cc (no liblzma needed)
- [x] GitHub Actions `ci.yml` (tests on the workspace, image `artcraft-server` to GHCR)
- [x] README rewritten, `NOTICE` written
- [x] "Like Adobe X" lines now off by default (`SHOW_INSPIRED_BY=false`)
- [x] New `GITHUB_DOWNLOAD_URL` setting (mirror / testing)
- [x] `cargo build --locked` and `cargo test --workspace --locked` pass (53 tests)

### Done in the latest session (no Rust toolchain or browser in the sandbox, so none of this was built or run)
- [x] Likely source of the harmless console 404 found: `/api/apps/<id>/icon` answered 404 for apps without a bundled icon (SolidCraft, custom apps) when GitHub has none. It now answers 204 (`src/api.rs`, `icon()`); the UI already falls back to the letter tile. Unknown app ids still return 404.
- [x] Keyboard/accessibility pass in `ui/app.js` + `ui/index.html`: focus survives the 15 s refresh and filter/sort re-renders; the "..." menu takes focus, supports Up/Down, and Esc returns focus to its button; the details drawer and screenshot lightbox move focus in, trap Tab, and restore focus on close; sortable table headers and the screenshot opener are focusable and work with Enter/Space; `aria-pressed` on view/filter buttons, `aria-sort` on headers, progress bars have `role="progressbar"`; labels for the search box, sort select, icon buttons, and dialogs; the "..." button was wrongly labelled "Settings", now "More actions".

- [x] First install is manual: `AUTO_INSTALL` now defaults to `false` (`src/config.rs`); installed apps still auto-update, `AUTO_INSTALL=true` restores the old behaviour. README, `.env.example`, Settings text updated. Not compiled.

- [x] Checked the SolidCraft 0.14.0 release page against the asset picker (by reading the code, not running it): web zip `solidcraft-web-0.14.0.zip` is picked and its digest used; `.snap`, `.exe`, `.zsync` and unversioned duplicates are ignored. Added unit test `solidcraft_0_14_0_release_page` (not compiled).

- [x] Brand back to **CraftHub** with the ArtCraft look: wordmark `CRAFTHUB`, tag `Self-hosted`, package/binary/image `crafthub`, volume `craft-data` again, cookies `crafthub_session`/`crafthub_login`, localStorage `crafthub.settings` (saved theme/view settings reset once). Trademark note 3 below is resolved by this. Not compiled.
- [x] "no web build" on every release: web asset detection now falls back to `-web-` when an entry's own marker (e.g. an old `solvecraft-web`) matches nothing. Test extended. Not compiled.

### For you to test (not verified by me)
- [ ] Run `cargo build --locked && cargo test --workspace --locked` after the `icon()` change (one-line status-code edit, not compiled by me)
- [ ] Reload in a browser and confirm the console 404 is gone; tab through the page, the menu, the drawer and the lightbox
- [ ] `docker build -t artcraft-server .` and `docker compose up -d --build` (no Docker daemon in my sandbox, never built)
- [ ] Install / update / cancel / uninstall / Update all / roll back / preview / SSO flows with real releases (E2E tests skipped on request)
- [ ] The feed's live response
- [ ] Mobile layout pass (keyboard pass is done above, still needs your check)
- [ ] Paste your "notice from a human" paragraph into README.md (placeholder; the original wasn't in the files)

### Later
- [ ] Translate server-only strings (admin panels, Add app, Versions, SSO) beyond English
- [ ] Decide on pause/resume downloads (only Cancel exists)

## Notes and decisions you need to make

1. **SolidCraft**: your README listed "SolveCraft", but `bherbruck/solvecraft` on GitHub is now **`bherbruck/solidcraft`** ("parametric 3D CAD in pure Rust"). The old name would break downloads under the launcher's strict URL check, so the built-in is `solidcraft`. Existing custom entries with the old repo name keep working (same-owner renames are followed).
2. **"Inspired by"**: the current launcher removed its "Like Adobe Photoshop" lines (older screenshots still show them). CraftHub had them, so they are kept and can be turned off with `SHOW_INSPIRED_BY=false`.
3. **Trademark: please read.** The launcher's brand licence (`docs/brand/LICENSE-brand.txt`) says forks **must not be called ArtCraft** or use its logo/wordmark. You asked for "ArtCraft Server", so I used that name, but only as text. I did **not** copy the ArtCraft logo/"A" mark files; the header wordmark is plain text in the launcher's display font and the favicon is my own. Before publishing the Docker image or repo under this name, get the ArtCraft team's OK or rename. The name lives in one file, `src/brand.rs` (+ Docker/README names), so renaming is quick.
4. Rust toolchain used here: 1.91 (the launcher core requires 1.90+); the Dockerfile must use a recent `rust:1` image.
- [ ] SolidCraft moved to `tangency-labs/solidcraft` (`src/config.rs`). Its icon is fetched from that repo by the server and cached. To bundle it like the suite apps, save `assets/icons/app/linux/exports/hicolor/256x256/apps/ai.storyteller.solidcraft.png` from that repo as `ui/icons/solidcraft.png` (the icon route serves a bundled file first for every built-in app).
- [ ] Run `cargo build --locked && cargo test --workspace --locked` after the SolidCraft repo/icon change (not compiled by me)
