// Crate-wide clippy allows — see rationale in `main.rs`.
#![allow(clippy::field_reassign_with_default)]
#![allow(clippy::type_complexity)]
#![allow(clippy::too_many_arguments)]

pub fn check_external_tools() {
    // Resolve through the `which` crate rather than shelling out to the
    // `which` binary: that binary does not exist on Windows, so the daemon
    // reported every tool as missing there even when all of them were on
    // PATH. The crate also honours %PATHEXT%, so `ffmpeg` resolves to
    // `ffmpeg.exe`, and it saves three process spawns on every start.
    for tool in &["ffmpeg", "streamlink", "yt-dlp"] {
        if which::which(tool).is_err() {
            eprintln!("Warning: '{tool}' not found in PATH. Some features may not work.");
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
pub mod state;
pub mod stream;
pub mod tasks;
pub mod webhook;
