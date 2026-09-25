# strivo-web — the browser front end

The StriVo web UI. It serves a vanilla-JS single-page app and the JSON API the
app talks to, and reaches the running `strivo` daemon over that daemon's local
IPC transport (a Unix socket, or a named pipe on Windows). It never owns state
directly.

There is no TUI. The ratatui front end, `src/tui/`, `src/app.rs`, and the
`strivo tui` / `strivo theme` subcommands were all deleted in 0.6.0 — see the
"Removed" section of [CHANGELOG.md](../../CHANGELOG.md). The browser is the only
UI this project ships.

## Quickstart

```bash
# In one terminal:
strivo daemon

# In another:
strivo serve --bind 127.0.0.1:8181
# strivo-web on http://127.0.0.1:8181

curl -H "X-Api-Key: $KEY" http://127.0.0.1:8181/api/v1/channels
xdg-open http://127.0.0.1:8181/
```

## Stack

- Axum 0.8 (HTTP)
- rust-embed (asset tree embedded in the binary)
- Vanilla ES modules — no framework, no component library, no JS build toolchain
- `build.rs` concatenates `assets/spa/*.js` and `assets/spa-css/*.css`, in
  filename order, into a single `spa.js` / `spa.css` pair; release builds
  minify both. Modules are named `<seq>-pvr.<ext>` or `<seq>-creator.<ext>`, and
  only the `-pvr` ones land in a PVR build (ADR 0002).
- `assets/spa.html` is the shell: `<link rel="stylesheet" href="/assets/spa.css">`
  plus `<script type="module" src="/assets/spa.js">`, served at `/` and `/app`.
- Askama is still a declared dependency and `templates/` still holds the old
  server-rendered pages, but no route renders them and no served asset loads
  them. They are residue, not a live rendering path.

The SPA modules are fragments split mid-file, so an individual
`assets/spa/NNN-*.js` will not parse on its own. Judge syntax on the assembled
bundle, not the fragment.

## Routes

The page surface is one shell; everything else is JSON under `/api/v1` plus two
SSE feeds.

| Path | Purpose |
|---|---|
| `/`, `/app` | SPA shell (`routes::assets`) |
| `/assets/*` | Embedded asset tree, including the assembled `spa.js` / `spa.css` |
| `/api/v1/*` | JSON API (`routes::api` and the handler modules split out of it) |
| `/api/v1/recordings/{id}/download` | Raw file stream, range requests supported (`routes::recordings`) |
| `/api/v1/recordings/{id}/play` | Redirect to the download stream |
| `/events` | SSE daemon-event relay (`routes::events`) |
| `/api/v1/auth/login`, `/api/v1/auth/logout` | Cookie session issue/clear (`routes::login`) |
| `/yt-websub` | Public YouTube WebSub callback (`routes::websub`) |
| `/api/v1/multistream/*` | Multi-stream tile layout for the watch player |

Creator-only routers (first-party plugin endpoints, Pro licence
status/activate/trial/refresh) are gated at the module declaration with
`#[cfg(feature = "creator")]` and are absent from a PVR build entirely.

## Auth

Two accepted tracks, via `auth::check_dual`:

- **Programmatic** — the `X-Api-Key` header (the *arr convention). Generated on
  first run and persisted to `[web] api_key` in `config.toml`; override per-run
  with `strivo serve --api-key <KEY>`. Compared in constant time. To rotate,
  delete the field from `config.toml` and restart.
- **Browser** — a HMAC-SHA-256 session cookie issued by
  `POST /api/v1/auth/login`, keyed by `[web] session_secret` (generated and
  persisted on first login), 7-day rolling lifetime.

Default bind is `127.0.0.1`.

## CSRF posture

`src/csrf.rs` guards POST/PUT/PATCH/DELETE, with a different stance per track:

- A request carrying `X-Api-Key` is exempt — a cross-origin browser cannot
  forge that header without a CORS preflight, which is not granted.
- A cookie-authenticated mutation must carry a custom `X-Strivo-CSRF` (or
  `X-Requested-With`) header *and* an `Origin` (preferred) or `Referer` whose
  host matches the server's `Host`. Anything else gets `403 Forbidden`.

There are still no per-form CSRF tokens. Operators binding to a non-loopback
interface should run behind a reverse proxy that adds them; full per-form tokens
land in a follow-up.

## Daemon dependency

`strivo serve` refuses to start if the daemon isn't reachable and suggests
`strivo daemon`.
