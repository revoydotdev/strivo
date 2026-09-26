pub mod api;
pub mod assets;
// Config/DB backup + restore, and durable recording history — split out of
// `api` alongside the other `/api/v1/*` handler modules below.
pub mod backup;
// Blocklist, per-channel auto-record + alerts, and (Creator Edition)
// archiver tandem/playlist toggles — split out of `api`.
pub mod blocklist;
// Twitch IRC chat's two server-side routes (list rooms, relay an outbound
// send) — core, every-edition functionality, not a Creator/Pro plugin. See
// the module doc for why it isn't folded into `plugins`.
pub mod chat;
// Capture-profile CRUD (quality tiers) — split out of `api`.
pub mod capture_profiles;
pub mod events;
// JSON channel import/export, the Monitor page's unified view, and the
// capability matrix + feature-capability descriptor — split out of `api`.
pub mod import_export;
// Strivo Pro licence routes (status/activate/trial/refresh) — entitlement is
// a Creator Edition product concept (ADR 0002, closing CE06); the PVR
// edition has no notion of "Pro" at all, so this module and its router are
// Creator-only. Compiling it out of a PVR build (rather than leaving it
// mounted-but-inert) keeps licence/tier vocabulary out of the PVR surface
// entirely, matching the monorepo's invisibility bar.
#[cfg(feature = "creator")]
pub mod licence;
pub mod login;
// Proxy-backed preview reads for the recording library: one frame and one
// waveform, both decoded from a content-addressed proxy rather than from the
// recording itself. PVR, not Creator-only: navigating a long recording is core
// library functionality, and the routes take a recording id rather than a
// caller-supplied path, so they need no media-admission gate. See the module
// doc for why that distinction decides the edition.
pub mod preview;
// The media-input admission gate: every ffmpeg-bound media request passes it
// before a child process exists. Creator-only because the only callers are the
// Creator editor and clipper routes — in a PVR build no route hands a
// client-supplied path or range to a media tool, so the gate has nothing to
// guard and compiling it out keeps its vocabulary out of the PVR surface.
#[cfg(feature = "creator")]
pub mod media_input;
// Local file and folder import (report item C01). Creator-only: a PVR install
// has no way to name a file the daemon did not record, and no surface that
// would accept one. Compiling it out rather than mounting it inert keeps
// `import`, `relink` and their vocabulary out of the PVR binary entirely.
#[cfg(feature = "creator")]
pub mod import;
// OBS-style multitrack import (report item C02). Creator-only for the same
// reason as `import`: a PVR install has no project to import a session into, and
// the route spawns a media probe, which is exactly the surface `media_input`
// exists to gate. Compiling it out keeps its vocabulary out of the PVR binary.
#[cfg(feature = "creator")]
pub mod multitrack;
#[cfg(feature = "creator")]
pub mod multitrack_gaps;
#[cfg(feature = "creator")]
pub mod multitrack_probe;
// Multi-stream tile layout for the watch player. Core (single + multi view),
// not a Pro plugin — always compiled so the PVR build's player works.
pub mod multistream;
// First-party plugin routes (Crunchr/Archiver/Insights/Viewguard + the
// recording captions VTT endpoint) — Creator Edition only. The licence
// runtime gate still filters locked Pro plugins for non-entitled clients
// within the edition.
#[cfg(feature = "creator")]
pub mod plugins;
// The pipeline graph, derived from the executor's own `Pipeline`/`Stage`
// model. Creator-only for the same reason as `plugins`: the stages it draws
// are dispatched by Creator plugins, and a PVR install has none registered,
// so the graph would render an empty page in the PVR surface.
#[cfg(feature = "creator")]
pub mod pipeline_graph;
// Authored graph templates: the CRUD the browser editor writes to. Same
// edition boundary and the same reason — the editor authors the Creator
// pipeline model, and a PVR install has none to author.
#[cfg(feature = "creator")]
pub mod pipeline_templates;
// The project workspace: one project, one selected revision, and the same
// answer for every view (player, transcript, timeline, inspector, task status).
// Creator-only because the table it reads is Creator-only — a PVR install has
// no project context to serve, and a mounted-but-inert route would claim
// otherwise.
// The multi-track timeline: one route that applies an edit and answers with
// the compiled plan. Creator-only for the same reason as `workspace` — the
// composition it edits lives in the Creator store, and a PVR install has none.
#[cfg(feature = "creator")]
pub mod timeline;
#[cfg(feature = "creator")]
pub mod workspace;
// Mounted (see `server.rs::build_router`): the sole recording file-serving
// path (download/play) plus the path-containment guard + tests from roadmap
// item 2. The page routers that once sat beside it (channels/dashboard/logs/
// schedule/settings/system) were retired in item 10 — the SPA + /api/v1
// supersede them, but the byte-serving handlers the SPA's player and
// download links depend on did not go with them.
pub mod recordings;
// Health, channels/patreon snapshots, recordings CRUD, schedule, settings +
// platform credentials, storage/gantt gauges, and logs — split out of `api`.
pub mod settings;
pub mod websub;
