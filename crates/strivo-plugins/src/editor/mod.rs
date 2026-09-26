//! Editor — transcript-driven cut composer.
//!
//! Was a TUI editor (timeline / compilation / filter views). With the
//! TUI deletion the plugin is now a headless registration shell —
//! the webui's editor surface lives under `strivo-web` and operates on
//! the shared EDL types in `strivo_editor`.

use std::any::Any;
use std::path::PathBuf;

use strivo_core::events::DaemonEvent;
use strivo_core::plugin::{Plugin, PluginAction, PluginContext, StatusSlot};

pub mod filter;

/// An EDL segment selected from a transcript view. Was the TUI
/// editor's interaction primitive; preserved here because `filter`
/// still builds on it.
#[derive(Debug, Clone)]
pub struct EditorClip {
    pub in_word: u32,
    pub out_word: u32,
    pub label: String,
}

pub struct EditorPlugin {
    data_dir: PathBuf,
}

impl Default for EditorPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorPlugin {
    pub fn new() -> Self {
        Self {
            data_dir: PathBuf::new(),
        }
    }
}

impl Plugin for EditorPlugin {
    fn name(&self) -> &'static str {
        "editor"
    }
    fn display_name(&self) -> &str {
        "Editor"
    }

    fn init(&mut self, ctx: &PluginContext) -> anyhow::Result<()> {
        self.data_dir = ctx.data_dir.join("plugins").join("editor");
        std::fs::create_dir_all(&self.data_dir)?;
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
