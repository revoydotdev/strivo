# StriVo Roadmap (swarm scheme)

> **Authority:** this file + `VISION.md` + `docs/adr/`. Governed by the swarm
> harness (ADR-0028); the prior revoy-format ROADMAP was superseded by
> [ADR-0005](docs/adr/ADR-0005-supersede-revoy-roadmap.md).
>
> **Format:** milestones `# M#`, phases `## M#.P#`, stages `### M#.P#.S#`, todos
> `- **`M#.P#.S#.T#`**`, gates `- **M#G#**`. Each todo carries an *Artifact:* — a
> single command that must exit 0 to prove it done — and a *Concern:* tag so a tick
> can pick disjoint work. Each milestone closes with a `## M#.P9` gate section.
>
> **Definition of done (non-negotiable, per VISION AX-3):** a todo is done only
> when its Artifact check passes on the integrated tree. A pure-data crate with
> tests is necessary but not sufficient — wiring separates 🟡 from ✅.
>
> **Current state (v0.5.0 alpha):** PVR core is solid and wired end-to-end
> (web-only frontend, recording/finalization pipeline, live detection, daemon↔SPA
> IPC, SQLite jobs journal, notifications). The Creator Edition toolkit (~34 in-tree
> crates) is built but only partly wired — the daemon does not yet drive the DAG
> executor and per-plugin SQLite is fragmented. Milestones below run current → 1.0.0.

---

# M1 — PVR hardening & security correctness (v0.5.x → 0.6.0)

The product comes first. Close the near-term correctness, security, and code-health
gaps on the shipped PVR before extending the Creator engine. Todos here are
immediately buildable against existing files.

## M1.P1 — Licence & credential trust

### M1.P1.S1 — Verify the licence JWT signature
The licence route currently trusts the backend JWT payload without checking its
ES256 signature (`crates/strivo-web/src/routes/licence.rs:245`,
`TODO(licence-verify)`), relying only on the machine-hash binding and a 72h refresh
window. Verify before trust (VISION AX-7).

- **`M1.P1.S1.T1`** — Embed the backend's P-256 public key and verify the ES256 signature of the licence JWT in `routes/licence.rs` **before** constructing `Licence`; reject a bad signature with a typed `Problem`. → *Artifact:* `cargo test -p strivo-web --features creator licence_verify` · *Concern:* licence-verify
- **`M1.P1.S1.T2`** — On verified tokens, reject when `machine_hash` ≠ local machine id or `expires_at` is in the past — as explicit verification failures, not a silent tier fallback. → *Artifact:* `cargo test -p strivo-web --features creator licence_reject` · *Concern:* licence-verify
- **`M1.P1.S1.T3`** — Remove the `TODO(licence-verify)` marker and the "we rely on the machine_hash binding" comment once verification is the real gate. → *Artifact:* `bash -c '! git grep -qn "TODO(licence-verify)" -- crates/strivo-web'` · *Concern:* licence-verify

## M1.P2 — Capture-path performance & correctness

### M1.P2.S1 — Cache ffprobe results
`recording_probe` (`crates/strivo-web/src/routes/api.rs:135`) shells out to
`ffprobe` on **every** `/probe` call, re-analysing unchanged files.

- **`M1.P2.S1.T1`** — Put an in-process cache keyed by `(path, mtime, size)` in front of the `recording_probe` ffprobe subprocess; a repeat probe of an unchanged file returns the cached result, and a changed mtime or size invalidates it. → *Artifact:* `cargo test -p strivo-web ffprobe_cache` · *Concern:* ffprobe-cache

## M1.P3 — Creator code health

### M1.P3.S1 — Clean the Creator clippy warnings
~44 clippy warnings across the Creator tool crates; the PVR build is clean but the
Creator surface is not gated.

- **`M1.P3.S1.T1`** — Resolve the outstanding Creator-crate clippy warnings so a strict clippy run under the feature is clean. → *Artifact:* `cargo clippy --workspace --features creator --all-targets -- -D warnings` · *Concern:* clippy-creator

### M1.P3.S2 — Single canonical viewguard data path
`viewguard`'s `data_dir` double-nests, so the web layer probes two candidate paths
as a workaround — a violation of "one canonical source" (VISION AX-6).

- **`M1.P3.S2.T1`** — Fix the `viewguard` `data_dir` resolution so the plugin and the web layer agree on one path, and drop the two-path probe workaround in the web route. → *Artifact:* `cargo test -p strivo-web --features creator viewguard_data_path` · *Concern:* viewguard-path

## M1.P9 — Milestone quality gates

- **M1G1** — PVR (default) build + tests green. → *Check:* `cargo test`
- **M1G2** — Creator build + tests green. → *Check:* `cargo test --workspace --features creator`
- **M1G3** — strict clippy clean under Creator. → *Check:* `cargo clippy --workspace --features creator --all-targets -- -D warnings`
- **M1G4** — no licence-verify TODO remains anywhere. → *Check:* `bash -c '! git grep -qn "TODO(licence-verify)"'`

---

# M2 — Creator signal spine & pipeline executor (CE-P1, CE-P3)

Give the Creator engine its two load-bearing foundations: one canonical signal store
(VISION AX-6) and a daemon that actually drives the DAG executor (VISION AX-3). Both
are Creator-gated; the PVR build stays untouched.

## M2.P1 — Unified signal store (CE-P1)

### M2.P1.S1 — Schema, write API, query API
Replace fragmented per-plugin SQLite with one append-only signal store:
`(recording_id, t_start, t_end, kind, label, payload JSON, confidence, source_plugin)`.

- **`M2.P1.S1.T1`** — Define the canonical signal schema + migration behind a typed store module. → *Artifact:* `cargo test --workspace --features creator signal_store_schema` · *Concern:* signal-store
- **`M2.P1.S1.T2`** — Plugin **write** API: a typed `write_signals` entrypoint extractors call, enforcing confidence + `source_plugin` provenance. → *Artifact:* `cargo test --workspace --features creator signal_store_write` · *Concern:* signal-store
- **`M2.P1.S1.T3`** — Analytic **query** API: range/kind/recording queries every analytic reads through. → *Artifact:* `cargo test --workspace --features creator signal_store_query` · *Concern:* signal-store

### M2.P1.S2 — Retire per-plugin SQLite reach-ins
- **`M2.P1.S2.T1`** — Migrate `insights` off its hardcoded `crunchr.db` reach-in to the query API. → *Artifact:* `bash -c 'cargo test --workspace --features creator insights_via_signal_store && ! git grep -qn "crunchr.db" -- crates/strivo-plugins/src/insights'` · *Concern:* insights-migrate

## M2.P2 — Drive the DAG executor from the daemon (CE-P3)

The `pipelines-dag` + `src/pipeline/` model/executor is complete and tested, but the
daemon never drives it — the highest-leverage Creator gap.

- **`M2.P2.S1.T1`** — Handle `PluginAction::SubmitPipeline` → `PipelineRegistry::submit`, dispatching ready stages to plugin verbs. → *Artifact:* `cargo test --workspace --features creator pipeline_submit_dispatch` · *Concern:* pipeline-exec
- **`M2.P2.S1.T2`** — `mark_stage_done`/`mark_stage_failed` advance the DAG, honouring the `ResourceLock` and the `max_attempts`/backoff the model encodes. → *Artifact:* `cargo test --workspace --features creator pipeline_advance_backoff` · *Concern:* pipeline-exec
- **`M2.P2.S1.T3`** — Emit live `StageState` transitions over SSE so the SPA reflects pipeline progress. → *Artifact:* `cargo test --workspace --features creator pipeline_sse` · *Concern:* pipeline-sse

## M2.P9 — Milestone quality gates

- **M2G1** — signal store round-trips (write → query). → *Check:* `cargo test --workspace --features creator signal_store`
- **M2G2** — no plugin reaches into a sibling plugin's SQLite. → *Check:* `bash -c '! git grep -qn "crunchr.db" -- crates/strivo-plugins/src'`
- **M2G3** — a submitted pipeline runs to completion driven by the daemon. → *Check:* `cargo test --workspace --features creator pipeline_submit_dispatch pipeline_advance_backoff`
- **M2G4** — workspace+creator green & strict clippy clean. → *Check:* `bash -c 'cargo test --workspace --features creator && cargo clippy --workspace --features creator --all-targets -- -D warnings'`

---

# M3 — Corpus service & extraction adapters (CE-P2, CE-P4)

Move corpus assembly server-side and give extraction a domain-agnostic contract that
writes into the M2 signal store.

## M3.P1 — Server-side corpus assembly (CE-P2)

### M3.P1.S1 — Hydrate corpora behind an endpoint
- **`M3.P1.S1.T1`** — `hydrate_corpus` builds a `dataviz::Corpus` by `recording | playlist | channel + date-range` from the signal store, behind an HTTP endpoint. → *Artifact:* `cargo test --workspace --features creator corpus_hydrate` · *Concern:* corpus-service
- **`M3.P1.S1.T2`** — The SPA consumes the endpoint instead of hand-assembling the corpus client-side. → *Artifact:* `cargo test --workspace --features creator corpus_endpoint_route` · *Concern:* corpus-web

## M3.P2 — Extraction adapters (CE-P4)

### M3.P2.S1 — Common extractor contract
- **`M3.P2.S1.T1`** — Define an `Extractor` trait that writes into the signal store with per-extractor confidence + provenance. → *Artifact:* `cargo test --workspace --features creator extractor_contract` · *Concern:* extractor-contract
- **`M3.P2.S1.T2`** — Back-pressure: a bounded extraction queue so extraction keeps up with capture without unbounded growth. → *Artifact:* `cargo test --workspace --features creator extractor_backpressure` · *Concern:* extractor-contract

### M3.P2.S2 — New domain-agnostic extractors
- **`M3.P2.S2.T1`** — Timecoded entity/event extractor (the sports spine) writing typed events into the store. → *Artifact:* `cargo test --workspace --features creator extractor_events` · *Concern:* extractor-events
- **`M3.P2.S2.T2`** — Visual/OCR extractor (scoreboards, lower-thirds) with confidence + provenance. → *Artifact:* `cargo test --workspace --features creator extractor_ocr` · *Concern:* extractor-ocr

## M3.P9 — Milestone quality gates

- **M3G1** — corpus hydrates server-side from the signal store. → *Check:* `cargo test --workspace --features creator corpus_hydrate`
- **M3G2** — every extractor writes through the store contract (confidence + provenance present). → *Check:* `cargo test --workspace --features creator extractor_contract`
- **M3G3** — workspace+creator green & strict clippy clean. → *Check:* `bash -c 'cargo test --workspace --features creator && cargo clippy --workspace --features creator --all-targets -- -D warnings'`

---

# M4 — Analytics, visualisation & clip/export (CE-P5, CE-P6, CE-P7)

Turn stored signals into analysis, visuals, and shippable clips.

## M4.P1 — Analytics over real corpora (CE-P5)
- **`M4.P1.S1.T1`** — Experiment registry over `dataviz`, run against a hydrated corpus. → *Artifact:* `cargo test --workspace --features creator experiment_registry` · *Concern:* analytics
- **`M4.P1.S1.T2`** — Cross-signal experiment (transcript × events × chat) joined through the store. → *Artifact:* `cargo test --workspace --features creator experiment_cross_signal` · *Concern:* analytics
- **`M4.P1.S1.T3`** — Incremental/streaming aggregation over SSE as signals arrive. → *Artifact:* `cargo test --workspace --features creator experiment_incremental` · *Concern:* analytics-sse

## M4.P2 — Visualisation & composer UI (CE-P6)
- **`M4.P2.S1.T1`** — A general composer: pick corpus → pick experiment → render via `chart_hint` (not per-plugin pages). → *Artifact:* `cargo test --workspace --features creator composer_route` · *Concern:* composer
- **`M4.P2.S1.T2`** — Export a rendered view to CSV/JSON/PNG. → *Artifact:* `cargo test --workspace --features creator composer_export` · *Concern:* composer-export

## M4.P3 — Clip & export pipeline (CE-P7)
- **`M4.P3.S1.T1`** — Wire `clipper` + `captions` into `finalize_completion` and the M2 DAG so *extract → select highlights → cut → caption → export* is one chain. → *Artifact:* `cargo test --workspace --features creator clip_export_chain` · *Concern:* clip-export

## M4.P9 — Milestone quality gates

- **M4G1** — an experiment runs against a hydrated corpus and produces a chartable result. → *Check:* `cargo test --workspace --features creator experiment_registry`
- **M4G2** — the composer renders + exports a view end-to-end. → *Check:* `cargo test --workspace --features creator composer_export`
- **M4G3** — the clip chain produces a captioned clip from stored signals. → *Check:* `cargo test --workspace --features creator clip_export_chain`
- **M4G4** — workspace+creator green & strict clippy clean. → *Check:* `bash -c 'cargo test --workspace --features creator && cargo clippy --workspace --features creator --all-targets -- -D warnings'`

---

# M5 — Real-time extraction & domain templates (CE-P8, CE-Capstone)

The headline promise: analysis "as fast as it is recorded," plus the two shipping
domain templates that prove the engine is domain-agnostic.

## M5.P1 — Real-time incremental extraction (CE-P8)
- **`M5.P1.S1.T1`** — Extractors tail the live capture segment and write signals **during** recording, not only after finalize. → *Artifact:* `cargo test --workspace --features creator realtime_tail_extract` · *Concern:* realtime
- **`M5.P1.S1.T2`** — Analytics + visualisation update live over SSE as in-capture signals arrive. → *Artifact:* `cargo test --workspace --features creator realtime_live_update` · *Concern:* realtime-sse

## M5.P2 — Domain templates (CE-Capstone)
- **`M5.P2.S1.T1`** — Ship a **Sports** template (event taxonomy + box-score rollups) as config over the domain-agnostic core, not new code. → *Artifact:* `cargo test --workspace --features creator template_sports` · *Concern:* template-sports
- **`M5.P2.S1.T2`** — Ship a **Creator** template (highlight/retention rollups + publish-ready clips) as config. → *Artifact:* `cargo test --workspace --features creator template_creator` · *Concern:* template-creator

## M5.P9 — Milestone quality gates

- **M5G1** — signals appear for an in-flight capture before finalize. → *Check:* `cargo test --workspace --features creator realtime_tail_extract`
- **M5G2** — both domain templates load and drive the engine from config alone. → *Check:* `cargo test --workspace --features creator template_sports template_creator`
- **M5G3** — workspace+creator green & strict clippy clean. → *Check:* `bash -c 'cargo test --workspace --features creator && cargo clippy --workspace --features creator --all-targets -- -D warnings'`

---

# M6 — 1.0.0 stabilization & release

Freeze the contracts (VISION AX-8), close the platform gap, and cut 1.0.0.

## M6.P1 — Freeze the contracts
- **`M6.P1.S1.T1`** — Stabilize and version the config schema; document it and add a migration path so upgrades no longer require hand-editing `config.toml`. → *Artifact:* `cargo test config_schema_stable` · *Concern:* freeze-config
- **`M6.P1.S1.T2`** — Assert and document the daemon IPC protocol version; add a cross-version compatibility test around `IPC_PROTOCOL_VERSION`. → *Artifact:* `cargo test ipc_version_compat` · *Concern:* freeze-ipc
- **`M6.P1.S1.T3`** — Document and pin the plugin ABI contract; the manifest declares the ABI version the loader enforces. → *Artifact:* `cargo test --workspace --features creator plugin_abi_version` · *Concern:* freeze-abi

## M6.P2 — Windows daemon transport
- **`M6.P2.S1.T1`** — Add the Windows named-pipe transport behind the IPC abstraction (or, if deferred, record the deferral in an ADR). → *Artifact:* `bash -c 'cargo test ipc_transport_abstraction || test -f docs/adr/ADR-0006-defer-windows-transport.md'` · *Concern:* windows-transport

## M6.P3 — Release
- **`M6.P3.S1.T1`** — Reconcile `README.md`, `DESIGN.md`, and `CHANGELOG.md` with shipped reality; no aspirational claim outruns the code. → *Artifact:* `bash -c 'grep -q "1.0.0" CHANGELOG.md'` · *Concern:* release-docs
- **`M6.P3.S1.T2`** — Bump workspace version to `1.0.0` and tag the release. → *Artifact:* `bash -c 'grep -q "^version = \"1.0.0\"" Cargo.toml || git tag | grep -qx v1.0.0'` · *Concern:* release-cut

## M6.P9 — Milestone quality gates

- **M6G1** — config/IPC/ABI contracts are versioned and tested. → *Check:* `bash -c 'cargo test config_schema_stable ipc_version_compat && cargo test --workspace --features creator plugin_abi_version'`
- **M6G2** — Windows transport landed or ADR-deferred. → *Check:* `bash -c 'cargo test ipc_transport_abstraction || test -f docs/adr/ADR-0006-defer-windows-transport.md'`
- **M6G3** — version is 1.0.0 and docs match. → *Check:* `bash -c 'grep -q "^version = \"1.0.0\"" Cargo.toml && grep -q "1.0.0" CHANGELOG.md'`
- **M6G4** — full workspace+creator green & strict clippy clean. → *Check:* `bash -c 'cargo test --workspace --features creator && cargo clippy --workspace --features creator --all-targets -- -D warnings'`

---

## Conventions

- Commit prefixes: `feat:` `fix:` `chore:` `refactor:` `ci:` `docs:` `test:` `perf:`.
- **No AI attribution** in commits, PRs, or code comments (per project CLAUDE.md).
- **Editions:** default build = PVR; `--features creator` = Creator Edition. Keep the
  PVR build free of creator deps; gate new creator surfaces behind the feature.
- A PVR slice is: change + tests + daemon/web wiring + SPA surface + E2E verify. A
  Creator slice adds a signal-store/contract change (where relevant) + the pure-data
  crate + capability/marketplace registration. The wiring step separates 🟡 from ✅.
