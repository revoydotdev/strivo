pub mod api;
pub mod assets;
pub mod events;
pub mod licence;
pub mod login;
// Multi-stream tile layout for the watch player. Core (single + multi view),
// not a Pro plugin — always compiled so the PVR build's player works.
pub mod multistream;
// First-party plugin routes (Crunchr/Archiver/Insights/Viewguard + the
// recording captions VTT endpoint) — Creator Edition only. The licence
// runtime gate still filters locked Pro plugins for non-entitled clients
// within the edition.
#[cfg(feature = "creator")]
pub mod plugins;
// Retained but unmounted: the sole recording file-serving path (download/
// play) plus the path-containment guard + tests from roadmap item 2. The
// legacy htmx page routers (channels/dashboard/logs/schedule/settings/
// system) were retired in item 10 — the SPA + /api/v1 supersede them.
pub mod recordings;
pub mod websub;
