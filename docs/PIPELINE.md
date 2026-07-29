# Creator pipeline architecture

The Creator Edition pipeline is a daemon-owned orchestration kernel. Plugins
implement capabilities; they do not own queues, retries, concurrency policy, or
durability.

## Execution contract

An executable stage contains:

- a typed `StageKind` for display and policy;
- a `StageDispatch` target (`plugin`, `verb`, recording selection, JSON payload);
- dependency stage IDs;
- resource locks;
- retry and cost policy;
- structured artifact descriptors.

The plugin registry resolves the dispatch target through
`Plugin::execute_stage`. A missing, disabled, or unhealthy executor is a
permanent and visible stage failure.

## Scheduler guarantees

- DAGs are rejected when cyclic, empty, duplicated, or dangling.
- Ready stages are discovered from dependency state and may fan out in parallel.
- GPU, provider, and file locks coordinate work across every active pipeline.
- Failures retry with bounded exponential backoff.
- Cancellation propagates through each stage's cancellation token.
- Every transition is persisted atomically to
  `~/.local/share/strivo/pipelines.json`.
- A daemon restart converts interrupted `Running` stages back to `Pending`.
- Equivalent active workflows for the same subject are coalesced.
- `PipelineUpdated` events stream complete snapshots to the Pro UI over SSE.

Stage executors must be idempotent for a `(pipeline, stage, subject)` because
crash recovery may invoke them again.

## Product surface

The Pipelines page separates two concepts:

1. **Executable workflows** — honest, daemon-backed runs with live state,
   cancellation, retry, history, and errors.
2. **Blueprints** — the roadmap capability topology. A blueprint is not
   presented as runnable until every required capability has an executor.

The production workflow is **Ultimate creator publish**:

`recording → transcript intelligence + visual cuepoints`

The transcript branch fans out to chapters, SRT/VTT/text captions, and
brand-safety analysis. The visual branch scores highlight moments, cuts the top
clips, and generates thumbnail candidates. Both branches converge into
platform-specific reuse drafts and a Casebook editorial report. The run detail
surface lists each stage and provides authenticated downloads for every
artifact, confined to the Creator artifact root.

Crunchr still implements its internal extraction/transcription/diarization/
analysis milestones as one idempotent capability stage. Splitting those
milestones is the remaining granularity improvement; it no longer blocks the
end-to-end publish workflow.

## Architectural trajectory

The DAG should grow by adding capability adapters, not special cases:

- a canonical append-only signal store becomes the artifact exchange;
- stages consume artifact descriptors rather than another plugin's SQLite DB;
- live extractors write incremental artifacts during capture;
- domain templates compose the same capabilities for Creator and Sports use;
- a future third-party plugin may participate only after declaring and
  implementing an executable capability contract.

This preserves a single control plane while allowing extraction and analytics
implementations to evolve independently.
