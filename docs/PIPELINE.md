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
- Cancellation settles the run immediately and prevents late worker results
  from resurrecting it. A blocking media subprocess already in flight drains
  while retaining its resource locks; work still waiting for a lock exits
  immediately.
- Every transition is persisted atomically to
  `~/.local/share/strivo/pipelines.json`.
- A daemon restart converts interrupted `Running` stages back to `Pending`.
- Equivalent active workflows for the same subject are coalesced.
- `PipelineUpdated` events stream complete snapshots to the Pro UI over SSE.

Stage executors must be idempotent for a `(pipeline, stage, subject)` because
crash recovery may invoke them again.

## Authoring and execution: one model

A graph authored in the browser **is** the run. There is no second pipeline
model, no second `PipelineId`, and no translation layer between the graph and
the executor:

- `GET /api/v1/pipelines/templates` returns every template — shipped and
  authored — in the exact shape a save takes back, as a real `Pipeline`.
- `POST /api/v1/pipelines/runs` accepts that same value as `graph` and submits
  it to the daemon **verbatim**. The reported `pipeline_id` is read off the
  value that is sent, so the reported id and the executed id are the same
  expression.
- The executor's registry holds the submitted `Pipeline` under the id minted
  at authoring and persists it under that same id to `pipelines.json`. The id
  is therefore identical at all three points: author → executor → history.
- `GET /api/v1/pipelines/runs` lists the run, and the `graph` key on the same
  response draws it, one node per `Stage` and one edge per `inputs` link —
  derived, never tabulated.

Editing the graph after submitting does not affect the run: submission takes a
snapshot, so a stage added or re-locked afterwards is not a stage of the run
already in flight.

### Identity and re-running

`PipelineId` is minted by the author and never re-minted by a layer. Two
consequences, both deliberate:

- Submitting the same graph twice is refused — `pipeline id already exists`.
  History is keyed by `PipelineId`, so a silent second record under a fresh id
  would be indistinguishable from a real second run.
- Re-running is the author's call: mint a new id, and the executor coalesces it
  against any **equivalent active** workflow (same name and `subject_id`) until
  that one reaches a terminal state.

### What a template's `selection` means when it executes

The shipped templates are rendered for the graph view with the **nil uuid** as
their subject, so a graph exists on a fresh install before any run. That nil
value is a *rendering* placeholder, not a runtime value.

- The **named-template path** — `POST /api/v1/pipelines/runs` with
  `recording_id` and `template` — is where the server binds a real recording
  into `subject_id` and every `dispatch.selection`.
- The **authored-graph path** executes the graph exactly as sent. Its
  `selection` is the author's own value, because only the author knows which
  recordings a run is over. Nothing is substituted at run time, because a
  silent substitution is precisely the translation this design removes.

### Validation, and where the backstop is

Seven rules gate a graph: six structural rules in
`PipelineRegistry::validation_errors` (acyclic, non-empty, unique stage ids,
no dangling inputs, non-zero capacities, non-empty dispatch target) and a
seventh the executor **structurally cannot express** —
`unknown-dispatch-target`, a dispatch naming a plugin this build does not
register. `StageKind::Custom` carries an opaque identifier, so the executor's
`validate` checks only that the dispatch strings are non-empty, and it has no
plugin registry to resolve a name against.

All three layers are live, and each answers a different caller:

| Layer | Where | Role |
|---|---|---|
| Browser | `assets/spa/047-creator.js` | inline errors before a save |
| Route | `pipeline_templates::graph_errors` | every rule, before the file is touched and before the IPC write |
| Executor | `PipelineRegistry::validate` | last line for a persisted or out-of-band pipeline |

`graph_errors` is one function on purpose: "valid" must not have two answers.
The run route's rule-7 check is a backstop the executor's cannot be — with no
editor in front of the caller, a graph dispatching to an unregistered plugin
is refused before it reaches the executor rather than failing hours later, at
dispatch, on a real recording.

## Product surface

The Pipelines page separates two concepts, and they are deliberately different
models:

1. **Authored graphs and runs** — real `Pipeline` values, daemon-backed, with
   live state, cancellation, retry, history, and errors. This is the runnable
   surface: a graph authored here is submitted as-is.
2. **Blueprints** — `GET /api/v1/pipelines/dag`, the roadmap capability
   topology from `crates/pipelines-dag`. Not a runnable workflow. A blueprint is
   not presented as runnable unless every capability it needs has a registered
   provider **and** its own edges resolve; one that fails its own `validate`
   reports `invalid` in the payload and is not runnable.

The blueprint model is **not** a parallel source of truth for execution. It is
read at exactly one place — `api.rs::dag_payload` — and its readers are:

- `api.rs::dag_payload` → `GET /api/v1/pipelines/dag`.
- The SPA accessor `API.pipelinesDag` (`assets/spa/007-creator.js:121`), which
  is **defined but never called**: no client reads `/pipelines/dag` today.
- `RecipeChain` is read only by the three `/api/v1/pipelines/chains` handlers.
  `API.pipelinesChains` does not exist in the SPA, so those routes have no
  client reader either.

Nothing in the executor reads `crates/pipelines-dag`, and nothing in the
authoring surface does. Todo 56 owns retiring `/api/v1/pipelines/dag`,
`API.pipelinesDag`, the `RecipeChain` type and its routes; they remain until
then as todo 6's delivered surface, and the list above is the record of who
reads them.

The production workflow is **Ultimate creator preparation** — it ends at a
rendered artifact and a draft, and publishing that artifact is a separate,
explicit step (see `crates/research/src/creator/publication.rs`):

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
