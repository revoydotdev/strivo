# ADR-0005 — Supersede the revoy-format ROADMAP with the swarm scheme

- **Status:** Accepted
- **Date:** 2026-07-19

## Context

StriVo was previously tracked by **revoy**, a cross-project ledger whose per-repo
state lived in `ROADMAP.md` as a narrative document plus a machine-readable
`<!-- revoy:begin -->` TOML phase block (phase name + a flat list of `[[todo]]`
entries with `difficulty`/`priority`). That format served a portfolio-level
changelog and phase ledger, but it does not carry the structure the autonomous swarm
harness (vendored under `.agents/`, ADR-0028 lineage) requires to dispatch work:

- No stable, hierarchical todo IDs (`M#.P#.S#.T#`) for a tick to claim disjoint
  concerns against.
- No per-todo **artifact check** — the swarm's `ledger.py done --run` refuses to
  record a todo done unless a named command exits 0 (provenance-as-a-type). The
  revoy block had priorities, not verifiable exit-0 gates.
- No milestone **quality gates** (`M#G#`) or `M#.P9` gate sections.
- `roadmap-slice.sh` and `ledger.py next` parse the `# M#` / `**`M#.P#.S#.T#`**`
  grammar, which the revoy block did not use.

The swarm-enrollment task (ADR-0028) explicitly authorizes superseding a revoy-era
ROADMAP and requires the audit trail to be an ADR.

## Decision

`ROADMAP.md` is **rewritten in the swarm scheme** (milestones `# M#`, phases
`## M#.P#`, stages `### M#.P#.S#`, todos `- **`M#.P#.S#.T#`**` with an *Artifact:*
check + *Concern:* tag, and `## M#.P9` gate sections). The revoy narrative and its
`<!-- revoy:begin -->` TOML phase block are **removed** from `ROADMAP.md`.

- The genuine remaining work from the revoy roadmap was **mined, not discarded**: the
  three near-term revoy todos (verify the licence JWT signature, cache ffprobe
  results, clean Creator clippy warnings) are M1 todos; the Creator Edition phases
  CE-P1…CE-Capstone are decomposed into milestones M2–M5; 1.0 stabilization is M6.
- `VISION.md` now holds the north-star/identity narrative that lived as prose in the
  old ROADMAP.
- The project **remains enrolled in revoy's registry** (code `STR`) for cross-project
  changelog and audit purposes; only the *phase ledger* moves to the swarm scheme.
  revoy's generated outputs (`revoy.md`, `DEFERRALS.md`) and the central ledger are
  untouched.

## Consequences

- The swarm can parse, slice, claim, and verify roadmap work; `next-buildable.sh`
  reports M1 as `BUILDABLE`, so the first tick has real, immediately-buildable work.
- `revoy audit STR` will no longer find a `<!-- revoy:begin -->` phase block in
  `ROADMAP.md`. This is the deliberate, opinionated change this ADR authorizes; the
  swarm ROADMAP + `.agents/ledger.jsonl` are now the phase source of truth for this
  repo. If revoy portfolio reporting must be reconciled, do it by teaching revoy to
  read the swarm ledger — not by re-adding the TOML block.
- This is reversible: the decision is recorded here and the change lives on the
  `integration` branch until an audit-pass fast-forwards the default branch.
- Any future move back to a revoy-managed phase block requires a superseding ADR.
