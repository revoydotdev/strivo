// Crate-wide clippy allows — see rationale in `main.rs`.
#![allow(clippy::field_reassign_with_default)]
#![allow(clippy::type_complexity)]
#![allow(clippy::too_many_arguments)]

pub fn check_external_tools() {
    for tool in &["ffmpeg", "streamlink", "yt-dlp"] {
        match std::process::Command::new("which")
            .arg(tool)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
        {
            Ok(status) if status.success() => {}
            _ => {
                eprintln!("Warning: '{tool}' not found in PATH. Some features may not work.");
            }
        }
    }
}

pub mod config;
pub mod daemon;
pub mod edl;
pub mod events;
pub mod intents;
pub mod ipc;
pub mod licence;
pub mod media;
pub mod monitor;
pub mod pipeline;
pub mod platform;
pub mod playback;
pub mod plugin;
pub mod recording;
pub mod search;
#[cfg(feature = "creator")]
pub mod signal_store;
pub mod state;
pub mod stream;
pub mod tasks;
pub mod webhook;
