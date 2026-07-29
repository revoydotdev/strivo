//! Durable daemon-owned DAG scheduler.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex, Notify, RwLock};
use tokio_util::sync::CancellationToken;

use crate::events::DaemonEvent;
use crate::plugin::registry::PluginRegistry;
use crate::plugin::{PluginAction, VerbContext};
use crate::recording::job::RecordingJob;

use super::{PipelineId, PipelineRegistry, ResourceRegistry, StageId, StageState};

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
            let resources = ResourceRegistry::new();
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
                // Sort lock keys before acquisition to prevent cross-stage
                // deadlock when two stages request the same set in reverse.
                let mut required = stage.requires.clone();
                required.sort_by_key(|lock| format!("{lock:?}"));
                let mut permits = Vec::with_capacity(required.len());
                for lock in &required {
                    match resources.acquire(lock).await {
                        Ok(permit) => permits.push(permit),
                        Err(error) => {
                            runtime
                                .finish_failure(
                                    pipeline_id,
                                    stage_id,
                                    format!("resource lock closed: {error}"),
                                )
                                .await;
                            return;
                        }
                    }
                }

                let result = tokio::select! {
                    _ = stage.cancel.cancelled() => Err("stage cancelled".to_string()),
                    result = future => result,
                };
                drop(permits);
                match result {
                    Ok(result) => {
                        {
                            let mut registry = runtime.registry.lock().await;
                            registry.complete_stage(pipeline_id, stage_id, result.artifacts);
                        }
                        for action in result.actions {
                            let _ = actions.send(action);
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
                    .retry_stage(stage_id, None)
                    .is_some()
                {
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
