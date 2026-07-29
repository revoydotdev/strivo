//! Crunchr — Whisper / Voxtral transcription orchestrator.
//!
//! The plugin is a headless trigger shell: the webui's "Re-transcribe" verb
//! (and the tandem auto-trigger) dispatch into [`CrunchrPlugin::on_verb`] /
//! [`on_event`], which spawn the end-to-end [`runner::process_recording`]
//! job (extract → transcribe+diarize → persist → chunk → vectorize). The
//! webui's transcript, search, and analytics surfaces read `crunchr.db`
//! directly.

use std::any::Any;
use std::path::PathBuf;

use strivo_core::config::CrunchrConfig;
use strivo_core::events::DaemonEvent;
use strivo_core::plugin::{
    DaemonEventKind, Plugin, PluginAction, PluginContext, StageExecutionResult, StageFuture,
    StatusSlot, VerbContext,
};
use strivo_core::recording::job::RecordingState;
use uuid::Uuid;

pub mod analysis;
pub mod cluster;
pub mod cost;
pub mod db;
pub mod embed;
pub mod pipeline;
pub mod presets;
pub mod runner;
pub mod transcribe;
pub mod types;
pub mod voice_samples;

pub struct CrunchrPlugin {
    /// Captured from `AppConfig.crunchr` at init. `None` until then, which
    /// is fine because verbs/events only fire after the registry inits.
    cfg: Option<CrunchrConfig>,
    data_dir: PathBuf,
    cache_dir: PathBuf,
    db_path: PathBuf,
    tandem_channels: Vec<String>,
    tandem_playlists: Vec<String>,
    enabled: bool,
}

impl Default for CrunchrPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl CrunchrPlugin {
    pub fn new() -> Self {
        Self {
            cfg: None,
            data_dir: PathBuf::new(),
            cache_dir: PathBuf::new(),
            db_path: PathBuf::new(),
            tandem_channels: Vec::new(),
            tandem_playlists: Vec::new(),
            enabled: true,
        }
    }

    fn transcription_pipeline(&self, id: Uuid, trigger: &str) -> PluginAction {
        PluginAction::SubmitPipeline(strivo_core::pipeline::templates::creator_intelligence(
            id, trigger,
        ))
    }
}

impl Plugin for CrunchrPlugin {
    fn name(&self) -> &'static str {
        "crunchr"
    }
    fn display_name(&self) -> &str {
        "Crunchr"
    }

    fn init(&mut self, ctx: &PluginContext) -> anyhow::Result<()> {
        // `init_all` already scopes data_dir/cache_dir to
        // `<base>/plugins/crunchr`; use them directly (re-nesting here was a
        // latent double-nest bug from the old stub, harmless only because it
        // never wrote). The webui reads `<data>/plugins/crunchr/crunchr.db`.
        self.data_dir = ctx.data_dir.clone();
        std::fs::create_dir_all(&self.data_dir)?;
        self.db_path = self.data_dir.join("crunchr.db");
        self.cache_dir = ctx.cache_dir.clone();
        std::fs::create_dir_all(&self.cache_dir)?;

        let c = &ctx.config.crunchr;
        self.enabled = c.enabled;
        self.tandem_channels = c.tandem_channels.clone();
        self.tandem_playlists = c.tandem_playlists.clone();
        self.cfg = Some(c.clone());

        // Ensure the schema exists so the webui's read routes have a DB to
        // open before the first transcription lands.
        if let Err(e) = db::open_and_init(&self.db_path) {
            tracing::warn!("crunchr: db init failed: {e:#}");
        }
        Ok(())
    }

    fn event_filter(&self) -> Option<Vec<DaemonEventKind>> {
        Some(vec![DaemonEventKind::RecordingFinished])
    }

    fn on_event(&mut self, event: &DaemonEvent, ctx: &VerbContext) -> Vec<PluginAction> {
        if let DaemonEvent::RecordingFinished {
            job_id,
            final_state,
            ..
        } = event
        {
            if *final_state != RecordingState::Finished || !self.enabled {
                return Vec::new();
            }
            if let Some(rec) = ctx.recordings.get(job_id) {
                let channel_key = format!("{}:{}", rec.platform, rec.channel_id);
                let is_tandem = self.tandem_channels.contains(&channel_key)
                    || rec
                        .playlist
                        .as_ref()
                        .is_some_and(|p| self.tandem_playlists.contains(p));

                let crunchr_auto_marker = rec
                    .output_path
                    .parent()
                    .map(|p| p.join(".crunchr-auto"))
                    .map(|m| m.exists())
                    .unwrap_or(false);

                if is_tandem || crunchr_auto_marker {
                    return vec![self.transcription_pipeline(*job_id, "recording_finished")];
                }
            }
        }
        Vec::new()
    }

    fn on_verb(&mut self, verb: &str, selection: &[Uuid], ctx: &VerbContext) -> Vec<PluginAction> {
        if !self.enabled || !matches!(verb, "Re-transcribe" | "transcribe" | "retranscribe") {
            return Vec::new();
        }
        let mut actions = Vec::new();
        for id in selection {
            if ctx.recordings.contains_key(id) {
                actions.push(self.transcription_pipeline(*id, "manual"));
            }
        }
        actions
    }

    fn execute_stage(
        &mut self,
        verb: &str,
        selection: &[Uuid],
        _payload: &serde_json::Value,
        ctx: &VerbContext,
    ) -> Option<StageFuture> {
        if !self.enabled || !matches!(verb, "transcribe" | "retranscribe") {
            return None;
        }
        let id = *selection.first()?;
        let rec = ctx.recordings.get(&id)?;
        let cfg = self.cfg.clone()?;
        let db_path = self.db_path.clone();
        let cache_dir = self.cache_dir.clone();
        let channel_name = rec.channel_name.clone();
        let title = rec
            .stream_title
            .clone()
            .unwrap_or_else(|| channel_name.clone());
        let video_path = rec.output_path.clone();
        Some(Box::pin(async move {
            let outcome = runner::process_recording(
                cfg,
                db_path,
                cache_dir,
                id,
                channel_name,
                title,
                video_path,
            )
            .await;
            match outcome.result {
                Ok(summary) => Ok(StageExecutionResult {
                    artifacts: vec![serde_json::json!({
                        "kind": "transcript",
                        "recording_id": id,
                        "segments": summary.segments,
                        "speakers": summary.speakers,
                        "embedded_chunks": summary.embedded,
                    })],
                    actions: vec![PluginAction::Notify {
                        title: "Crunchr: transcription complete".to_string(),
                        body: format!(
                            "{} segments · {} speakers · {} chunks vectorized",
                            summary.segments, summary.speakers, summary.embedded
                        ),
                    }],
                }),
                Err(error) => Err(error.to_string()),
            }
        }))
    }

    fn on_plugin_event(&mut self, event: Box<dyn Any + Send>) -> Vec<PluginAction> {
        match event.downcast::<runner::RunnerOutcome>() {
            Ok(outcome) => {
                let (title, body) = match &outcome.result {
                    Ok(s) => (
                        "Crunchr: transcription complete".to_string(),
                        format!(
                            "{} — {} segments · {} speakers · {} chunks vectorized",
                            outcome.title, s.segments, s.speakers, s.embedded
                        ),
                    ),
                    Err(e) => (
                        "Crunchr: transcription failed".to_string(),
                        format!("{} — {}", outcome.title, e),
                    ),
                };
                vec![PluginAction::Notify { title, body }]
            }
            Err(_) => Vec::new(),
        }
    }

    fn status_line(&self) -> Option<String> {
        None
    }
    fn status_slot(&self) -> StatusSlot {
        StatusSlot::None
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
