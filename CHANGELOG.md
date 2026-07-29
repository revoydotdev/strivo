# Changelog

All notable changes to strivo will be documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- **ROADMAP regenerated around the engine north star.** `ROADMAP.md` is now the
  single authority: it reframes StriVo as a domain-agnostic stream→clip analytics &
  content-creation engine (capture PVR + DAW plugins as substrate), grounds the
  honest build state (incl. the inert daemon-side pipeline executor and fragmented
  per-plugin SQLite), and lays out phases P1–P8 with every blocker/stub tracked under
  an explicit definition-of-done. README identity language reconciled (web-only;
  engine framing); adversarial-review findings folded in as tracked status.
- **`strivo-plugins` folded into the workspace.** The separate
  `Chorosyne/strivo-plugins` repo is retired. The five first-party
  plugins (`crunchr`, `archiver`, `insights`, `editor`, `viewguard`)
  now live in-tree at `crates/strivo-plugins/`. Removed: the git
  submodule, the `pro` cargo feature gate in `strivo-bin` and
  `strivo-web` (plugins always build), `PLUGINS_PRIVATE.md`,
  `.gitmodules`, and the `[patch."https://github.com/Chorosyne/strivo"]`
  block in the root `Cargo.toml`. Contributors no longer need
  `--recurse-submodules` or a private plugins clone.

### Removed
- **The ratatui-based TUI is gone.** Deleted: `src/tui/` (36 files),
  `src/app.rs` (4537 lines), the `strivo tui` CLI subcommand, the
  `strivo theme` subcommand (was TUI theming), the `STRIVO_LEGACY_TUI`
  escape hatch, `run_tui`/`run_client`/`run_legacy_tui` from
  `crates/strivo-bin/src/main.rs`. Dropped deps: `ratatui`,
  `crossterm`, `ratatui-image` (host workspace) + same from
  `crates/strivo-plugins`. Plugin trait surgery: `on_key`,
  `render_pane`, `panes`, `properties_section` removed; `key`/
  `modifiers` fields removed from `PluginCommand`; `status_line`
  takes no `&AppState`. Plugins (Crunchr, Archiver, Insights, Editor,
  Viewguard) are now headless trigger shells — their webui surfaces
  read the same SQLite stores directly. The non-TUI emitter sites
  (platforms, monitor, recording, daemon) now publish
  `DaemonEvent` directly instead of wrapping it in the retired
  `AppEvent`.

### Added
- **Durable Creator publish DAG.** Creator Edition now ships a daemon-owned,
  restart-safe ten-stage workflow spanning transcription intelligence, visual
  cuepoints, chapters, captions, brand-safety analysis, highlight scoring,
  clip and thumbnail export, reuse drafts, and a converged Casebook report.
  The web UI exposes live stage state, bounded retries, cancellation, manual
  retry, run history, and authenticated streamed artifact downloads.
- **Recording finalisation pipeline + browser playback.** `Recording::from_file`
  now derives stable `UUIDv5` ids from the canonical output path (webui permalinks
  survive daemon restarts); a new `src/recording/remux.rs` losslessly remuxes
  MPEG-TS bytes (yt-dlp hls-native output) to Matroska so Chromium can play Twitch
  HLS captures, keeping a `.orig` backup; `finalize_completion()` merges gap-resume
  segments, runs Twitch ad-trim, then normalises the container. The web layer adds
  RFC 9110 HTTP Range support (206 Partial Content) for `<video>` seeking, plus the
  SPA recordings playback surface.
- Community-health files: `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`,
  bug / feature issue templates, pull-request template, `CODEOWNERS`, weekly
  Dependabot configuration (cargo + github-actions + git-submodules).
- Annotated `config.toml.example` covering every user-facing config section,
  not just theming.
- `docs/FIRST-RUN.md`, `docs/DAEMON.md`, and `docs/PLUGIN-TEMPLATE.md`.
- README "Known limitations" section and explicit alpha-status callout.
- Modern badge row (CI status, latest release, MSRV, AUR version,
  platforms).
- `docs/demo/demo.tape` skeleton for regenerating the README demo recording
  with [VHS](https://github.com/charmbracelet/vhs).

### Changed
- README platform table now reflects reality: daemon mode is Unix-only;
  Windows is unsupported pending a named-pipe transport.
- `.gitmodules` and Cargo `repository`/`homepage` fields repointed from
  `revelri/strivo*` to the canonical `Chorosyne/*` org URLs.
- ROADMAP gains a "Quick roadmap" preamble for visitors; internal-only
  design-note references are de-linked.
- `docs/PLUGIN-MANIFEST.md` opens with an alpha plugin-safety banner.

### Fixed
- Daemon startup no longer panics inside a spawned task if SIGTERM handler
  registration fails — the error is now propagated from `daemon::run()`.
- Recording-journal persistence logs a structured serialization error and
  writes a diagnostic marker payload instead of silently storing an empty
  string when `serde_json::to_string(job)` fails.

### Removed
- Internal-only design notes (`REVIEW.md`, `YAZI-AUDIT.md`,
  `FOLLOWUP-PLUGIN-WALK.md`) are no longer tracked — public-root sprawl
  cleanup. Historical content is recoverable from git history.
- `docs/ROADMAP.md` — stale internal engineering checklist (mostly
  completed phases); the public product roadmap stays at `/ROADMAP.md`.

## [0.3.0] — 2026-05-18

### Added
- Dynamic plugin loader: `libloading`-based `cdylib` discovery via the
  per-plugin TOML manifest at `~/.config/strivo/plugins/`. Same-toolchain
  caveat documented in `docs/PLUGIN-MANIFEST.md`.
- User-authored themes at `~/.config/strivo/themes/*.{toml,conf}`, including
  a Kitty / Ghostty `.conf` parser and a `strivo theme import` CLI helper.
- Runtime theme switching: `Ctrl+T` picker overlay with live preview, Enter
  to commit, Esc to revert via `Theme::snapshot`/`restore`, `R` to rescan.
- Rich-table theme form with `[theme.colors]` and `[theme.ansi]` overlay
  overrides on top of any built-in or user theme.
- Stream-gap auto-resume orchestrator (M5.5).
- Cost UI integration for transcription backends.

### Changed
- `ThemeRef` accepts both the legacy string form and the new rich-table
  form via `#[serde(untagged)]`; existing configs continue to load
  unchanged.
- `strivo-plugins` is consumed via a git dependency (with a workspace
  `[patch]` back to local `strivo-core`) in addition to the submodule, so
  in-tree builds keep a single `strivo-core` trait identity.

## [0.2.0] — 2026-04-19

### Added
- Tier-1 navigation: Home / End across all panes; help overlay bound to
  `F5`, plus `t` / `R` / `g` / `G` / `Home` / `End` and consistent Esc
  semantics.
- Quit-during-recording modal with live elapsed-seconds counter and a
  per-job ✓ checklist.
- Daemon disconnect banner with an auto-reconnect supervisor (1 / 2 / 5 /
  10 / 30 s backoff).
- In-TUI device-code wizard; `AppAction::OpenUrl` opens the verification
  URL with the appropriate platform handler (`xdg-open` / `open` /
  `start`).
- Pre-record disk-space gate (≥ 5 GB free, via `statvfs`).
- Retry-exhaustion error surface: `rec.job.error` plus a final
  `RecordingFinished` event.
- 10 integration tests (config round-trip, filename collision, IPC
  handshake); 72 tests total green in CI on a self-hosted runner.

### Changed
- Esc precedence: clear filter first, then navigate back. Status indicator
  reads `[/query] N/M · Esc clears`.
- Search input is now cursor-editable; `status_message` renders in the
  hotkey bar with a 5-second auto-dismiss.
- OAuth flows refresh on 401 across Twitch, YouTube, and Patreon.
- Rate-limit backoff uses a shared `parse_retry_after` that honours both
  `Retry-After` and `RateLimit-Reset`.
- Daemon socket hygiene: `sweep_stale_files` plus pid + socket unlink on
  shutdown. Stale-pid detection uses `kill(pid, 0)` plus an actual
  `connect(2)` cross-check.
- Config corruption recovery: `.backup` fallback, quarantine of the bad
  file, fall through to defaults.
- Transcode mode now persists through the Settings panel and the `t`
  hotkey.

### Fixed
- Credential leak: `strivo config get` refuses `*_secret` / `*_token` /
  related keys.
- Keyring single-point-of-failure: `STRIVO_*` env-var fallback with a
  once-only warning log.
- Filename collision: numeric `_N` (1..999) suffix, then UUID fallback.
- Standalone `PollNow` now wakes the monitor via
  `Arc<Notify>` from `ChannelMonitor::poll_notify()`.

## [0.1.0] — 2026-03-14

### Added

- TUI dashboard with sidebar navigation, channel detail view, recording
  list, settings panel, and status bar.
- Setup wizard for first-run configuration.
- Twitch platform integration (OAuth app flow, channel lookup, live-status
  polling).
- YouTube platform integration (Data API v3, live-broadcast detection,
  cookie-based auth).
- FFmpeg-based stream recording with MKV output.
- Optional video-transcoding pipeline.
- Configurable filename templates (`{channel}_{date}_{title}.mkv`).
- Auto-record support for configured channels.
- Live playback through mpv.
- Stream-URL resolution via streamlink and yt-dlp.
- Channel monitoring with configurable poll interval.
- Desktop notifications on go-live events.
- TOML configuration with XDG-compliant paths.
- OS keyring credential storage.
- Live log viewer widget in the TUI.
- CLI subcommands for config management (`config list / get / set / path /
  reset`).
- CLI subcommands for log management (`log path / clear / tail`).
- Dialog system for confirmations and input.
- Color theme system for the TUI.

[Unreleased]: https://github.com/Chorosyne/strivo/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/Chorosyne/strivo/releases/tag/v0.3.0
[0.2.0]: https://github.com/Chorosyne/strivo/releases/tag/v0.2.0
[0.1.0]: https://github.com/Chorosyne/strivo/releases/tag/v0.1.0
