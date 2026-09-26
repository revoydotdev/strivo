//! Durable daemon-owned DAG scheduler.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex, Notify, RwLock};
use tokio_util::sync::CancellationToken;

use crate::events::DaemonEvent;
use crate::plugin::registry::PluginRegistry;
use crate::plugin::{PluginAction, VerbContext};
use crate::recording::job::RecordingJob;

use super::executor::{LockAcquireError, RESOURCE_ACQUIRE_TIMEOUT};
use super::stage::ResourceLock;
use super::{PipelineId, PipelineRegistry, ResourceRegistry, Stage, StageId, StageState};

#[derive(Clone)]
pub struct PipelineRuntime {
    registry: Arc<Mutex<PipelineRegistry>>,
    wake: Arc<Notify>,
    event_tx: mpsc::UnboundedSender<DaemonEvent>,
}

impl PipelineRuntime {
    pub fn spawn(
        registry: Arc<Mutex<PipelineRegistry>>,
        plugins: Arc<Mutex<PluginRegistry>>,
        recordings: Arc<RwLock<HashMap<uuid::Uuid, RecordingJob>>>,
        capture_reserve: crate::recording::capacity::CaptureReserve,
        plugin_toggles: std::collections::BTreeMap<String, crate::config::PluginToggle>,
        event_tx: mpsc::UnboundedSender<DaemonEvent>,
        action_tx: mpsc::UnboundedSender<PluginAction>,
        cancel: CancellationToken,
    ) -> Self {
        let runtime = Self {
            registry,
            wake: Arc::new(Notify::new()),
            event_tx: event_tx.clone(),
        };
        let worker = runtime.clone();
        tokio::spawn(async move {
            let resources = ResourceRegistry::new().with_capture_reserve(capture_reserve);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = worker.wake.notified() => {}
                }
                worker
                    .dispatch_ready(
                        &plugins,
                        &recordings,
                        &plugin_toggles,
                        &resources,
                        &action_tx,
                    )
                    .await;
            }
        });
        runtime
    }

    pub fn wake(&self) {
        self.wake.notify_one();
    }

    pub async fn submit(&self, pipeline: super::Pipeline) -> Result<PipelineId, &'static str> {
        let id = self.registry.lock().await.submit(pipeline)?;
        self.emit_snapshot(id).await;
        self.wake();
        Ok(id)
    }

    pub async fn cancel(&self, pipeline_id: PipelineId) {
        self.registry.lock().await.cancel_pipeline(pipeline_id);
        self.emit_snapshot(pipeline_id).await;
    }

    pub async fn retry(&self, stage_id: StageId) -> bool {
        let pipeline_id = self.registry.lock().await.retry_stage(stage_id, None);
        if let Some(id) = pipeline_id {
            self.emit_snapshot(id).await;
            self.wake();
            true
        } else {
            false
        }
    }

    pub async fn update_stage(
        &self,
        stage_id: StageId,
        update: crate::plugin::PipelineStageUpdate,
    ) -> bool {
        let pipeline_id = self.registry.lock().await.update_stage(stage_id, update);
        if let Some(id) = pipeline_id {
            self.emit_snapshot(id).await;
            self.wake();
            true
        } else {
            false
        }
    }

    async fn dispatch_ready(
        &self,
        plugins: &Arc<Mutex<PluginRegistry>>,
        recordings: &Arc<RwLock<HashMap<uuid::Uuid, RecordingJob>>>,
        plugin_toggles: &std::collections::BTreeMap<String, crate::config::PluginToggle>,
        resources: &ResourceRegistry,
        action_tx: &mpsc::UnboundedSender<PluginAction>,
    ) {
        let ready = self.registry.lock().await.ready_stages();
        for (pipeline_id, stage_id) in ready {
            let stage = {
                let mut registry = self.registry.lock().await;
                if !registry.mark_stage_running(pipeline_id, stage_id) {
                    continue;
                }
                registry
                    .get(pipeline_id)
                    .and_then(|pipeline| pipeline.stages.iter().find(|stage| stage.id == stage_id))
                    .cloned()
            };
            let Some(stage) = stage else { continue };
            self.emit_snapshot(pipeline_id).await;

            let Some(dispatch) = stage.dispatch.clone() else {
                self.finish_permanent(
                    pipeline_id,
                    stage_id,
                    "stage has no executable dispatch target".to_string(),
                )
                .await;
                continue;
            };

            let future = {
                let recs = recordings.read().await;
                let ctx = VerbContext {
                    recordings: &recs,
                    plugin_toggles,
                };
                plugins.lock().await.execute_stage(
                    &dispatch.plugin,
                    &dispatch.verb,
                    &dispatch.selection,
                    &dispatch.payload,
                    &ctx,
                )
            };
            let Some(future) = future else {
                self.finish_permanent(
                    pipeline_id,
                    stage_id,
                    format!(
                        "no ready executor for {}:{}",
                        dispatch.plugin, dispatch.verb
                    ),
                )
                .await;
                continue;
            };

            let runtime = self.clone();
            let resources = resources.clone();
            let actions = action_tx.clone();
            tokio::spawn(async move {
                let Some(permits) = runtime
                    .acquire_stage_locks(pipeline_id, stage_id, &stage, &resources)
                    .await
                else {
                    return;
                };

                // Do not drop an opaque executor future on cancellation:
                // `spawn_blocking` media adapters cannot be force-cancelled,
                // and releasing their file permit early would let a retry
                // race the still-running ffmpeg process. Cancellation changes
                // registry state immediately; the guarded completion below
                // discards this late result after the worker drains.
                let result = future.await;
                drop(permits);
                match result {
                    Ok(result) => {
                        let accepted = {
                            let mut registry = runtime.registry.lock().await;
                            registry.complete_stage(pipeline_id, stage_id, result.artifacts)
                        };
                        if accepted {
                            for action in result.actions {
                                let _ = actions.send(action);
                            }
                        }
                        runtime.emit_snapshot(pipeline_id).await;
                        runtime.wake();
                    }
                    Err(error) => {
                        runtime.finish_failure(pipeline_id, stage_id, error).await;
                    }
                }
            });
        }
    }

    /// Acquire every resource lock a stage requires. Cancellation-safe: a
    /// cancelled stage's own token still interrupts the wait immediately
    /// (`tokio::select!` below), same as before F-37. A lock that cannot
    /// be acquired within [`RESOURCE_ACQUIRE_TIMEOUT`] — a wedged holder,
    /// per F-37 — routes the stage through [`Self::finish_failure`], the
    /// same transient/auto-retried failure path used for every other
    /// stage-execution error, with a structured warning naming the
    /// resource and (best-effort) its current holder. Returns `None` on
    /// cancellation or failure; the stage state has already been updated
    /// in both cases, so the caller just bails.
    async fn acquire_stage_locks(
        &self,
        pipeline_id: PipelineId,
        stage_id: StageId,
        stage: &Stage,
        resources: &ResourceRegistry,
    ) -> Option<Vec<tokio::sync::OwnedSemaphorePermit>> {
        // Sort lock keys before acquisition to prevent cross-stage
        // deadlock when two stages request the same set in reverse.
        let mut required = stage.requires.clone();
        required.sort_by_key(|lock| format!("{lock:?}"));
        let mut permits = Vec::with_capacity(required.len());
        for lock in &required {
            let acquired = tokio::select! {
                _ = stage.cancel.cancelled() => return None,
                permit = resources.acquire(lock) => permit,
            };
            match acquired {
                Ok(permit) => permits.push(permit),
                Err(LockAcquireError::Closed) => {
                    self.finish_failure(
                        pipeline_id,
                        stage_id,
                        format!("resource lock closed: {lock:?}"),
                    )
                    .await;
                    return None;
                }
                Err(LockAcquireError::CaptureReserve {
                    free_bytes,
                    reserve_bytes,
                }) => {
                    // Reported through the same state path as `Closed` so the
                    // reason reaches `StageState::Failed` / `last_error`
                    // rather than only the log. It rides the backoff retry
                    // like the other variants, but a run where it never
                    // clears exhausts `max_attempts` instead of looping.
                    tracing::warn!(
                        stage = %stage.name,
                        stage_id = %stage_id,
                        pipeline_id = %pipeline_id,
                        free_bytes,
                        reserve_bytes,
                        "render refused: free space is not above the capture reserve"
                    );
                    self.finish_failure(
                        pipeline_id,
                        stage_id,
                        format!(
                            "render refused: {free_bytes} bytes free is not above the \
                             {reserve_bytes}-byte reserve held for capture"
                        ),
                    )
                    .await;
                    return None;
                }
                Err(LockAcquireError::TimedOut) => {
                    self.fail_lock_timeout(pipeline_id, stage_id, stage, lock)
                        .await;
                    return None;
                }
            }
        }
        Some(permits)
    }

    async fn fail_lock_timeout(
        &self,
        pipeline_id: PipelineId,
        stage_id: StageId,
        stage: &Stage,
        lock: &ResourceLock,
    ) {
        let holder = self.registry.lock().await.find_lock_holder(lock);
        let holder_name = holder
            .as_ref()
            .map(|(_, name)| name.as_str())
            .unwrap_or("unknown");
        tracing::warn!(
            stage = %stage.name,
            stage_id = %stage_id,
            pipeline_id = %pipeline_id,
            resource = ?lock,
            holder_stage = %holder_name,
            holder_stage_id = ?holder.as_ref().map(|(id, _)| *id),
            timeout_secs = RESOURCE_ACQUIRE_TIMEOUT.as_secs(),
            "pipeline stage timed out waiting for resource lock"
        );
        self.finish_failure(
            pipeline_id,
            stage_id,
            format!(
                "timed out after {}s waiting for resource lock {lock:?} (held by: {holder_name})",
                RESOURCE_ACQUIRE_TIMEOUT.as_secs()
            ),
        )
        .await;
    }

    async fn finish_failure(&self, pipeline_id: PipelineId, stage_id: StageId, error: String) {
        let retry = {
            let mut registry = self.registry.lock().await;
            registry.mark_stage_failed(stage_id, error);
            registry
                .get(pipeline_id)
                .and_then(|pipeline| pipeline.stages.iter().find(|stage| stage.id == stage_id))
                .and_then(|stage| match stage.state {
                    StageState::Failed { attempt, .. } => {
                        Some((attempt, super::Stage::backoff_after(attempt)))
                    }
                    _ => None,
                })
        };
        self.emit_snapshot(pipeline_id).await;
        if let Some((_attempt, delay)) = retry {
            let runtime = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                if runtime
                    .registry
                    .lock()
                    .await
                    .retry_failed_stage(stage_id)
                    .is_some()
                {
                    runtime.emit_snapshot(pipeline_id).await;
                    runtime.wake();
                }
            });
        }
    }

    async fn finish_permanent(&self, pipeline_id: PipelineId, stage_id: StageId, error: String) {
        self.registry.lock().await.exhaust_stage(stage_id, error);
        self.emit_snapshot(pipeline_id).await;
    }

    async fn emit_snapshot(&self, pipeline_id: PipelineId) {
        if let Some(pipeline) = self.registry.lock().await.get(pipeline_id).cloned() {
            let _ = self
                .event_tx
                .send(DaemonEvent::PipelineUpdated { pipeline });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{Pipeline, StageKind};
    use std::time::Duration;

    /// Build a `PipelineRuntime` without spawning its background dispatch
    /// loop, so tests can drive `acquire_stage_locks` (and the registry
    /// methods it calls) directly and deterministically. Valid because
    /// `PipelineRuntime`'s fields are only private, not test-inaccessible
    /// — this module is a descendant of `pipeline::runtime`.
    fn test_runtime(
        registry: PipelineRegistry,
    ) -> (PipelineRuntime, mpsc::UnboundedReceiver<DaemonEvent>) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let runtime = PipelineRuntime {
            registry: Arc::new(Mutex::new(registry)),
            wake: Arc::new(Notify::new()),
            event_tx,
        };
        (runtime, event_rx)
    }

    // ---------------------------------------------------------------------
    // Todo 54 — the authored graph *is* the run.
    //
    // These are the properties a rendered graph cannot show: that what the
    // author wrote is what the executor runs, in the author's order, under
    // the author's id — and that nothing between authoring and the persisted
    // history re-derives, re-orders, or re-mints any of it. They live in
    // `core`, not in the web crate, because that is the layer that would have
    // to be wrong for the seam to break, and they must pass in the PVR
    // configuration too, where no editor exists in front of the caller.
    // ---------------------------------------------------------------------

    /// A graph whose stages are mutually distinguishable in all three
    /// dimensions the acceptance names — order, kind, and lock. If two stages
    /// shared a kind *and* a lock *and* an attempt count, a re-ordering
    /// assertion could not fail, and would prove nothing.
    fn authored_graph() -> Pipeline {
        let mut pipeline = Pipeline::new("authored").with_trigger("manual");
        let root = pipeline.add_stage(
            Stage::new(
                "Detect cuepoints",
                StageKind::Custom("cuepoints.extract".into()),
            )
            .with_dispatch(crate::pipeline::StageDispatch::new("cuepoints", "extract"))
            .with_requires(vec![ResourceLock::Cpu { cap: 2 }])
            .with_max_attempts(5),
        );
        let branch = pipeline.add_stage(
            Stage::new("Write captions", StageKind::Subtitle)
                .with_dispatch(crate::pipeline::StageDispatch::new("captions", "export"))
                .with_inputs(vec![root])
                .with_requires(vec![ResourceLock::Gpu])
                .with_max_attempts(2),
        );
        pipeline.add_stage(
            Stage::new("Concat outputs", StageKind::Concat)
                .with_dispatch(crate::pipeline::StageDispatch::new("artifacts", "concat"))
                .with_inputs(vec![root, branch])
                .with_requires(vec![ResourceLock::Api {
                    name: "openai".into(),
                    cap: 7,
                }])
                .with_max_attempts(1),
        );
        pipeline
    }

    /// One row per stage, **in order**: `(stage id, kind label, lock labels,
    /// max attempts, dispatch target)`.
    ///
    /// A `Vec`, deliberately: a `BTreeSet` or a sorted comparison would accept
    /// any permutation, and this todo's load-bearing word is *order*. Every
    /// element is a value read from the model, not a projection, so a layer
    /// that rebuilt the stages would show up as a difference.
    fn shape(pipeline: &Pipeline) -> Vec<(String, String, Vec<String>, u8, String)> {
        pipeline
            .stages
            .iter()
            .map(|stage| {
                (
                    stage.id.to_string(),
                    stage.kind.label(),
                    stage.requires.iter().map(|l| l.label()).collect(),
                    stage.max_attempts,
                    stage.dispatch.as_ref().map_or_else(
                        || "none".to_string(),
                        |d| format!("{}/{}", d.plugin, d.verb),
                    ),
                )
            })
            .collect()
    }

    /// The persisted registry, read exactly the way
    /// `GET /api/v1/pipelines/runs` reads it: the `Vec<Pipeline>` the daemon
    /// wrote to `pipelines.json`. Reading the file rather than the
    /// in-memory registry is what makes this the *history* claim.
    fn history(path: &std::path::Path) -> Vec<Pipeline> {
        let bytes = std::fs::read(path).expect("the registry persisted on submit");
        serde_json::from_slice(&bytes).expect("history decodes")
    }

    fn t54_registry_file(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("strivo-t54-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("pipelines.json")
    }

    /// (e) THE test. Author a graph, submit it, and read the id back at all
    /// three points — the author, the executor's registry, and the persisted
    /// history — plus the stage list in order, with kinds and locks.
    #[tokio::test]
    async fn an_authored_graph_is_the_run_with_its_identity_intact() {
        let path = t54_registry_file("identity");
        let (runtime, _rx) = test_runtime(PipelineRegistry::open(&path).expect("open registry"));

        let graph = authored_graph();
        let authored_id = graph.id;
        let authored = shape(&graph);
        assert_eq!(authored.len(), 3);

        let run_id = runtime.submit(graph.clone()).await.expect("accepted");
        assert_eq!(
            run_id, authored_id,
            "the id the executor returns is the id the author minted"
        );

        let in_flight = runtime
            .registry
            .lock()
            .await
            .get(run_id)
            .expect("run registered")
            .clone();
        assert_eq!(in_flight.id, authored_id, "the run carries the authored id");
        assert_eq!(
            shape(&in_flight),
            authored,
            "the run's stages ARE the authored nodes, in the authored order, with the authored kinds and locks"
        );

        let runs = history(&path);
        let recorded = runs
            .iter()
            .find(|run| run.id == authored_id)
            .expect("history records the authored id");
        assert_eq!(
            shape(recorded),
            authored,
            "history holds the same ordered stages, not a re-derivation"
        );
        assert_eq!(recorded.subject_id, graph.subject_id);
        assert_eq!(recorded.trigger, "manual");
        assert_eq!(runs.len(), 1, "one submission is one run");
    }

    /// Falsifiability, made mechanical. The assertion above compares two
    /// ordered vectors, so it is only meaningful if a re-ordering actually
    /// changes them. This proves that on this very fixture: a submission path
    /// that reversed the stage list — the defect this todo names — is
    /// detected rather than absorbed.
    #[test]
    fn the_stage_comparison_can_fail_on_a_reordered_graph() {
        let graph = authored_graph();
        let mut reordered = graph.clone();
        reordered.stages.reverse();
        assert_ne!(
            shape(&graph),
            shape(&reordered),
            "a reordered graph must not compare equal, or the order assertion is vacuous"
        );
        assert_eq!(shape(&graph)[0].1, "custom(cuepoints.extract)");
        assert_eq!(shape(&reordered)[0].1, "concat");
    }

    /// (f) The QA failure scenario: a graph edited *after* submission must not
    /// reach the run. Proves the submit path takes a snapshot rather than a
    /// shared handle — a `Rc`/`Arc` seam or a stored reference would let a
    /// later edit mutate a run already in flight.
    #[tokio::test]
    async fn editing_the_authors_copy_after_submission_does_not_touch_the_run() {
        let path = t54_registry_file("isolation");
        let (runtime, _rx) = test_runtime(PipelineRegistry::open(&path).expect("open registry"));

        let mut graph = authored_graph();
        let submitted = shape(&graph);
        let run_id = runtime.submit(graph.clone()).await.expect("accepted");

        graph.stages[0].name = "Renamed after submit".into();
        graph.stages[1].requires = vec![ResourceLock::Disk { cap: 9 }];
        graph.stages[1].max_attempts = 1;
        let tail = graph.stages[2].id;
        graph.stages[0].inputs.push(tail);
        graph.add_stage(Stage::new("Late arrival", StageKind::Archive));
        graph.name = "Renamed pipeline".into();

        let running = runtime
            .registry
            .lock()
            .await
            .get(run_id)
            .expect("run registered")
            .clone();
        assert_eq!(
            shape(&running),
            submitted,
            "the run keeps the graph as submitted"
        );
        assert_eq!(running.name, "authored", "the name too");
        assert_eq!(
            running.stages.len(),
            3,
            "a stage added later is not a stage of the run"
        );

        let runs = history(&path);
        assert_eq!(runs.len(), 1);
        assert_eq!(
            shape(&runs[0]),
            submitted,
            "and history agrees with the run"
        );
    }

    /// (g) No layer re-mints identity, which has a second consequence worth
    /// stating: the same graph cannot become a second run, and the refusal is
    /// the authored id, not a fresh one. Re-running is the author's call —
    /// they mint a new id for a new run.
    #[tokio::test]
    async fn identity_is_never_re_minted_so_the_same_graph_is_not_a_second_run() {
        let path = t54_registry_file("identity-once");
        let (runtime, _rx) = test_runtime(PipelineRegistry::open(&path).expect("open registry"));

        let graph = authored_graph();
        let authored_id = graph.id;
        assert_eq!(
            runtime.submit(graph.clone()).await.expect("first"),
            authored_id
        );

        let second = runtime
            .submit(graph.clone())
            .await
            .expect_err("the same id is not a second run");
        assert_eq!(second, "pipeline id already exists");
        let runs = history(&path);
        assert_eq!(runs.len(), 1, "the refusal wrote nothing");
        assert_eq!(runs[0].id, authored_id);

        // A fresh author-minted id is the author's own call — but while the
        // first run is still active the registry deliberately coalesces an
        // equivalent workflow, so the id alone is not a licence to duplicate.
        // Asserted rather than worked around: it is a real contract.
        let mut rerun = graph;
        rerun.id = uuid::Uuid::new_v4();
        let rerun_id = rerun.id;
        assert_ne!(rerun_id, authored_id);
        assert_eq!(
            runtime
                .submit(rerun.clone())
                .await
                .expect_err("coalesced while active"),
            "an equivalent pipeline is already active for this subject"
        );
        assert_eq!(
            history(&path).len(),
            1,
            "the coalesce refusal wrote nothing"
        );

        // Once the first run is terminal the same author-minted id is accepted,
        // and history then holds both ids — the author minted both, and no
        // layer changed either.
        runtime.cancel(authored_id).await;
        runtime
            .submit(rerun)
            .await
            .expect("a re-run once the first is terminal");
        let runs = history(&path);
        let ids: Vec<_> = runs.iter().map(|r| r.id).collect();
        assert!(
            ids.contains(&authored_id) && ids.contains(&rerun_id),
            "{ids:?}"
        );
        assert_eq!(runs.len(), 2);
    }

    /// (a) A stage that cannot get a permit within the timeout fails with
    /// the existing transient failure class (`Failed { attempt }`, the
    /// same one `finish_failure` uses for every other stage-execution
    /// error) — and the stage actually holding the permit is untouched.
    #[tokio::test]
    async fn lock_timeout_fails_stage_transiently_holder_unaffected() {
        let mut pr = PipelineRegistry::new();
        let mut pipeline = Pipeline::new("contention");
        let holder_id = pipeline.add_stage(
            Stage::new("Transcribe", StageKind::Extract).with_requires(vec![ResourceLock::Gpu]),
        );
        let waiter_id = pipeline.add_stage(
            Stage::new("Diarize", StageKind::Extract).with_requires(vec![ResourceLock::Gpu]),
        );
        let pipeline_id = pr.submit(pipeline).unwrap();
        pr.mark_stage_running(pipeline_id, holder_id);
        pr.mark_stage_running(pipeline_id, waiter_id);
        let waiter = pr.get(pipeline_id).unwrap().stages[1].clone();
        assert_eq!(waiter.id, waiter_id);

        let (runtime, _rx) = test_runtime(pr);
        let resources = ResourceRegistry::with_timeout(Duration::from_millis(50));
        // Simulate the holder stage actually owning the GPU permit.
        let _held = resources.acquire(&ResourceLock::Gpu).await.unwrap();

        let outcome = runtime
            .acquire_stage_locks(pipeline_id, waiter_id, &waiter, &resources)
            .await;
        assert!(
            outcome.is_none(),
            "acquisition must fail once the timeout elapses"
        );

        let registry = runtime.registry.lock().await;
        let pipeline = registry.get(pipeline_id).unwrap();
        let waiter_after = pipeline.stages.iter().find(|s| s.id == waiter_id).unwrap();
        assert!(
            matches!(waiter_after.state, StageState::Failed { attempt: 1, .. }),
            "expected transient Failed{{attempt:1}}, got {:?}",
            waiter_after.state
        );
        let holder_after = pipeline.stages.iter().find(|s| s.id == holder_id).unwrap();
        assert!(
            matches!(holder_after.state, StageState::Running { .. }),
            "the permit-holder stage must be unaffected by a sibling's timeout"
        );
    }

    /// (b) A stage that timed out retries per the existing backoff policy
    /// (`Stage::backoff_after`) and succeeds once the permit frees.
    #[tokio::test]
    async fn lock_timeout_retries_via_backoff_and_succeeds_once_free() {
        let mut pr = PipelineRegistry::new();
        let mut pipeline = Pipeline::new("retry after contention");
        let waiter_id = pipeline.add_stage(
            Stage::new("waiter", StageKind::Extract).with_requires(vec![ResourceLock::Gpu]),
        );
        let pipeline_id = pr.submit(pipeline).unwrap();
        pr.mark_stage_running(pipeline_id, waiter_id);
        let waiter = pr.get(pipeline_id).unwrap().stages[0].clone();

        let (runtime, _rx) = test_runtime(pr);
        let resources = ResourceRegistry::with_timeout(Duration::from_millis(50));
        let held = resources.acquire(&ResourceLock::Gpu).await.unwrap();

        let outcome = runtime
            .acquire_stage_locks(pipeline_id, waiter_id, &waiter, &resources)
            .await;
        assert!(outcome.is_none());
        {
            let registry = runtime.registry.lock().await;
            let stage = registry.get(pipeline_id).unwrap().stages[0].clone();
            assert!(matches!(stage.state, StageState::Failed { attempt: 1, .. }));
        }

        // Free the resource before the scheduled backoff retry fires
        // (`Stage::backoff_after(1)` == 5s).
        drop(held);
        tokio::time::sleep(Duration::from_secs(6)).await;
        {
            let registry = runtime.registry.lock().await;
            let stage = registry.get(pipeline_id).unwrap().stages[0].clone();
            assert!(
                matches!(stage.state, StageState::Pending),
                "finish_failure's backoff task must requeue the stage automatically, got {:?}",
                stage.state
            );
        }

        // The real dispatch loop would re-mark it Running and re-dispatch;
        // do that step explicitly here and prove the retry now acquires.
        runtime
            .registry
            .lock()
            .await
            .mark_stage_running(pipeline_id, waiter_id);
        let waiter = runtime
            .registry
            .lock()
            .await
            .get(pipeline_id)
            .unwrap()
            .stages[0]
            .clone();
        let retried = runtime
            .acquire_stage_locks(pipeline_id, waiter_id, &waiter, &resources)
            .await;
        assert!(
            retried.is_some(),
            "retry must succeed once the permit is free"
        );
    }

    /// (c) Cancellation during the acquisition wait aborts promptly and
    /// leaks no permit.
    #[tokio::test]
    async fn cancellation_during_wait_aborts_promptly_without_leaking_permit() {
        let mut pr = PipelineRegistry::new();
        let mut pipeline = Pipeline::new("cancel during wait");
        let stage_id = pipeline.add_stage(
            Stage::new("waiter", StageKind::Extract).with_requires(vec![ResourceLock::Gpu]),
        );
        let pipeline_id = pr.submit(pipeline).unwrap();
        pr.mark_stage_running(pipeline_id, stage_id);
        let stage = pr.get(pipeline_id).unwrap().stages[0].clone();

        let (runtime, _rx) = test_runtime(pr);
        // A long timeout: if cancellation didn't interrupt the wait, this
        // test would hang for 30s instead of failing fast.
        let resources = ResourceRegistry::with_timeout(Duration::from_secs(30));
        let held = resources.acquire(&ResourceLock::Gpu).await.unwrap();

        let task_runtime = runtime.clone();
        let task_resources = resources.clone();
        let handle = tokio::spawn(async move {
            task_runtime
                .acquire_stage_locks(pipeline_id, stage_id, &stage, &task_resources)
                .await
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        // Real cancellation path: cancels the registry's stored token,
        // which is linked to the clone the spawned task is waiting on.
        runtime.cancel(pipeline_id).await;

        let result = tokio::time::timeout(Duration::from_millis(500), handle)
            .await
            .expect("cancellation must interrupt the wait promptly, not wait out the timeout")
            .expect("task must not panic");
        assert!(result.is_none());

        // No permit leaked: dropping the real holder and re-acquiring
        // must succeed immediately.
        drop(held);
        let reacquired = tokio::time::timeout(
            Duration::from_millis(200),
            resources.acquire(&ResourceLock::Gpu),
        )
        .await;
        assert!(
            matches!(reacquired, Ok(Ok(_))),
            "a leaked permit would make this hang or fail"
        );
    }

    /// (d) Normal contention below the timeout threshold acquires fine —
    /// a briefly-held permit frees before the timeout and the waiting
    /// stage is never failed.
    #[tokio::test]
    async fn normal_contention_below_timeout_acquires_without_failing() {
        let mut pr = PipelineRegistry::new();
        let mut pipeline = Pipeline::new("normal contention");
        let waiter_id = pipeline.add_stage(
            Stage::new("waiter", StageKind::Extract).with_requires(vec![ResourceLock::Gpu]),
        );
        let pipeline_id = pr.submit(pipeline).unwrap();
        pr.mark_stage_running(pipeline_id, waiter_id);
        let waiter = pr.get(pipeline_id).unwrap().stages[0].clone();

        let (runtime, _rx) = test_runtime(pr);
        let resources = ResourceRegistry::with_timeout(Duration::from_millis(500));
        let holder = resources.acquire(&ResourceLock::Gpu).await.unwrap();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            drop(holder);
        });

        let permits = runtime
            .acquire_stage_locks(pipeline_id, waiter_id, &waiter, &resources)
            .await;
        assert!(
            permits.is_some(),
            "contention well under the timeout must not fail the stage"
        );

        let registry = runtime.registry.lock().await;
        let waiter_after = registry.get(pipeline_id).unwrap().stages[0].clone();
        assert!(matches!(waiter_after.state, StageState::Running { .. }));
    }
}
