# Research data architecture

Phase 1 establishes `strivo-research` as the canonical qualitative evidence
kernel. It is local-first and stored at
`<data-dir>/research/research.db`.

## Canonical records

- Projects scope all research work.
- Sources represent recordings, video, audio, chat, documents, or datasets.
- Cases group sources using researcher-defined attributes.
- Codes form project-local hierarchies.
- Codings are human, imported, or model-proposed time-ranged annotations.
- Memos attach reflexive notes to sources or codings.
- Relationships link research objects without changing their identity.
- Signals are append-only normalized machine observations.
- Provenance records producer, version, method, model, parameters, and digest.

Time is stored as non-negative source-relative milliseconds. Generated findings
must reference a source and may reference immutable provenance. UUIDs are stable
across export; legacy adapter IDs are deterministic UUIDv5 values, so migration
is safe to retry.

## Storage contract

SQLite uses WAL, normal synchronization, foreign keys, a 30-second busy timeout,
and indexes for project, source/time, kind, code, and relationship queries.
Mutable researcher work remains authoritative here. Phase 2 may project
immutable signals into Parquet and DuckDB, but those tiers must preserve IDs and
produce results identical to this store.

Portable JSON export is deliberately bounded to 1,000 signals. Large project
export becomes a streamed bundle in Phase 2; the API refuses to silently
truncate.

## Legacy reconciliation

The migration adapters currently normalize:

| Legacy store | Canonical signal |
| --- | --- |
| Crunchr `segments` | `transcript.utterance` |
| Cuepoints sets | `visual.scene_change` |
| Viewguard detector output | `audience.anomaly` |

Each report returns examined, inserted, and skipped counts plus a SHA-256 digest
of examined source rows. Re-running an unchanged migration produces zero new
signals and the same digest. Existing nested Viewguard databases remain readable
for migration, while new writes use the corrected plugin-scoped directory.

## Creator API

All routes require the existing API-key or browser-session authentication.
Mutations also pass the global CSRF guard.

```text
GET|POST /api/v1/research/projects
GET      /api/v1/research/projects/{id}
POST     /api/v1/research/projects/{id}/sources
POST     /api/v1/research/projects/{id}/cases
POST     /api/v1/research/projects/{id}/codes
POST     /api/v1/research/projects/{id}/codings
POST     /api/v1/research/projects/{id}/memos
POST     /api/v1/research/projects/{id}/relationships
GET      /api/v1/research/projects/{id}/signals
POST     /api/v1/research/projects/{id}/migrate/crunchr
POST     /api/v1/research/projects/{id}/migrate/legacy
```

SQLite work runs on Tokio's blocking pool. Signal queries are paginated and
capped at 1,000 records per request.

## Adapter rule

New extractors write through `strivo-research`; they must not introduce another
cross-plugin read dependency. During transition, private legacy databases may
continue serving their owning plugin, but research and cross-signal analysis use
the canonical spine.
