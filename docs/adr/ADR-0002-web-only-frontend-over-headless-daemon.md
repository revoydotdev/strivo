# ADR-0002 — Web-only frontend over a headless daemon

- **Status:** Accepted
- **Date:** 2026-07-19 (records a decision realized across `2ab4e6c` and the
  `src/intents/` introduction)

## Context

StriVo began with a ratatui-based TUI **and** a web UI. Maintaining two frontends
against one daemon produced an "architectural straddle": duplicated event plumbing
(the retired `AppEvent` wrapper around `DaemonEvent`), a plugin trait bloated with
TUI-only methods (`on_key`, `render_pane`, `panes`, `properties_section`), and two
divergent surfaces for every feature. Meanwhile the product's peers (Sonarr/Radarr,
Jellyfin) are *arr-style web apps, and the core requirement is that **recording and
scheduled capture must work with no UI attached at all**.

## Decision

The **web UI (SPA) is the only supported frontend**, served over HTTP from
`strivo-web`; the ratatui TUI was deleted (`src/tui/`, `src/app.rs`, the `strivo tui`
and `strivo theme` subcommands, the `STRIVO_LEGACY_TUI` escape hatch, and the
`ratatui`/`crossterm`/`ratatui-image` deps).

- A background **daemon** owns monitoring, recording, and scheduling and runs
  headless. Clients — the SPA is just one — attach over a **versioned Unix-socket
  IPC** (`Hello {version}`, `IPC_PROTOCOL_VERSION`), with a server-sent-event stream
  for live updates.
- Recording dispatch is centralised through **one** intent translator
  (`src/intents/`) rather than per-frontend paths, so there is a single recording
  service (VISION AX-6).
- Plugins became headless trigger shells; their UI surfaces read the same SQLite
  stores directly rather than rendering TUI panes.

## Consequences

- One frontend, one event path; the plugin trait shed its TUI surface.
- Headless-first is now structural: `strivo daemon` records with no browser, and
  `strivo serve` is an attachable client (VISION AX-5).
- The daemon IPC is **Unix-socket only** — Windows support requires a named-pipe
  transport behind the IPC abstraction, tracked as ROADMAP M6.P2.
- The SPA is a single-file vanilla-JS app; richer client tooling is a deliberate
  non-goal for now.
