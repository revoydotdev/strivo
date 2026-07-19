# StriVo — Vision & Constitution

StriVo is a self-hosted **live-stream PVR** — "Sonarr/Radarr for live streams." It
monitors Twitch, YouTube, and Patreon channels, records them automatically when
they go live, finalizes each capture into a clean browsable library, and plays it
back in the browser. It ships in two editions from **one** codebase: the default
build is the pure PVR; `--features creator` adds the transcription / analytics /
editor toolkit whose destination is a domain-agnostic **stream→signal** engine.

These axioms are the project's constitution. They are durable: every ADR under
`docs/adr/` must justify itself against them, and the swarm ROADMAP decomposes work
that moves the project toward the 1.0.0 definition below. Axioms change only by a
superseding ADR, never silently.

---

## AX-1 — The PVR is the product; the Creator engine is the upsell
The default `cargo build` is a **complete, excellent live-stream PVR** on its own.
The creator/analytics toolkit is real ambition but it is the *Creator Edition's*
trajectory, gated behind the `creator` Cargo feature. Every change keeps the PVR
build free of creator dependencies; the PVR must never regress to make room for
engine work.

## AX-2 — Capture is sacred
A missed go-live, a dropped stream, or a corrupted recording is the cardinal
failure — everything downstream is worthless without the bytes. Live detection,
the recording pipeline, finalization, and durability across daemon crashes take
precedence over every analytic, visualisation, or editor feature.

## AX-3 — Done means wired end-to-end
A pure-data crate with green unit tests is **necessary but never sufficient**. A
capability is done only when it is instantiated, registered, routed, and reachable
from the daemon and (where user-facing) the SPA on **real input**. Stubs, inert
modules, hardcoded paths, and "tested but disconnected" code are tracked as
blockers, never presented as shipped.

## AX-4 — One-way dependency graph, compile-time editions
The dependency graph is strictly `strivo-core ← strivo-plugins ← strivo-bin`. The
core crate has no awareness of concrete plugins. The PVR/Creator split is a
**compile-time feature**, never a runtime fork or a pair of divergent codebases.

## AX-5 — The daemon is headless-first; the SPA is just a client
Recording, scheduling, and monitoring work with **no browser attached**. The web UI
is one more client that attaches to the background daemon over a **versioned**
Unix-socket IPC. No feature may assume a UI is present for correctness.

## AX-6 — One canonical source per fact
One recording-intent translator (`src/intents/`). One signal store that every
extractor writes and every analytic reads. One credential path (TOML + OS keyring).
No plugin reaches into another plugin's SQLite; no fact is copied where it could be
derived. Duplicated knowledge is a defect.

## AX-7 — Credentials never touch disk as plaintext; trust is verified, not assumed
Secrets live in the OS keyring, never in plaintext on disk or in logs/fixtures.
Anything the daemon is asked to trust — most sharply the licence JWT — is
**cryptographically verified** before its payload is believed. A documented-but-
unwired safeguard is worse than none.

## AX-8 — Stability is a dated promise
The config format, the daemon IPC protocol, and the plugin ABI are explicitly
**unstable until 1.0** and **frozen at 1.0**. Breaking changes are versioned,
migration-noted in `CHANGELOG.md`, and never silent. Before 1.0 we may break; at
1.0 we commit.

---

## Done at 1.0.0

StriVo reaches **1.0.0** when all of the following hold:

1. **PVR complete & durable** — go-live detection, live + VOD capture, finalization
   (gap-resume merge, ad-trim, remux), library, scheduling, and monitoring are
   wired end-to-end, and in-flight recordings **survive a daemon crash** (journal
   recovers the process, not just replays status).
2. **Contracts frozen** — config schema, daemon IPC protocol version, and plugin
   ABI are stabilized and documented; upgrades no longer require hand-editing
   `config.toml`.
3. **Platforms** — Linux and macOS are first-class; Windows daemon transport
   (named pipes) has either landed or been explicitly deferred by an ADR.
4. **Creator Edition wired** — under `--features creator`, the engine runs end-to-
   end: unified signal spine → daemon-driven DAG executor → domain-agnostic
   extraction adapters → analytics over real corpora → visualisation/composer →
   clip/export, with real-time ("as fast as it is recorded") extraction and at
   least the Sports and Creator domain templates.
5. **Trust is real** — the licence JWT signature is verified; no `TODO`-gated
   security safeguard remains in a shipped path.
6. **Docs match reality** — `README.md`, `DESIGN.md`, `CHANGELOG.md`, and the ADR
   index describe what actually ships; no aspirational claim outruns the code.
