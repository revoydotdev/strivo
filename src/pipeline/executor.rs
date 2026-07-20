//! In-memory pipeline registry + resource-lock semaphore registry.
//!
//! The executor itself (dispatching ready stages, handling completions)
//! lives in plugin code today — each plugin owns the actual work and
//! reports results back via `PluginAction::TaskCompleted`. The registry
//! here is the cross-plugin coordination point: it holds Pipelines so the
//! UI (status bar, DAG overlay, `:batches` resource) and other plugins
//! can read state, and it owns the resource semaphores so a stage
//! requesting a `Gpu` lock blocks if another stage in another pipeline
//! holds it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Semaphore};

use super::stage::{Pipeline, PipelineId, PipelineState, ResourceLock, Stage, StageId, StageState};

/// One ready stage's dispatch target, computed by [`PipelineRegistry::dispatch_ready`].
/// Kept as plain data (rather than calling into the plugin registry
/// directly) so the submit+dispatch and advance logic stay testable
/// without a live `PluginRegistry` — the daemon turns each of these into
/// an actual `PluginRegistry::dispatch_verb` call, passing `stage_id` as
/// the verb's selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageDispatch {
    pub pipeline_id: PipelineId,
    pub stage_id: StageId,
    pub owner: String,
    pub verb: String,
}

/// Result of advancing the DAG after a stage reaches a terminal-ish
/// state (Done / Failed / Cancelled / Skipped). `retry_after` is set
/// when a `Failed` stage still has attempts left — the caller schedules
/// a delayed `retry_stage` + `dispatch_ready` after that backoff.
#[derive(Debug, Clone)]
pub struct StageAdvance {
    pub pipeline_id: Option<PipelineId>,
    pub dispatch: Vec<StageDispatch>,
    pub retry_after: Option<(StageId, Duration)>,
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Canonical string key a [`ResourceLock`] reserves against in
/// [`PipelineRegistry`]'s lock-holder bookkeeping.
fn lock_key(lock: &ResourceLock) -> String {
    match lock {
        ResourceLock::Gpu => "gpu".to_string(),
        ResourceLock::Api { name, .. } => format!("api:{name}"),
        ResourceLock::File { path } => format!("file:{path}"),
    }
}

/// How many stages may hold this lock concurrently.
fn lock_capacity(lock: &ResourceLock) -> usize {
    match lock {
        ResourceLock::Gpu => 1,
        ResourceLock::Api { cap, .. } => *cap,
        ResourceLock::File { .. } => 1,
    }
}

/// Shared registry of every Pipeline submitted this session. Cloned via
/// `Arc<Mutex<…>>` from `AppState` into anything that wants to read or
/// mutate pipeline state — plugin event handlers, the status bar
/// telemetry strip, the `:batches` palette resource, the DAG overlay.
#[derive(Default)]
pub struct PipelineRegistry {
    pipelines: HashMap<PipelineId, Pipeline>,
    /// Insertion order so the UI can list "newest first" without
    /// re-sorting on every render.
    order: Vec<PipelineId>,
    /// Currently-held resource locks, keyed by [`lock_key`] → the stage
    /// ids currently occupying a slot. Consulted by `dispatch_ready` so
    /// a stage whose lock is already at capacity (in this pipeline or
    /// another — locks are global) stays `Pending` until a holder
    /// terminates and frees it via `free_locks_for`.
    lock_holders: HashMap<String, Vec<StageId>>,
}

impl PipelineRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn submit(&mut self, pipeline: Pipeline) -> Result<PipelineId, &'static str> {
        pipeline.assert_acyclic()?;
        let id = pipeline.id;
        self.order.push(id);
        self.pipelines.insert(id, pipeline);
        Ok(id)
    }

    pub fn get(&self, id: PipelineId) -> Option<&Pipeline> {
        self.pipelines.get(&id)
    }

    pub fn get_mut(&mut self, id: PipelineId) -> Option<&mut Pipeline> {
        self.pipelines.get_mut(&id)
    }

    pub fn remove(&mut self, id: PipelineId) -> Option<Pipeline> {
        self.order.retain(|&i| i != id);
        self.pipelines.remove(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Pipeline> {
        self.order.iter().filter_map(|id| self.pipelines.get(id))
    }

    pub fn len(&self) -> usize {
        self.pipelines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pipelines.is_empty()
    }

    pub fn active_count(&self) -> usize {
        self.iter()
            .filter(|p| matches!(p.state, PipelineState::Running | PipelineState::Pending))
            .count()
    }

    /// Mark a stage Done by id. Returns the stage's pipeline id so the
    /// caller can decide what to do next (advance dependent stages,
    /// finalize the pipeline if all stages are terminal).
    pub fn mark_stage_done(&mut self, stage_id: StageId) -> Option<PipelineId> {
        for (pid, pipe) in &mut self.pipelines {
            if let Some(stage) = pipe.stages.iter_mut().find(|s| s.id == stage_id) {
                stage.state = StageState::Done;
                return Some(*pid);
            }
        }
        None
    }

    /// Manually reset a Failed / Exhausted / Cancelled stage so the
    /// executor will pick it up again on the next tick. Resets the
    /// state to `Pending` and re-arms the cancellation token. If
    /// `provider_override` is supplied and the stage carries a
    /// provider-bearing kind, the new provider replaces the old one
    /// for subsequent attempts. (C3 UI dispatcher.)
    pub fn retry_stage(
        &mut self,
        stage_id: StageId,
        provider_override: Option<String>,
    ) -> Option<PipelineId> {
        for (pid, pipe) in &mut self.pipelines {
            if let Some(stage) = pipe.stages.iter_mut().find(|s| s.id == stage_id) {
                stage.state = StageState::Pending;
                stage.cancel = tokio_util::sync::CancellationToken::new();
                if let Some(prov) = provider_override {
                    match &mut stage.kind {
                        super::stage::StageKind::Transcribe { provider }
                        | super::stage::StageKind::Diarize { provider }
                        | super::stage::StageKind::Analyze { provider } => {
                            *provider = prov;
                        }
                        _ => {}
                    }
                }
                // A pipeline that had any retryable stage flips back
                // to Running so the executor wakes; the executor
                // re-checks the post-condition once stages settle.
                if matches!(pipe.state, PipelineState::Failed) {
                    pipe.state = PipelineState::Running;
                }
                return Some(*pid);
            }
        }
        None
    }

    /// Mark a stage as `Skipped` so the executor walks past it without
    /// running. Downstream stages with this stage in their inputs
    /// proceed as if the skipped stage had completed. The caller is
    /// responsible for explaining "why" to the user via the status
    /// bar. (C3 UI dispatcher.)
    pub fn skip_stage(&mut self, stage_id: StageId) -> Option<PipelineId> {
        for (pid, pipe) in &mut self.pipelines {
            if let Some(stage) = pipe.stages.iter_mut().find(|s| s.id == stage_id) {
                stage.state = StageState::Skipped;
                return Some(*pid);
            }
        }
        None
    }

    /// Cancel every still-running stage in a pipeline. Marks the
    /// pipeline `Cancelled`. Idempotent.
    pub fn cancel_pipeline(&mut self, pipeline_id: PipelineId) {
        if let Some(pipe) = self.pipelines.get_mut(&pipeline_id) {
            for stage in &mut pipe.stages {
                if matches!(
                    stage.state,
                    StageState::Pending | StageState::Running { .. } | StageState::Failed { .. }
                ) {
                    stage.cancel.cancel();
                    stage.state = StageState::Cancelled;
                }
            }
            pipe.state = PipelineState::Cancelled;
        }
    }

    /// Record a stage failure. If retries remain, the stage stays in
    /// `Failed { attempt }` and the caller schedules a re-dispatch after
    /// [`super::stage::Stage::backoff_after`]. If retries are exhausted
    /// the stage becomes `Exhausted` and the pipeline is marked Failed.
    pub fn mark_stage_failed(&mut self, stage_id: StageId, error: String) -> Option<PipelineId> {
        let mut owning_pipeline = None;
        for (pid, pipe) in &mut self.pipelines {
            if let Some(stage) = pipe.stages.iter_mut().find(|s| s.id == stage_id) {
                stage.attempts = stage.attempts.saturating_add(1);
                if stage.attempts >= stage.max_attempts {
                    stage.state = StageState::Exhausted { error };
                    pipe.state = PipelineState::Failed;
                } else {
                    stage.state = StageState::Failed {
                        error,
                        attempt: stage.attempts,
                    };
                }
                owning_pipeline = Some(*pid);
                break;
            }
        }
        owning_pipeline
    }

    /// Host stamps a stage `Running` — used when a plugin reports
    /// `PipelineStageUpdate::Running` for a stage the host didn't
    /// itself dispatch via `dispatch_ready` (e.g. a plugin-internal
    /// sub-step it wants reflected in the DAG overlay).
    pub fn mark_stage_running(&mut self, stage_id: StageId) -> Option<PipelineId> {
        let now = now_ms();
        for (pid, pipe) in &mut self.pipelines {
            if let Some(stage) = pipe.stages.iter_mut().find(|s| s.id == stage_id) {
                stage.state = StageState::Running { started_at_ms: now };
                return Some(*pid);
            }
        }
        None
    }

    /// Mark a single stage `Cancelled` (as opposed to `cancel_pipeline`,
    /// which cascades the whole pipeline). Used for
    /// `PipelineStageUpdate::Cancelled`.
    pub fn mark_stage_cancelled(&mut self, stage_id: StageId) -> Option<PipelineId> {
        for (pid, pipe) in &mut self.pipelines {
            if let Some(stage) = pipe.stages.iter_mut().find(|s| s.id == stage_id) {
                stage.cancel.cancel();
                stage.state = StageState::Cancelled;
                return Some(*pid);
            }
        }
        None
    }

    /// Release every resource lock `stage_id` was holding. No-op if the
    /// stage held none (or wasn't found) — safe to call unconditionally
    /// whenever a stage leaves `Running`.
    fn free_locks_for(&mut self, stage_id: StageId) {
        let requires = self.pipelines.values().find_map(|p| {
            p.stages
                .iter()
                .find(|s| s.id == stage_id)
                .map(|s| s.requires.clone())
        });
        let Some(requires) = requires else {
            return;
        };
        for lock in &requires {
            if let Some(holders) = self.lock_holders.get_mut(&lock_key(lock)) {
                if let Some(pos) = holders.iter().position(|id| *id == stage_id) {
                    holders.remove(pos);
                }
            }
        }
    }

    /// Scan every pipeline for `Pending` stages whose inputs are all
    /// satisfied (`Done` or `Skipped`) and whose required resource
    /// locks have room. Reserves the locks and stamps each dispatched
    /// stage `Running`, returning the dispatch targets for the caller
    /// to hand to `PluginRegistry::dispatch_verb`.
    ///
    /// Locks are reserved one stage at a time (in pipeline-insertion,
    /// then stage-declaration order) so two stages contending for the
    /// same sole `Gpu` lock don't both dispatch in the same batch.
    pub fn dispatch_ready(&mut self) -> Vec<StageDispatch> {
        let mut dispatched = Vec::new();
        let now = now_ms();
        for pid in self.order.clone() {
            let Some(pipe) = self.pipelines.get(&pid) else {
                continue;
            };
            let candidates: Vec<StageId> = pipe
                .stages
                .iter()
                .filter(|s| matches!(s.state, StageState::Pending))
                .filter(|s| {
                    s.inputs.iter().all(|dep| {
                        pipe.stages
                            .iter()
                            .find(|d| d.id == *dep)
                            .map(|d| matches!(d.state, StageState::Done | StageState::Skipped))
                            .unwrap_or(false)
                    })
                })
                .map(|s| s.id)
                .collect();

            for stage_id in candidates {
                let Some(requires) = self.pipelines.get(&pid).and_then(|p| {
                    p.stages
                        .iter()
                        .find(|s| s.id == stage_id)
                        .map(|s| s.requires.clone())
                }) else {
                    continue;
                };
                let has_room = requires.iter().all(|lock| {
                    let held = self
                        .lock_holders
                        .get(&lock_key(lock))
                        .map(|v| v.len())
                        .unwrap_or(0);
                    held < lock_capacity(lock)
                });
                if !has_room {
                    continue;
                }
                for lock in &requires {
                    self.lock_holders
                        .entry(lock_key(lock))
                        .or_default()
                        .push(stage_id);
                }
                if let Some(p) = self.pipelines.get_mut(&pid) {
                    let owner = p.owner.clone();
                    if matches!(p.state, PipelineState::Pending) {
                        p.state = PipelineState::Running;
                    }
                    if let Some(s) = p.stages.iter_mut().find(|s| s.id == stage_id) {
                        s.state = StageState::Running { started_at_ms: now };
                        let verb = s.kind.verb().to_string();
                        dispatched.push(StageDispatch {
                            pipeline_id: pid,
                            stage_id,
                            owner,
                            verb,
                        });
                    }
                }
            }
        }
        dispatched
    }

    /// Submit a pipeline and immediately dispatch whatever stages are
    /// ready (no inputs, or all deps already satisfied). This is the
    /// host-side handler for `PluginAction::SubmitPipeline`.
    pub fn submit_and_dispatch(
        &mut self,
        pipeline: Pipeline,
    ) -> Result<(PipelineId, Vec<StageDispatch>), &'static str> {
        let id = self.submit(pipeline)?;
        Ok((id, self.dispatch_ready()))
    }

    /// Advance the DAG after `stage_id` reaches `Done`: frees its
    /// resource locks and dispatches any newly-ready stages (in this
    /// pipeline or another, since locks are global).
    pub fn advance_on_done(&mut self, stage_id: StageId) -> StageAdvance {
        let pipeline_id = self.mark_stage_done(stage_id);
        self.free_locks_for(stage_id);
        StageAdvance {
            pipeline_id,
            dispatch: self.dispatch_ready(),
            retry_after: None,
        }
    }

    /// Advance the DAG after `stage_id` fails: records the failure via
    /// `mark_stage_failed` (retrying up to `max_attempts` with
    /// `Stage::backoff_after`, then `Exhausted`), frees its resource
    /// locks either way, and dispatches whatever that freed up. If the
    /// stage still has attempts left, `retry_after` tells the caller
    /// how long to wait before calling `retry_stage` + `dispatch_ready`
    /// again.
    pub fn advance_on_failure(&mut self, stage_id: StageId, error: String) -> StageAdvance {
        let pipeline_id = self.mark_stage_failed(stage_id, error);
        self.free_locks_for(stage_id);
        let retry_after = pipeline_id.and_then(|pid| {
            let pipe = self.pipelines.get(&pid)?;
            let stage = pipe.stages.iter().find(|s| s.id == stage_id)?;
            match &stage.state {
                StageState::Failed { attempt, .. } => {
                    Some((stage_id, Stage::backoff_after(*attempt)))
                }
                _ => None,
            }
        });
        StageAdvance {
            pipeline_id,
            dispatch: self.dispatch_ready(),
            retry_after,
        }
    }

    /// Advance the DAG after `stage_id` reaches a terminal state that
    /// isn't `Done`/`Failed` (i.e. `Cancelled` or `Skipped` — the
    /// caller applies `mark_stage_cancelled`/`skip_stage` first). Frees
    /// its resource locks and dispatches whatever that freed up.
    pub fn advance_on_terminal(&mut self, stage_id: StageId) -> StageAdvance {
        let pipeline_id = self.pipelines.iter().find_map(|(pid, p)| {
            if p.stages.iter().any(|s| s.id == stage_id) {
                Some(*pid)
            } else {
                None
            }
        });
        self.free_locks_for(stage_id);
        StageAdvance {
            pipeline_id,
            dispatch: self.dispatch_ready(),
            retry_after: None,
        }
    }

    /// Whether `lock` currently has a free slot. Exposed for the DAG
    /// overlay / status bar (contention indicator) and for tests that
    /// need to confirm a lock was actually released on stage
    /// completion/failure rather than leaking.
    pub fn lock_available(&self, lock: &ResourceLock) -> bool {
        let held = self
            .lock_holders
            .get(&lock_key(lock))
            .map(|v| v.len())
            .unwrap_or(0);
        held < lock_capacity(lock)
    }
}

/// Per-resource semaphore handles. Created lazily on first request.
#[derive(Clone)]
pub struct ResourceRegistry {
    inner: Arc<Mutex<ResourceRegistryInner>>,
}

#[derive(Default)]
struct ResourceRegistryInner {
    gpu: Option<Arc<Semaphore>>,
    apis: HashMap<String, Arc<Semaphore>>,
    files: HashMap<String, Arc<Semaphore>>,
}

impl ResourceRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ResourceRegistryInner::default())),
        }
    }

    /// Acquire a permit for the given lock. Holds the permit until the
    /// returned guard is dropped. Caller awaits in a stage's body before
    /// running the actual work.
    pub async fn acquire(
        &self,
        lock: &ResourceLock,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, tokio::sync::AcquireError> {
        let sem = {
            let mut inner = self.inner.lock().await;
            match lock {
                ResourceLock::Gpu => inner
                    .gpu
                    .get_or_insert_with(|| Arc::new(Semaphore::new(1)))
                    .clone(),
                ResourceLock::Api { name, cap } => inner
                    .apis
                    .entry(name.clone())
                    .or_insert_with(|| Arc::new(Semaphore::new(*cap)))
                    .clone(),
                ResourceLock::File { path } => inner
                    .files
                    .entry(path.clone())
                    .or_insert_with(|| Arc::new(Semaphore::new(1)))
                    .clone(),
            }
        };
        sem.acquire_owned().await
    }
}

impl Default for ResourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::stage::{Stage, StageKind};

    #[test]
    fn submit_acyclic_ok() {
        let mut reg = PipelineRegistry::new();
        let mut p = Pipeline::new("test".to_string());
        let a = p.add_stage(Stage::new("a", StageKind::Extract));
        p.add_stage(Stage::new("b", StageKind::Subtitle).with_inputs(vec![a]));
        assert!(reg.submit(p).is_ok());
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn submit_cyclic_rejected() {
        let mut reg = PipelineRegistry::new();
        let mut p = Pipeline::new("bad".to_string());
        let a = p.add_stage(Stage::new("a", StageKind::Extract));
        let b = p.add_stage(Stage::new("b", StageKind::Subtitle).with_inputs(vec![a]));
        // Force the cycle.
        p.stages.iter_mut().find(|s| s.id == a).unwrap().inputs = vec![b];
        assert!(reg.submit(p).is_err());
    }

    #[test]
    fn retry_stage_resets_to_pending() {
        let mut reg = PipelineRegistry::new();
        let mut p = Pipeline::new("t".to_string());
        let a = p.add_stage(Stage::new("a", StageKind::Extract));
        let pid = reg.submit(p).unwrap();

        // Drive it to Exhausted.
        reg.mark_stage_failed(a, "boom".into());
        reg.mark_stage_failed(a, "boom".into());
        reg.mark_stage_failed(a, "boom".into());
        assert!(matches!(reg.get(pid).unwrap().state, PipelineState::Failed));

        // Retry — provider override is irrelevant for Extract but
        // shouldn't blow up.
        reg.retry_stage(a, None);
        let pipe = reg.get(pid).unwrap();
        assert!(matches!(pipe.stages[0].state, StageState::Pending));
        assert!(matches!(pipe.state, PipelineState::Running));
    }

    #[test]
    fn retry_stage_swaps_transcribe_provider() {
        let mut reg = PipelineRegistry::new();
        let mut p = Pipeline::new("t".to_string());
        let a = p.add_stage(Stage::new(
            "a",
            StageKind::Transcribe {
                provider: "whisper-cli".into(),
            },
        ));
        let pid = reg.submit(p).unwrap();
        reg.mark_stage_failed(a, "boom".into());

        reg.retry_stage(a, Some("voxtral-api".into()));
        let pipe = reg.get(pid).unwrap();
        match &pipe.stages[0].kind {
            StageKind::Transcribe { provider } => assert_eq!(provider, "voxtral-api"),
            _ => panic!("kind mutated unexpectedly"),
        }
    }

    #[test]
    fn skip_stage_marks_skipped() {
        let mut reg = PipelineRegistry::new();
        let mut p = Pipeline::new("t".to_string());
        let a = p.add_stage(Stage::new("a", StageKind::Subtitle));
        reg.submit(p).unwrap();
        reg.skip_stage(a);
        let pipe = reg.iter().next().unwrap();
        assert!(matches!(pipe.stages[0].state, StageState::Skipped));
    }

    #[test]
    fn cancel_pipeline_cascades() {
        let mut reg = PipelineRegistry::new();
        let mut p = Pipeline::new("t".to_string());
        let a = p.add_stage(Stage::new("a", StageKind::Extract));
        let b = p.add_stage(Stage::new("b", StageKind::Subtitle).with_inputs(vec![a]));
        let pid = reg.submit(p).unwrap();
        reg.cancel_pipeline(pid);
        let pipe = reg.get(pid).unwrap();
        assert!(matches!(pipe.state, PipelineState::Cancelled));
        for s in &pipe.stages {
            assert!(matches!(s.state, StageState::Cancelled));
        }
        let _ = b; // silence
    }

    #[test]
    fn stage_failure_retries_then_exhausts() {
        let mut reg = PipelineRegistry::new();
        let mut p = Pipeline::new("t".to_string());
        let a = p.add_stage(Stage::new("a", StageKind::Extract).with_max_attempts(2));
        let pid = reg.submit(p).unwrap();

        let owning = reg.mark_stage_failed(a, "boom".into()).unwrap();
        assert_eq!(owning, pid);
        let pipe = reg.get(pid).unwrap();
        assert!(matches!(
            pipe.stages[0].state,
            StageState::Failed { attempt: 1, .. }
        ));

        reg.mark_stage_failed(a, "boom again".into()).unwrap();
        let pipe = reg.get(pid).unwrap();
        assert!(matches!(pipe.stages[0].state, StageState::Exhausted { .. }));
        assert!(matches!(pipe.state, PipelineState::Failed));
    }

    #[tokio::test]
    async fn gpu_lock_serializes() {
        let reg = ResourceRegistry::new();
        let p1 = reg.acquire(&ResourceLock::Gpu).await.unwrap();
        // p2 would block forever waiting for the GPU; just confirm we can
        // drop p1 and then immediately get p2.
        drop(p1);
        let p2 = reg.acquire(&ResourceLock::Gpu).await.unwrap();
        drop(p2);
    }

    // M2.P2.S1.T1 — the daemon's `PluginAction::SubmitPipeline` handler
    // calls exactly `submit_and_dispatch`; this exercises that real
    // path (not a reimplementation) and observes "dispatched" via the
    // returned `StageDispatch` list, standing in for the daemon's fake
    // verb-dispatch sink.
    #[test]
    #[cfg(feature = "creator")]
    fn pipeline_submit_dispatch() {
        let mut reg = PipelineRegistry::new();
        let mut p = Pipeline::new("t").with_owner("test-plugin");
        let a = p.add_stage(Stage::new("a", StageKind::Extract));
        let b = p.add_stage(Stage::new("b", StageKind::Subtitle).with_inputs(vec![a]));

        let (pid, dispatches) = reg.submit_and_dispatch(p).unwrap();

        // The pipeline is held in the registry.
        assert_eq!(reg.len(), 1);
        assert!(reg.get(pid).is_some());

        // Only the dep-free stage (a) dispatches; b is gated on a.
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].pipeline_id, pid);
        assert_eq!(dispatches[0].stage_id, a);
        assert_eq!(dispatches[0].owner, "test-plugin");
        assert_eq!(dispatches[0].verb, "extract");

        let pipe = reg.get(pid).unwrap();
        assert!(matches!(pipe.stages[0].state, StageState::Running { .. }));
        assert!(matches!(pipe.stages[1].state, StageState::Pending));
        let _ = b;
    }

    // M2.P2.S1.T2 — drives `advance_on_failure`/`advance_on_done`
    // (the same methods the daemon's `UpdateStage` handler calls) end
    // to end: retry with backoff up to `max_attempts`, then
    // `Exhausted`; the GPU lock the failing stage held is released
    // rather than leaked; and a dependent stage only dispatches once
    // its dep actually reaches `Done` (never merely `Exhausted`).
    #[test]
    #[cfg(feature = "creator")]
    fn pipeline_advance_backoff() {
        let mut reg = PipelineRegistry::new();

        // Pipeline 1: a (Gpu, max_attempts=2) -> b (dependent).
        let mut p1 = Pipeline::new("p1").with_owner("test-plugin");
        let a = p1.add_stage(
            Stage::new("a", StageKind::Extract)
                .with_max_attempts(2)
                .with_requires(vec![ResourceLock::Gpu]),
        );
        let b = p1.add_stage(Stage::new("b", StageKind::Subtitle).with_inputs(vec![a]));
        let (pid1, first_dispatch) = reg.submit_and_dispatch(p1).unwrap();
        assert_eq!(first_dispatch.len(), 1);
        assert_eq!(first_dispatch[0].stage_id, a);
        assert!(!reg.lock_available(&ResourceLock::Gpu));

        // First failure: attempt 1 of 2 — retries after backoff_after(1).
        let advance1 = reg.advance_on_failure(a, "boom".into());
        assert_eq!(advance1.pipeline_id, Some(pid1));
        assert!(advance1.dispatch.is_empty(), "b still gated on a");
        assert_eq!(
            advance1.retry_after,
            Some((a, Stage::backoff_after(1))),
            "must honour the model's backoff schedule, not invent a new one"
        );
        assert!(
            reg.lock_available(&ResourceLock::Gpu),
            "failed stage must release its Gpu lock, not hold it during backoff"
        );
        assert!(matches!(
            reg.get(pid1).unwrap().stages[0].state,
            StageState::Failed { attempt: 1, .. }
        ));
        assert!(matches!(reg.get(pid1).unwrap().stages[1].state, StageState::Pending));

        // Second failure: attempt 2 of 2 — exhausted, no more retries.
        let advance2 = reg.advance_on_failure(a, "boom again".into());
        assert!(advance2.retry_after.is_none());
        assert!(matches!(
            reg.get(pid1).unwrap().stages[0].state,
            StageState::Exhausted { .. }
        ));
        assert!(matches!(reg.get(pid1).unwrap().state, PipelineState::Failed));
        assert!(reg.lock_available(&ResourceLock::Gpu));
        // b never dispatches — its dep never reached Done, only Exhausted.
        assert!(matches!(reg.get(pid1).unwrap().stages[1].state, StageState::Pending));
        assert!(advance2.dispatch.iter().all(|d| d.stage_id != b));

        // Pipeline 2: c -> d, proves the positive case — d dispatches
        // only once c reaches Done (not before).
        let mut p2 = Pipeline::new("p2").with_owner("test-plugin");
        let c = p2.add_stage(Stage::new("c", StageKind::Extract));
        let d = p2.add_stage(Stage::new("d", StageKind::Subtitle).with_inputs(vec![c]));
        let (pid2, dispatch2) = reg.submit_and_dispatch(p2).unwrap();
        assert_eq!(dispatch2.len(), 1);
        assert_eq!(dispatch2[0].stage_id, c);
        assert!(reg.get(pid2).unwrap().stages[1].state == StageState::Pending);

        let advance3 = reg.advance_on_done(c);
        assert_eq!(advance3.dispatch.len(), 1);
        assert_eq!(advance3.dispatch[0].stage_id, d);
        assert_eq!(advance3.dispatch[0].verb, "subtitle");
        assert!(matches!(
            reg.get(pid2).unwrap().stages[1].state,
            StageState::Running { .. }
        ));
    }
}
