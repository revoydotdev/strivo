# Architecture Decision Records

This directory holds StriVo's Architecture Decision Records (ADRs). Each ADR
captures one significant, durable decision — its **Context**, **Decision**, and
**Consequences** — so a future reader inherits the *why*, not just the *what*.
ADRs are governed by [VISION.md](../../VISION.md); a decision that changes an axiom
must say so.

Conventions (see [ADR-0001](ADR-0001-record-architecture-decisions.md)):

- Sequentially numbered, `ADR-NNNN-kebab-title.md`.
- Once **Accepted**, an ADR is immutable. To change a decision, write a new ADR
  that **supersedes** the old one and update both statuses.
- Statuses: `Proposed` · `Accepted` · `Superseded by ADR-NNNN` · `Deprecated`.

## Index

| ADR | Title | Status |
|-----|-------|--------|
| [0001](ADR-0001-record-architecture-decisions.md) | Record architecture decisions | Accepted |
| [0002](ADR-0002-web-only-frontend-over-headless-daemon.md) | Web-only frontend over a headless daemon | Accepted |
| [0003](ADR-0003-two-editions-one-codebase.md) | Two editions from one codebase via the `creator` feature | Accepted |
| [0004](ADR-0004-platform-trait-and-plugin-event-bus.md) | Platform trait + plugin event bus with a one-way dependency graph | Accepted |
| [0005](ADR-0005-supersede-revoy-roadmap.md) | Supersede the revoy-format ROADMAP with the swarm scheme | Accepted |
