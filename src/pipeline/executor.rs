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
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{Mutex, Semaphore};

use super::stage::{Pipeline, PipelineId, PipelineState, ResourceLock, Stage, StageId, StageState};

const MAX_PIPELINE_HISTORY: usize = 500;

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
    /// Durable snapshot path. `None` keeps tests and embedders in-memory.
    persistence_path: Option<PathBuf>,
}

impl PipelineRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn submit(&mut self, pipeline: Pipeline) -> Result<PipelineId, &'static str> {
        Self::validate(&pipeline)?;
        let id = pipeline.id;
        if self.pipelines.contains_key(&id) {
            return Err("pipeline id already exists");
        }
        if self.iter().any(|existing| {
            !existing.is_terminal()
                && existing.name == pipeline.name
                && existing.subject_id == pipeline.subject_id
        }) {
            return Err("an equivalent pipeline is already active for this subject");
        }
        while self.pipelines.len() >= MAX_PIPELINE_HISTORY {
            let Some(oldest_terminal) = self
                .order
                .iter()
                .find(|id| self.pipelines.get(id).is_some_and(Pipeline::is_terminal))
                .copied()
            else {
                return Err("pipeline history is full of active runs");
            };
            self.order.retain(|id| *id != oldest_terminal);
            self.pipelines.remove(&oldest_terminal);
        }
        self.order.push(id);
        self.pipelines.insert(id, pipeline);
        self.persist_best_effort();
        Ok(id)
    }

    fn validate(pipeline: &Pipeline) -> Result<(), &'static str> {
        pipeline.assert_acyclic()?;
        if pipeline.stages.is_empty() {
            return Err("pipeline must contain at least one stage");
        }
        let ids: std::collections::HashSet<_> = pipeline.stages.iter().map(|s| s.id).collect();
        if ids.len() != pipeline.stages.len() {
            return Err("pipeline contains duplicate stage ids");
        }
        if pipeline
            .stages
            .iter()
            .flat_map(|stage| &stage.inputs)
            .any(|input| !ids.contains(input))
        {
            return Err("pipeline references an unknown input stage");
        }
        if pipeline.stages.iter().any(|stage| {
            stage.max_attempts == 0
                || stage
                    .requires
                    .iter()
                    .any(|lock| matches!(lock, ResourceLock::Api { cap: 0, .. }))
        }) {
            return Err("pipeline contains a zero retry or resource capacity");
        }
        if pipeline.stages.iter().any(|stage| {
            stage.dispatch.as_ref().is_some_and(|dispatch| {
                dispatch.plugin.trim().is_empty() || dispatch.verb.trim().is_empty()
            })
        }) {
            return Err("pipeline contains an empty dispatch target");
        }
        Ok(())
    }

    /// Open the durable registry. Interrupted running stages are re-queued:
    /// process death proves they are no longer running, while idempotent stage
    /// executors and file locks make retrying safer than silently losing work.
    pub fn open(path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let path = path.into();
        let mut registry = if path.exists() {
            let bytes = std::fs::read(&path)?;
            let pipelines: Vec<Pipeline> = match serde_json::from_slice::<Vec<Pipeline>>(&bytes) {
                Ok(pipelines)
                    if pipelines
                        .iter()
                        .all(|pipeline| Self::validate(pipeline).is_ok()) =>
                {
                    pipelines
                }
                Err(error) => {
                    let quarantine =
                        path.with_file_name(format!("pipelines.corrupt-{}.json", unix_secs()));
                    std::fs::rename(&path, &quarantine)?;
                    tracing::error!(
                        %error,
                        path = %path.display(),
                        quarantine = %quarantine.display(),
                        "corrupt pipeline registry quarantined"
                    );
                    Vec::new()
                }
                Ok(_) => {
                    let quarantine =
                        path.with_file_name(format!("pipelines.invalid-{}.json", unix_secs()));
                    std::fs::rename(&path, &quarantine)?;
                    tracing::error!(
                        path = %path.display(),
                        quarantine = %quarantine.display(),
                        "invalid pipeline registry quarantined"
                    );
                    Vec::new()
                }
            };
            let mut map = HashMap::new();
            let mut order = Vec::new();
            for mut pipeline in pipelines {
                Self::validate(&pipeline)
                    .map_err(|error| anyhow::anyhow!("invalid persisted pipeline: {error}"))?;
                for stage in &mut pipeline.stages {
                    if matches!(stage.state, StageState::Running { .. }) {
                        stage.state = StageState::Pending;
                        stage.cancel = tokio_util::sync::CancellationToken::new();
                    }
                }
                if matches!(pipeline.state, PipelineState::Running) {
                    pipeline.state = PipelineState::Pending;
                }
                order.push(pipeline.id);
                map.insert(pipeline.id, pipeline);
            }
            Self {
                pipelines: map,
                order,
                persistence_path: Some(path),
            }
        } else {
            Self {
                persistence_path: Some(path),
                ..Self::default()
            }
        };
        registry.persist()?;
        Ok(registry)
    }

    pub fn snapshot_path(&self) -> Option<&Path> {
        self.persistence_path.as_deref()
    }

    pub fn persist(&mut self) -> anyhow::Result<()> {
        let Some(path) = self.persistence_path.as_ref() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let payload: Vec<&Pipeline> = self.iter().collect();
        let bytes = serde_json::to_vec_pretty(&payload)?;
        let tmp = path.with_extension("json.tmp");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)?;
        if let Some(parent) = path.parent() {
            if let Ok(directory) = std::fs::File::open(parent) {
                let _ = directory.sync_all();
            }
        }
        Ok(())
    }

    fn persist_best_effort(&mut self) {
        if let Err(error) = self.persist() {
            tracing::error!(%error, "pipeline registry persistence failed");
        }
    }

    pub fn ready_stages(&self) -> Vec<(PipelineId, StageId)> {
        let mut ready = Vec::new();
        for pipeline in self.iter().filter(|pipeline| {
            matches!(
                pipeline.state,
                PipelineState::Pending | PipelineState::Running
            )
        }) {
            for stage in &pipeline.stages {
                if !matches!(stage.state, StageState::Pending) {
                    continue;
                }
                let inputs_ready = stage.inputs.iter().all(|input| {
                    pipeline.stages.iter().any(|candidate| {
                        candidate.id == *input
                            && matches!(candidate.state, StageState::Done | StageState::Skipped)
                    })
                });
                if inputs_ready {
                    ready.push((pipeline.id, stage.id));
                }
            }
        }
        ready
    }

    pub fn mark_stage_running(&mut self, pipeline_id: PipelineId, stage_id: StageId) -> bool {
        let Some(pipeline) = self.pipelines.get_mut(&pipeline_id) else {
            return false;
        };
        let Some(stage) = pipeline
            .stages
            .iter_mut()
            .find(|stage| stage.id == stage_id)
        else {
            return false;
        };
        if !matches!(stage.state, StageState::Pending | StageState::Failed { .. }) {
            return false;
        }
        let now_ms = unix_millis();
        let now_secs = (now_ms / 1000) as u64;
        stage.state = StageState::Running {
            started_at_ms: now_ms,
        };
        pipeline.state = PipelineState::Running;
        pipeline.started_at_secs.get_or_insert(now_secs);
        self.persist_best_effort();
        true
    }

    pub fn complete_stage(
        &mut self,
        pipeline_id: PipelineId,
        stage_id: StageId,
        artifacts: Vec<serde_json::Value>,
    ) -> bool {
        let Some(pipeline) = self.pipelines.get_mut(&pipeline_id) else {
            return false;
        };
        let Some(stage) = pipeline
            .stages
            .iter_mut()
            .find(|stage| stage.id == stage_id)
        else {
            return false;
        };
        if !matches!(stage.state, StageState::Running { .. }) {
            return false;
        }
        stage.state = StageState::Done;
        stage.artifacts = artifacts;
        Self::recompute_pipeline(pipeline);
        self.persist_best_effort();
        true
    }

    fn recompute_pipeline(pipeline: &mut Pipeline) {
        if pipeline
            .stages
            .iter()
            .any(|stage| matches!(stage.state, StageState::Exhausted { .. }))
        {
            pipeline.state = PipelineState::Failed;
            pipeline.completed_at_secs = Some(unix_secs());
            pipeline.error = pipeline.stages.iter().find_map(|stage| match &stage.state {
                StageState::Exhausted { error } => Some(error.clone()),
                _ => None,
            });
        } else if pipeline.stages.iter().all(Stage::is_terminal) {
            pipeline.state = PipelineState::Done;
            pipeline.completed_at_secs = Some(unix_secs());
            pipeline.error = None;
        } else if pipeline
            .stages
            .iter()
            .any(|stage| matches!(stage.state, StageState::Running { .. }))
        {
            pipeline.state = PipelineState::Running;
        }
    }

    pub fn get(&self, id: PipelineId) -> Option<&Pipeline> {
        self.pipelines.get(&id)
    }

    pub fn get_mut(&mut self, id: PipelineId) -> Option<&mut Pipeline> {
        self.pipelines.get_mut(&id)
    }

    pub fn remove(&mut self, id: PipelineId) -> Option<Pipeline> {
        self.order.retain(|&i| i != id);
        let removed = self.pipelines.remove(&id);
        self.persist_best_effort();
        removed
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
        let mut found = None;
        for (pid, pipe) in &mut self.pipelines {
            if let Some(stage) = pipe.stages.iter_mut().find(|s| s.id == stage_id) {
                if !matches!(stage.state, StageState::Running { .. }) {
                    return None;
                }
                stage.state = StageState::Done;
                Self::recompute_pipeline(pipe);
                found = Some(*pid);
                break;
            }
        }
        if found.is_some() {
            self.persist_best_effort();
        }
        found
    }

    pub fn update_stage(
        &mut self,
        stage_id: StageId,
        update: crate::plugin::PipelineStageUpdate,
    ) -> Option<PipelineId> {
        use crate::plugin::PipelineStageUpdate;
        let mut found = None;
        for (pid, pipe) in &mut self.pipelines {
            if let Some(stage) = pipe.stages.iter_mut().find(|stage| stage.id == stage_id) {
                let allowed = match &update {
                    PipelineStageUpdate::Running => {
                        matches!(stage.state, StageState::Pending | StageState::Failed { .. })
                    }
                    PipelineStageUpdate::Done
                    | PipelineStageUpdate::Failed(_)
                    | PipelineStageUpdate::Cancelled => {
                        matches!(stage.state, StageState::Running { .. })
                    }
                    PipelineStageUpdate::Skipped => {
                        matches!(stage.state, StageState::Pending | StageState::Failed { .. })
                    }
                };
                if !allowed {
                    return None;
                }
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
                Self::recompute_pipeline(pipe);
                found = Some(*pid);
                break;
            }
        }
        if found.is_some() {
            self.persist_best_effort();
        }
        found
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
        let mut found = None;
        for (pid, pipe) in &mut self.pipelines {
            if let Some(index) = pipe.stages.iter().position(|stage| stage.id == stage_id) {
                let was_exhausted =
                    matches!(pipe.stages[index].state, StageState::Exhausted { .. });
                let stage = &mut pipe.stages[index];
                if !matches!(
                    stage.state,
                    StageState::Failed { .. } | StageState::Exhausted { .. }
                ) {
                    return None;
                }
                if matches!(stage.state, StageState::Exhausted { .. }) {
                    stage.attempts = 0;
                }
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
                if was_exhausted {
                    for sibling in &mut pipe.stages {
                        if sibling.id != stage_id
                            && matches!(
                                sibling.state,
                                StageState::Failed { .. }
                                    | StageState::Exhausted { .. }
                                    | StageState::Cancelled
                                    | StageState::Skipped
                            )
                        {
                            sibling.state = StageState::Pending;
                            sibling.attempts = 0;
                            sibling.cancel = tokio_util::sync::CancellationToken::new();
                        }
                    }
                }
                // A pipeline that had any retryable stage flips back
                // to Running so the executor wakes; the executor
                // re-checks the post-condition once stages settle.
                if matches!(pipe.state, PipelineState::Failed) {
                    pipe.state = PipelineState::Running;
                }
                pipe.completed_at_secs = None;
                pipe.error = None;
                found = Some(*pid);
                break;
            }
        }
        if found.is_some() {
            self.persist_best_effort();
        }
        found
    }

    /// Requeue only an automatically retryable failure. Unlike a manual retry,
    /// this preserves the attempt counter and cannot resurrect a cancelled,
    /// exhausted, running, or completed stage.
    pub fn retry_failed_stage(&mut self, stage_id: StageId) -> Option<PipelineId> {
        let mut found = None;
        for (pid, pipe) in &mut self.pipelines {
            if !matches!(pipe.state, PipelineState::Running | PipelineState::Pending) {
                continue;
            }
            if let Some(stage) = pipe.stages.iter_mut().find(|stage| stage.id == stage_id) {
                if !matches!(stage.state, StageState::Failed { .. }) {
                    return None;
                }
                stage.state = StageState::Pending;
                stage.cancel = tokio_util::sync::CancellationToken::new();
                found = Some(*pid);
                break;
            }
        }
        if found.is_some() {
            self.persist_best_effort();
        }
        found
    }

    /// Mark a stage as `Skipped` so the executor walks past it without
    /// running. Downstream stages with this stage in their inputs
    /// proceed as if the skipped stage had completed. The caller is
    /// responsible for explaining "why" to the user via the status
    /// bar. (C3 UI dispatcher.)
    pub fn skip_stage(&mut self, stage_id: StageId) -> Option<PipelineId> {
        let mut found = None;
        for (pid, pipe) in &mut self.pipelines {
            if let Some(stage) = pipe.stages.iter_mut().find(|s| s.id == stage_id) {
                stage.state = StageState::Skipped;
                Self::recompute_pipeline(pipe);
                found = Some(*pid);
                break;
            }
        }
        if found.is_some() {
            self.persist_best_effort();
        }
        found
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
            pipe.completed_at_secs = Some(unix_secs());
            self.persist_best_effort();
        }
    }

    /// Record a stage failure. If retries remain, the stage stays in
    /// `Failed { attempt }` and the caller schedules a re-dispatch after
    /// [`super::stage::Stage::backoff_after`]. If retries are exhausted
    /// the stage becomes `Exhausted` and the pipeline is marked Failed.
    pub fn mark_stage_failed(&mut self, stage_id: StageId, error: String) -> Option<PipelineId> {
        let mut owning_pipeline = None;
        for (pid, pipe) in &mut self.pipelines {
            if let Some(index) = pipe.stages.iter().position(|stage| stage.id == stage_id) {
                let stage = &mut pipe.stages[index];
                if !matches!(stage.state, StageState::Running { .. }) {
                    return None;
                }
                stage.attempts = stage.attempts.saturating_add(1);
                if stage.attempts >= stage.max_attempts {
                    stage.state = StageState::Exhausted { error };
                    pipe.state = PipelineState::Failed;
                    pipe.completed_at_secs = Some(unix_secs());
                    pipe.error = match &stage.state {
                        StageState::Exhausted { error } => Some(error.clone()),
                        _ => None,
                    };
                    let _ = stage;
                    Self::stop_siblings(pipe, stage_id);
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
        self.persist_best_effort();
        owning_pipeline
    }

    fn stop_siblings(pipeline: &mut Pipeline, failed_stage_id: StageId) {
        for stage in &mut pipeline.stages {
            if stage.id == failed_stage_id {
                continue;
            }
            match stage.state {
                StageState::Pending | StageState::Failed { .. } => {
                    stage.cancel.cancel();
                    stage.state = StageState::Skipped;
                }
                _ => {}
            }
        }
    }

    pub fn exhaust_stage(&mut self, stage_id: StageId, error: String) -> Option<PipelineId> {
        let mut found = None;
        for (pipeline_id, pipeline) in &mut self.pipelines {
            if let Some(index) = pipeline
                .stages
                .iter()
                .position(|stage| stage.id == stage_id)
            {
                let stage = &mut pipeline.stages[index];
                if !matches!(stage.state, StageState::Running { .. }) {
                    return None;
                }
                stage.attempts = stage.max_attempts;
                stage.state = StageState::Exhausted {
                    error: error.clone(),
                };
                pipeline.state = PipelineState::Failed;
                pipeline.error = Some(error);
                pipeline.completed_at_secs = Some(unix_secs());
                let _ = stage;
                Self::stop_siblings(pipeline, stage_id);
                found = Some(*pipeline_id);
                break;
            }
        }
        if found.is_some() {
            self.persist_best_effort();
        }
        found
    }
}

fn unix_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn unix_secs() -> u64 {
    (unix_millis() / 1000) as u64
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
        for _ in 0..3 {
            assert!(reg.mark_stage_running(pid, a));
            reg.mark_stage_failed(a, "boom".into());
            if !matches!(reg.get(pid).unwrap().state, PipelineState::Failed) {
                reg.retry_failed_stage(a);
            }
        }
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
        assert!(reg.mark_stage_running(pid, a));
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
    fn ready_stages_advance_only_after_dependencies_finish() {
        let mut registry = PipelineRegistry::new();
        let mut pipeline = Pipeline::new("fan-out");
        let root = pipeline.add_stage(Stage::new("root", StageKind::Extract));
        let left =
            pipeline.add_stage(Stage::new("left", StageKind::Subtitle).with_inputs(vec![root]));
        let right = pipeline.add_stage(
            Stage::new(
                "right",
                StageKind::Analyze {
                    provider: "test".into(),
                },
            )
            .with_inputs(vec![root]),
        );
        let id = registry.submit(pipeline).unwrap();
        assert_eq!(registry.ready_stages(), vec![(id, root)]);

        assert!(registry.mark_stage_running(id, root));
        assert!(registry.ready_stages().is_empty());
        assert!(registry.complete_stage(id, root, vec![]));
        let ready = registry.ready_stages();
        assert_eq!(ready.len(), 2);
        assert!(ready.contains(&(id, left)));
        assert!(ready.contains(&(id, right)));
    }

    #[test]
    fn durable_registry_requeues_interrupted_stage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pipelines.json");
        let (pipeline_id, stage_id) = {
            let mut registry = PipelineRegistry::open(&path).unwrap();
            let mut pipeline = Pipeline::new("durable");
            let stage = pipeline.add_stage(Stage::new("work", StageKind::Extract));
            let id = registry.submit(pipeline).unwrap();
            assert!(registry.mark_stage_running(id, stage));
            (id, stage)
        };

        let registry = PipelineRegistry::open(&path).unwrap();
        let pipeline = registry.get(pipeline_id).unwrap();
        assert!(matches!(pipeline.state, PipelineState::Pending));
        assert!(matches!(pipeline.stages[0].state, StageState::Pending));
        assert_eq!(registry.ready_stages(), vec![(pipeline_id, stage_id)]);
    }

    #[test]
    fn rejects_dangling_dependency_and_duplicate_pipeline_id() {
        let mut registry = PipelineRegistry::new();
        let mut dangling = Pipeline::new("dangling");
        dangling.add_stage(
            Stage::new("work", StageKind::Extract).with_inputs(vec![uuid::Uuid::new_v4()]),
        );
        assert_eq!(
            registry.submit(dangling),
            Err("pipeline references an unknown input stage")
        );

        let mut pipeline = Pipeline::new("once");
        pipeline.add_stage(Stage::new("work", StageKind::Extract));
        let clone = pipeline.clone();
        registry.submit(pipeline).unwrap();
        assert_eq!(registry.submit(clone), Err("pipeline id already exists"));
    }

    #[test]
    fn corrupt_registry_is_quarantined_and_durability_continues() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pipelines.json");
        std::fs::write(&path, b"{ definitely not json").unwrap();

        let mut registry = PipelineRegistry::open(&path).unwrap();
        assert!(registry.is_empty());
        assert!(path.exists());
        assert!(std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with("pipelines.corrupt-")));

        let mut pipeline = Pipeline::new("after recovery");
        pipeline.add_stage(Stage::new("work", StageKind::Extract));
        registry.submit(pipeline).unwrap();
        let reopened = PipelineRegistry::open(&path).unwrap();
        assert_eq!(reopened.len(), 1);
    }

    #[test]
    fn semantically_invalid_registry_is_quarantined() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pipelines.json");
        let mut pipeline = Pipeline::new("invalid");
        pipeline.add_stage(Stage::new("work", StageKind::Extract).with_max_attempts(0));
        std::fs::write(&path, serde_json::to_vec(&vec![pipeline]).unwrap()).unwrap();

        let registry = PipelineRegistry::open(&path).unwrap();
        assert!(registry.is_empty());
        assert!(std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with("pipelines.invalid-")));
    }

    #[test]
    fn stage_failure_retries_then_exhausts() {
        let mut reg = PipelineRegistry::new();
        let mut p = Pipeline::new("t".to_string());
        let a = p.add_stage(Stage::new("a", StageKind::Extract).with_max_attempts(2));
        let pid = reg.submit(p).unwrap();

        assert!(reg.mark_stage_running(pid, a));
        let owning = reg.mark_stage_failed(a, "boom".into()).unwrap();
        assert_eq!(owning, pid);
        let pipe = reg.get(pid).unwrap();
        assert!(matches!(
            pipe.stages[0].state,
            StageState::Failed { attempt: 1, .. }
        ));

        assert_eq!(reg.retry_failed_stage(a), Some(pid));
        assert!(reg.mark_stage_running(pid, a));
        reg.mark_stage_failed(a, "boom again".into()).unwrap();
        let pipe = reg.get(pid).unwrap();
        assert!(matches!(pipe.stages[0].state, StageState::Exhausted { .. }));
        assert!(matches!(pipe.state, PipelineState::Failed));
    }

    #[test]
    fn late_worker_result_cannot_resurrect_cancelled_pipeline() {
        let mut registry = PipelineRegistry::new();
        let mut pipeline = Pipeline::new("cancel race");
        let stage = pipeline.add_stage(Stage::new("work", StageKind::Extract));
        let pipeline_id = registry.submit(pipeline).unwrap();
        assert!(registry.mark_stage_running(pipeline_id, stage));

        registry.cancel_pipeline(pipeline_id);
        assert!(!registry.complete_stage(pipeline_id, stage, vec![]));
        assert!(registry
            .mark_stage_failed(stage, "late failure".into())
            .is_none());

        let pipeline = registry.get(pipeline_id).unwrap();
        assert!(matches!(pipeline.state, PipelineState::Cancelled));
        assert!(matches!(pipeline.stages[0].state, StageState::Cancelled));
    }

    #[test]
    fn exhausted_stage_stops_sibling_work_and_records_error() {
        let mut registry = PipelineRegistry::new();
        let mut pipeline = Pipeline::new("fail fast");
        let failed =
            pipeline.add_stage(Stage::new("failed", StageKind::Extract).with_max_attempts(1));
        let sibling = pipeline.add_stage(Stage::new("sibling", StageKind::Subtitle));
        let downstream = pipeline
            .add_stage(Stage::new("downstream", StageKind::Concat).with_inputs(vec![failed]));
        let pipeline_id = registry.submit(pipeline).unwrap();
        assert!(registry.mark_stage_running(pipeline_id, failed));
        assert!(registry.mark_stage_running(pipeline_id, sibling));

        registry
            .mark_stage_failed(failed, "terminal".into())
            .unwrap();
        let pipeline = registry.get(pipeline_id).unwrap();
        assert_eq!(pipeline.error.as_deref(), Some("terminal"));
        assert!(matches!(
            pipeline.stages[1].state,
            StageState::Running { .. }
        ));
        assert!(matches!(pipeline.stages[2].state, StageState::Skipped));
        assert!(registry.complete_stage(pipeline_id, sibling, vec![]));
        let _ = downstream;
    }

    #[test]
    fn manual_retry_rearms_failed_branch_after_exhaustion() {
        let mut registry = PipelineRegistry::new();
        let mut pipeline = Pipeline::new("retry graph");
        let root = pipeline.add_stage(Stage::new("root", StageKind::Extract).with_max_attempts(1));
        let downstream =
            pipeline.add_stage(Stage::new("downstream", StageKind::Concat).with_inputs(vec![root]));
        let pipeline_id = registry.submit(pipeline).unwrap();
        assert!(registry.mark_stage_running(pipeline_id, root));
        registry.mark_stage_failed(root, "terminal".into()).unwrap();
        assert!(matches!(
            registry.get(pipeline_id).unwrap().stages[1].state,
            StageState::Skipped
        ));

        assert_eq!(registry.retry_stage(root, None), Some(pipeline_id));
        let pipeline = registry.get(pipeline_id).unwrap();
        assert!(matches!(pipeline.state, PipelineState::Running));
        assert!(pipeline
            .stages
            .iter()
            .all(|stage| matches!(stage.state, StageState::Pending)));
        assert_eq!(pipeline.stages[0].attempts, 0);
        let _ = downstream;
    }

    #[test]
    fn validation_rejects_non_executable_resource_policies() {
        let mut registry = PipelineRegistry::new();
        let mut zero_retry = Pipeline::new("zero retry");
        zero_retry.add_stage(Stage::new("work", StageKind::Extract).with_max_attempts(0));
        assert_eq!(
            registry.submit(zero_retry),
            Err("pipeline contains a zero retry or resource capacity")
        );

        let mut zero_capacity = Pipeline::new("zero capacity");
        zero_capacity.add_stage(Stage::new("work", StageKind::Extract).with_requires(vec![
            ResourceLock::Api {
                name: "provider".into(),
                cap: 0,
            },
        ]));
        assert_eq!(
            registry.submit(zero_capacity),
            Err("pipeline contains a zero retry or resource capacity")
        );
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
