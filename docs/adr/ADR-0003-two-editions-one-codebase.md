# ADR-0003 — Two editions from one codebase via the `creator` feature

- **Status:** Accepted
- **Date:** 2026-07-19 (records the shipped PVR/Creator edition split)

## Context

StriVo is two products in tension: a focused **live-stream PVR** (capture, library,
scheduling, monitoring) and an ambitious **creator/analytics engine** (transcription,
clip discovery, an EDL editor, and a domain-agnostic stream→signal pipeline built
from ~34 in-tree tool crates). An earlier "identity collapse" review found the two
blurred together, which risked a bloated default build, unclear positioning, and a
PVR that could regress to serve engine work. But the engine genuinely builds on the
PVR substrate — a hard fork would duplicate the capture pipeline and diverge.

## Decision

Ship **both editions from one codebase**, gated at **compile time** by the `creator`
Cargo feature (VISION AX-1, AX-4):

- **StriVo** (default, `cargo build`) — the pure PVR. No creator dependencies.
- **StriVo Creator Edition** (`cargo build -p strivo-bin --features creator`) — the
  PVR **plus** the creator toolkit.
- The workspace `default-members` are the PVR crates, so plain `cargo build`/`cargo
  test` *is* the PVR; `--workspace` and `-p … --features creator` reach the rest.
- `strivo-web` makes all tool-crate deps `optional`; the plugins/marketplace/
  pipelines/capabilities routes mount only under the feature. `strivo-core` gates the
  Crunchr/Archiver config sections and the tandem handshake. `strivo-bin` registers
  the first-party plugins only under the feature and fans out to
  `strivo-web/creator` + `strivo-core/creator`.
- `creator_enabled` is exposed in `/api/v1/settings` so the SPA hides creator UI in
  the PVR build at runtime.

There is **no runtime edition fork** and **no divergent branch** — the boundary is a
feature flag, verified by a `compile_error!` probe and by the PVR build refusing to
pull creator deps.

## Consequences

- The PVR stays small and shippable; the engine is opt-in.
- New creator surfaces **must** be gated behind the feature and kept out of the PVR
  build's dependency closure — the CI gate is "PVR build has no creator deps."
- Two build/test matrices exist: `cargo test` (PVR) and `cargo test --workspace
  --features creator` (Creator). ROADMAP gates reflect both.
- The commercial thesis (Creator Edition as the upsell) is encoded in code, not just
  positioning — but it means every Creator milestone must prove it did not leak into
  the default build.
