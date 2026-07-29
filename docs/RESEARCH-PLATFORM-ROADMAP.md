# Strivo Research Platform roadmap

## Product thesis

Strivo Pro becomes a stream-native qualitative and mixed-methods research
platform: ingest live and archived multimodal evidence, transcribe it, code it,
query it at corpus scale, visualize relationships, and preserve a reproducible
chain from every finding back to playable evidence.

This is one platform, not a collection of AI widgets. The recording system is
the acquisition layer; the DAG is the reproducible compute layer; the research
kernel is the evidence layer; analysis and visualization are projections over
that evidence.

## Architectural destination

```text
capture/import
  video · audio · chat · metrics · documents · external datasets
                              │
                              ▼
extract and normalize
  transcript · words · speakers · entities · OCR · scenes · events
                              │
                              ▼
canonical research kernel
  projects · sources · cases · attributes · codes · codings · memos
  append-only signals · provenance · revisions · evidence links
                              │
                 ┌────────────┴────────────┐
                 ▼                         ▼
portable control plane              analytical plane
SQLite                              Arrow · Parquet · DuckDB
                 │                         │
                 └────────────┬────────────┘
                              ▼
hybrid retrieval and analysis
  lexical · semantic · temporal · SQL · statistics · model-assisted coding
                              │
                              ▼
linked research workspaces
  Corpus Explorer · Coding Studio · Query Lab · Evidence Canvas
                              │
                              ▼
reproducible exports
  reports · clips · figures · CSV/JSON/Parquet · project bundles
```

Stable UUIDs cross every storage tier. SQLite remains authoritative for mutable
human work. High-volume immutable observations are partitioned by project,
source, signal kind, and time. Every generated object carries provenance.

## Engineering contract

These rules apply to every phase:

- Test-first loop: add a failing contract, migration, integration, or browser
  test; implement the smallest coherent slice; refactor only while green.
- Rust: stable toolchain, `rustfmt`, no Clippy warnings, typed errors, no
  unchecked panics in production paths, parameterized SQL, documented public
  contracts, bounded concurrency, cancellation, and deterministic ordering.
- JavaScript: syntax check, no implicit globals, escaped untrusted HTML,
  teardown for listeners/timers, keyboard operation, and route-level loading,
  empty, partial, failure, and retry states.
- Schemas: monotonic numbered migrations, forward and rollback rehearsal,
  foreign keys enabled, indexes justified by query plans, fixtures from the
  previous version, and no destructive migration without an export.
- DAG stages: idempotent, content-addressed inputs, declared resources,
  structured outputs, typed failure class, retry policy, cancellation, and
  complete provenance.
- Privacy: local-first defaults, explicit external-provider consent, secret
  redaction, retention controls, export/delete, audit events, and documented
  model/data destinations.
- Accessibility: WCAG 2.2 AA target, full keyboard paths, visible focus,
  semantic names, non-color state indicators, reduced motion, and screen-reader
  announcements for long-running analysis.
- Performance: representative large-corpus fixtures and budgets tracked in CI;
  paginate/stream instead of materializing unbounded collections.
- Release gate:

  ```sh
  cargo fmt --all -- --check
  cargo check --workspace --all-targets --all-features --locked
  cargo test --workspace --all-features --locked
  cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
  node --check crates/strivo-web/assets/spa.js
  ```

  Changed user journeys also require Playwright coverage, migration fixtures,
  a clean production build, security review, changelog entry, and documentation.

## Phase 0 — research foundations and decision records

Deliver:

- Research terminology, personas, workflows, threat model, and data classes.
- ADRs for SQLite/Parquet/DuckDB/Arrow boundaries, identifiers, time units,
  provenance, transcript revisions, and plugin compatibility.
- Synthetic benchmark corpora at 10 hours, 1,000 hours, and 10,000 hours.
- Product telemetry that measures latency and reliability without collecting
  corpus content.

Quality gate:

- Every architectural choice has alternatives and reversal cost.
- Synthetic fixtures contain no personal data and are reproducible.
- Baseline ingest, query, memory, storage, and first-render numbers recorded.

## Phase 1 — canonical research kernel and signal spine

Deliver:

- Versioned projects, multimodal sources, cases, typed attributes, hierarchical
  codebooks, time-ranged codings, annotations, memos, and relationships.
- Append-only normalized signals with confidence and immutable provenance.
- Stable millisecond time coordinates and source-relative evidence links.
- SQLite WAL store, constraints, indexes, import adapters, backup/export, and
  additive migrations from Crunchr, chat, cuepoint, and Viewguard stores.
- Transactional plugin write/query APIs; no plugin reaches into another
  plugin's private database.

Quality gate:

- Schema invariants, foreign-key failure paths, idempotent open/migrate,
  append-only behavior, cross-project isolation, round-trip export, and
  concurrent reader/writer tests.
- Migration dry-run reports counts/checksums; migrated samples reconcile 100%.
- 1 million signals insert in bounded batches and indexed source/time queries
  meet the recorded local budget.

Status: **in progress**. `strivo-research` schema version 1 now implements the
first project/source/code/coding/provenance/signal contracts with tests.

## Phase 2 — corpus assembly and analytical storage

Deliver:

- Server-side corpus definitions by project, case, source, channel, platform,
  date, speaker, code, completeness, or saved query.
- Arrow record batches as the internal interchange.
- Partitioned Parquet observations and DuckDB query service with projection and
  predicate pushdown.
- Incremental compaction, schema evolution, orphan detection, checksums, and
  rebuild-from-source.
- Corpus manifest recording exact source and transcript revisions.

Quality gate:

- Golden-result parity between SQLite and DuckDB projections.
- Crash-safe compaction and corruption recovery tests.
- A 10,000-hour synthetic corpus opens without loading all observations into
  memory; filtered aggregations have explicit p50/p95 budgets.

## Phase 3 — transcription and speaker intelligence

Deliver:

- VAD, adaptive overlapping chunks, batching, checkpoint/resume, and partial
  retranscription.
- Local CPU/GPU, remote worker, and paid-provider adapters behind one contract.
- Word timestamps, confidence, punctuation, language identification,
  translation, and overlap reconciliation.
- Diarization, speaker merge/split, cross-recording voice identity with explicit
  consent, vocabulary/glossary, and correction memory.
- Immutable transcript revisions, diff, alignment after edits, and caption
  regeneration.

Quality gate:

- WER/DER benchmark suite by language, audio quality, overlap, and domain.
- Chunk-boundary loss, timestamp monotonicity, resume, provider fallback,
  cancellation, and low-confidence retranscription tests.
- No biometric embedding leaves the machine by default; deletion is verifiable.
- Accuracy regressions outside agreed tolerances block release.

## Phase 4 — multimodal extraction

Deliver:

- Chat, audience metrics, moderation actions, scenes, OCR/lower-thirds,
  scoreboard/event, entities, sentiment/stance, topics, audio events, and
  visual-object adapters.
- A typed extractor SDK with capability declarations, calibration metadata,
  batching, backpressure, and provenance.
- Human-review queues for uncertain or consequential classifications.

Quality gate:

- Labeled precision/recall fixtures and calibration curves per extractor.
- Time alignment across every modality within documented tolerances.
- Extractor failure cannot corrupt capture or canonical evidence.

## Phase 5 — Corpus Explorer and data stewardship

Deliver:

- Project dashboard, corpus browser, faceted filters, saved sets, source
  completeness, missing-data diagnostics, bulk metadata, cases, and attributes.
- Import wizard with mapping preview, duplicate detection, resumability, and
  reconciliation report.
- Retention, legal hold, redaction, pseudonymization, consent notes, and
  project-level access roles.

Quality gate:

- 100,000-source browsing remains paginated and keyboard usable.
- Imports are idempotent and partial failures are resumable.
- Permission matrix, export/delete, redaction, and audit-log security tests pass.

## Phase 6 — Coding Studio

Deliver:

- Synchronized player, transcript, chat, signals, waveform, and timeline.
- Hierarchical/overlapping codes, quick-code shortcuts, annotations, memos,
  bookmarks, cases, relationships, code definitions, and color-independent
  markers.
- Transcript correction and speaker management without losing evidence links.
- Reviewer assignments, blind coding, adjudication, consensus, and coding
  comparison.
- Model suggestions are proposals, never silent researcher-authored facts.

Quality gate:

- Frame/time-link accuracy and edit/revision rebasing tests.
- Undo/redo, autosave, offline interruption, conflict, and recovery tests.
- Cohen's kappa/Krippendorff alpha validated against published fixtures.
- Complete primary workflow passes keyboard and screen-reader audit.

## Phase 7 — retrieval and Query Lab

Deliver:

- Tantivy lexical index with Boolean, phrase, proximity, fuzzy, fielded, and
  concordance search.
- Embedding abstraction and local/remote vector indexes with model versioning.
- Hybrid lexical/semantic ranking, temporal joins, co-occurrence, negative
  filters, case/code joins, and saved parameterized queries.
- Query notebook cells for SQL, search, statistics, annotations, and narrative.
- Every hit opens the exact source moment and transcript revision.

Quality gate:

- Search relevance golden set with NDCG/recall thresholds.
- Index rebuild, incremental update, deletion, version mismatch, and corrupt
  shard tests.
- Query planner rejects unbounded dangerous work or requires explicit consent.
- Citations are complete and stable across reindexing.

## Phase 8 — mixed-method analytics

Deliver:

- Code frequency, coverage, matrix coding, co-occurrence, sequence, lag,
  speaker-time, conversation network, topic evolution, sentiment/stance,
  audience-response, moderation, and cross-stream comparison.
- Statistical summaries, uncertainty, effect sizes, missingness, weighting,
  cohort definitions, and multiple-comparison warnings.
- Experiment registry with versioned inputs, parameters, outputs, seeds, and
  incremental materializations.

Quality gate:

- Statistical functions match reference fixtures and expose assumptions.
- Deterministic runs reproduce identical manifests and results.
- Charts never imply unsupported causality; missing/uncertain data is visible.

## Phase 9 — Evidence Canvas and visualization grammar

Deliver:

- Linked timelines, heatmaps, code matrices, concordances, networks, Sankey
  flows, topic streams, distributions, retention overlays, and small multiples.
- Brushing/filtering across views, drill-through to evidence, annotations,
  responsive layout, saved dashboards, and presentation mode.
- Declarative visualization specification and auto-selection with researcher
  override.
- Accessible tables for every chart and color-safe palettes.

Quality gate:

- Golden visual snapshots, data-to-mark correctness, empty/extreme/large-value
  fixtures, resize and export parity.
- Interaction remains responsive at stated mark limits; larger views aggregate.
- SVG/PNG/CSV exports include title, units, filters, provenance, and timestamp.

## Phase 10 — collaboration, governance, and reproducibility

Deliver:

- Local single-user first; optional workspace service with roles, invitations,
  optimistic concurrency, comments, review queues, and immutable audit history.
- Signed project snapshots, dependency/model lockfiles, reproducible runs,
  lineage graph, and environment diagnostics.
- Encrypted backups, portable project bundles, restore rehearsal, and
  NVivo/ATLAS.ti/MAXQDA-friendly interchange where formats permit.

Quality gate:

- Conflict simulations never silently lose researcher work.
- Restore produces matching record counts, checksums, lineage, and permissions.
- Threat model and external security review cover tenancy and untrusted imports.

## Phase 11 — live research

Deliver:

- Tail active recordings and incrementally transcribe/extract/index them.
- Watermarked event time, late-event correction, live coding, live dashboards,
  alert rules, and post-stream reconciliation.
- Recording has absolute priority; research work sheds load under contention.

Quality gate:

- Soak tests with disconnects, clock drift, late/out-of-order events, restart,
  and constrained CPU/disk.
- Defined capture-loss SLO remains zero under supported load.
- Live and reconciled post-stream results converge.

## Phase 12 — domain packs, ecosystem, and release

Deliver:

- Creator, esports/sports, community safety, media studies, oral history, and
  podcast research templates composed from the same contracts.
- Signed extractor/analysis plugin SDK, compatibility matrix, sandbox boundary,
  sample datasets, and developer conformance suite.
- Onboarding studies, documentation, tutorials, transparent model cards,
  support diagnostics, packaging, update/rollback, and long-term migrations.

Quality gate:

- A domain pack contains no privileged product-only code path.
- Third-party conformance tests cover schema, provenance, cancellation,
  resources, determinism, and malicious payloads.
- Release candidate completes restore/upgrade/rollback, accessibility,
  security, performance, and user-acceptance gates on every supported platform.

## Cross-phase acceptance targets

| Concern | Target |
| --- | --- |
| Evidence traceability | 100% of generated findings resolve to source, time range, revision, producer, parameters, and input digest |
| Researcher work durability | no acknowledged coding/memo edit lost in crash or reconnect tests |
| Capture isolation | analytical load cannot cause supported recording loss |
| Large corpus | bounded-memory operation at the 10,000-hour fixture |
| Search | incremental results and cancellation; relevance threshold defined by a maintained golden set |
| Accessibility | WCAG 2.2 AA for all primary workflows |
| Reproducibility | identical locked inputs produce identical manifests; nondeterministic models record seed/provider response identity |
| Privacy | local processing and local indexes by default; outbound data is explicit and auditable |

## Immediate execution sequence

1. Complete schema-v1 query/list/export contracts and migration harness.
2. Add Crunchr transcript-to-signal adapter with checksum reconciliation.
3. Expose project, corpus, codebook, coding, and signal APIs behind Creator
   authentication and CSRF controls.
4. Build the first Corpus Explorer and synchronized coding vertical slice.
5. Introduce Arrow batches, Parquet projection, and DuckDB behind the same
   corpus contract only after the SQLite semantics are proven.
