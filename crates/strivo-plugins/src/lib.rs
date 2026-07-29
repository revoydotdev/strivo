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
pub mod viewguard;
