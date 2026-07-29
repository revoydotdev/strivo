//! Insights — data-viz over the Crunchr transcript corpus.
//!
//! Reads Crunchr's existing `word_frequency` / `segments` /
//! `video_analysis` tables and surfaces them as histograms and exports.
//! Was a TUI-rendered pane; with the TUI deletion the plugin is now a
//! headless registration shell — the webui reads the same tables
//! directly via `strivo-web/src/routes/plugins.rs`.

use std::any::Any;
use std::path::PathBuf;

use strivo_core::events::DaemonEvent;
use strivo_core::plugin::{Plugin, PluginAction, PluginContext, StatusSlot};

pub mod export;
pub mod frequency;
pub mod speakers;
pub mod topics;

pub struct InsightsPlugin {
    db_path: PathBuf,
    last_status: Option<String>,
}

impl Default for InsightsPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl InsightsPlugin {
    pub fn new() -> Self {
        Self {
            db_path: PathBuf::new(),
            last_status: None,
        }
    }
}

impl Plugin for InsightsPlugin {
    fn name(&self) -> &'static str {
        "insights"
    }
    fn display_name(&self) -> &str {
        "Insights"
    }

    fn init(&mut self, ctx: &PluginContext) -> anyhow::Result<()> {
        self.db_path = ctx
            .data_dir
            .parent()
            .unwrap_or(&ctx.data_dir)
            .join("crunchr")
            .join("crunchr.db");
        Ok(())
    }

    fn on_event(
        &mut self,
        _event: &DaemonEvent,
        _ctx: &strivo_core::plugin::VerbContext,
    ) -> Vec<PluginAction> {
        Vec::new()
    }

    fn status_line(&self) -> Option<String> {
        self.last_status.clone()
    }
    fn status_slot(&self) -> StatusSlot {
        StatusSlot::Tray
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
