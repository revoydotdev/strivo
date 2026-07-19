# ADR-0001 — Record architecture decisions

- **Status:** Accepted
- **Date:** 2026-07-19

## Context

StriVo carries several load-bearing decisions that are not obvious from the code
alone: why the TUI was removed, why there are two editions from one codebase, why
the dependency graph runs one way, why the licence path is shaped as it is. As the
project is now driven by an autonomous swarm harness (vendored under `.agents/`;
see [ADR-0005](ADR-0005-supersede-revoy-roadmap.md)), both human and agent
contributors need a durable, greppable record of *why* the
architecture is the way it is — otherwise each tick risks re-litigating settled
decisions or silently violating them.

## Decision

We keep Architecture Decision Records in `docs/adr/`, one file per decision, using a
lightweight Nygard-style template: **Context**, **Decision**, **Consequences**, a
clear title, a status, and a date. ADRs are sequentially numbered.

- An ADR is written for any significant, durable, or surprising architectural
  decision, and for any change that supersedes a prior ADR or a foreign governance
  artifact.
- Once **Accepted**, an ADR is immutable. A changed decision is a *new* ADR that
  supersedes the old one; the superseded ADR's status is updated to point forward.
- Trivial, local, easily-reversed choices do **not** get an ADR.
- Every ADR must be consistent with [VISION.md](../../VISION.md), or explicitly
  amend an axiom.

## Consequences

- Contributors (human and agent) can reconstruct intent without archaeology through
  git history or chat logs.
- There is a small, ongoing authoring cost; we accept it for decisions with
  long-lived consequences and skip it for the trivial.
- The `docs/adr/README.md` index must be kept current as ADRs are added.
