# ADR-0004 — Platform trait + plugin event bus with a one-way dependency graph

- **Status:** Accepted
- **Date:** 2026-07-19 (records the core extensibility architecture)

## Context

StriVo must support several external services (Twitch, YouTube, Patreon) whose auth,
polling, and stream-resolution differ, and it must let optional capabilities
(transcription, archival, analytics) react to recordings without entangling the
capture pipeline. Two naïve failure modes loom: the core learning about every
concrete platform/plugin (a coupling knot), and plugins reaching sideways into each
other's state.

## Decision

Two abstractions and one dependency rule:

1. **Platform trait.** Adding a service means implementing one trait; auth, polling,
   and recording are decoupled from platform specifics (`src/platform/`). The monitor
   fans platform fetches out concurrently (`join_all`) but keeps state mutations
   serial.
2. **Plugin event bus.** Plugins react to `DaemonEvent`s (go-live, recording
   finalized) rather than being called from inside the recording pipeline. Plugins
   are headless trigger shells; the plugin trait/registry in `strivo-core` is generic
   and knows nothing about concrete plugins.
3. **One-way dependency graph** (VISION AX-4): `strivo-core ← strivo-plugins ←
   strivo-bin`. The core never depends on concrete plugins; the binary composes both.
   The first-party plugins (`crunchr`, `archiver`, `insights`, `editor`, `viewguard`)
   live in-tree under `crates/strivo-plugins/` (the former separate repo was folded
   in).

Credentials flow through one path — TOML config + OS keyring — never plaintext on
disk (VISION AX-7). Plugin state is SQLite, but **no plugin may reach into another
plugin's database**; the canonical cross-plugin substrate is the unified signal
store (ROADMAP M2). The current `insights → crunchr.db` reach-in and the
`viewguard` `data_dir` double-nest are known violations tracked for repair.

## Consequences

- New platforms and plugins are additive and isolated; the core stays stable.
- Plugins that need another's data must go through a shared contract (the signal
  store), not a sibling-DB path — this is an explicit invariant the swarm audits
  (ROADMAP M2G2).
- Dynamic `cdylib` plugin loading is coded but intentionally dormant until
  third-party plugins are real; same-toolchain compilation is required until an ABI
  is frozen (ROADMAP M6.P1).
- The event-bus indirection costs a little directness for a lot of decoupling — an
  accepted trade under VISION AX-6.
