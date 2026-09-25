//! First-party plugins for StriVo.
//!
//! - [`crunchr`] — transcription + diarization + analysis. Backends: Voxtral via OpenRouter (default),
//!   Mistral direct, WhisperX/pyannote (self-hosted GPU), self-hosted Voxtral, Whisper CLI. Speaker
//!   Editor modal for per-recording label edits; SRT/VTT export; optional `mkvmerge` soft-sub embed.
//! - [`archiver`] — recording organization + gallery rendering

#![allow(clippy::field_reassign_with_default)]
#![allow(clippy::type_complexity)]
#![allow(clippy::too_many_arguments)]

pub mod archiver;
pub mod artifacts;
pub mod crunchr;
pub mod dirs;
pub mod editor;
pub mod insights;
pub mod pipeline_templates;
pub mod viewguard;

use strivo_core::plugin::registry::PluginRegistry;

/// The single registration list for the first-party plugins. The daemon
/// builds its executor registry from this, and any process that has to
/// answer "which capability has an executor" builds the same registry from
/// it — a second, divergent list would let a surface claim a capability is
/// runnable while the executor that would run it is not registered.
pub fn register_all(registry: &mut PluginRegistry) {
    registry.register(Box::new(crunchr::CrunchrPlugin::new()));
    registry.register(Box::new(artifacts::ArtifactPlugin::new()));
    registry.register(Box::new(archiver::ArchiverPlugin::new()));
    registry.register(Box::new(insights::InsightsPlugin::new()));
    registry.register(Box::new(editor::EditorPlugin::new()));
    registry.register(Box::new(viewguard::ViewguardPlugin::new()));
}
