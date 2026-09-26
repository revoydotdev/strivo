// Crate-wide clippy allows — see rationale in `main.rs`.
#![allow(clippy::field_reassign_with_default)]
#![allow(clippy::type_complexity)]
#![allow(clippy::too_many_arguments)]

pub fn check_external_tools() {
    // Resolve the same way the daemon spawns these tools: an exe-relative
    // bundled binary first, then PATH via the `which` crate rather than
    // shelling out to the `which` binary (that binary does not exist on
    // Windows, so the daemon reported every tool as missing there even when
    // all of them were on PATH; the crate also honours %PATHEXT%, so
    // `ffmpeg` resolves to `ffmpeg.exe`, and it saves process spawns).
    for tool in &["ffmpeg", "streamlink", "yt-dlp"] {
        // `resolve_tool` always returns *a* path, falling back to the bare
        // name when nothing was found, so "found" has to be checked
        // explicitly rather than trusting a non-empty result.
        if !tools::resolve_tool(tool).is_file() {
            eprintln!("Warning: '{tool}' not found in PATH. Some features may not work.");
        }
    }
}

pub mod config;
pub mod daemon;
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
pub mod tools;
pub mod webhook;
