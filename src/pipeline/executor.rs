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

use tokio::sync::{Mutex, Semaphore};

use super::stage::{Pipeline, PipelineId, PipelineState, ResourceLock, StageId, StageState};

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

    pub fn update_stage(
        &mut self,
        stage_id: StageId,
        update: crate::plugin::PipelineStageUpdate,
    ) -> Option<PipelineId> {
        use crate::plugin::PipelineStageUpdate;
        for (pid, pipe) in &mut self.pipelines {
            if let Some(stage) = pipe.stages.iter_mut().find(|stage| stage.id == stage_id) {
                stage.state = match update {
                    PipelineStageUpdate::Running => StageState::Running {
                        started_at_ms: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|duration| duration.as_millis())
                            .unwrap_or(0),
                    },
                    PipelineStageUpdate::Done => StageState::Done,
                    PipelineStageUpdate::Failed(error) => StageState::Failed {
                        error,
                        attempt: stage.attempts.saturating_add(1),
                    },
                    PipelineStageUpdate::Cancelled => StageState::Cancelled,
                    PipelineStageUpdate::Skipped => StageState::Skipped,
                };
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
}
