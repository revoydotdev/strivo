pub mod registry;

use std::any::Any;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use crate::config::AppConfig;
use crate::events::DaemonEvent;

/// Unique identifier for a plugin-contributed pane.
pub type PaneId = &'static str;

/// Item types the actions popup knows about. Plugins use this to
/// scope verbs to "act on a selected recording" / "act on a transcript
/// row" etc. (D5+X5.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Recording,
    Transcript,
    Clip,
}

/// Where a plugin command applies. (D5+X5.)
///
/// - `Global` is the historical default — a global keybinding the
///   plugin owns. Kept for back-compat with existing Crunchr / Archiver
///   commands.
/// - `Pane` scopes the command to a specific plugin pane.
/// - `Item` registers the command as a *verb* in the actions popup,
///   so pressing `a` on the focused item type surfaces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PluginCommandScope {
    #[default]
    Global,
    Pane(PaneId),
    Item(ItemKind),
}

/// A command that a plugin registers. The host surfaces these by name
/// and description in the SPA's plugin hub. Previously carried
/// `crossterm` key + modifier fields for the TUI's keybinding system;
/// those were retired with the TUI deletion.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PluginCommand {
    pub name: &'static str,
    pub description: &'static str,
    #[doc(hidden)]
    pub scope: PluginCommandScope,
}

impl PluginCommand {
    pub const fn new(name: &'static str, description: &'static str) -> Self {
        Self {
            name,
            description,
            scope: PluginCommandScope::Global,
        }
    }

    /// Register a verb against an item type. The host surfaces it in
    /// the actions popup for items of that kind.
    pub const fn item(name: &'static str, description: &'static str, kind: ItemKind) -> Self {
        Self {
            name,
            description,
            scope: PluginCommandScope::Item(kind),
        }
    }
}

/// Actions a plugin can request the host to perform.
#[allow(dead_code)]
pub enum PluginAction {
    /// Update the status bar message.
    SetStatus(String),
    /// Send a desktop notification.
    Notify { title: String, body: String },
    /// Navigate to this plugin's pane.
    ActivatePane(PaneId),
    /// Navigate back to sidebar (deactivate plugin pane).
    NavigateBack,
    /// Spawn an async task; results delivered back via on_plugin_event.
    SpawnTask {
        plugin_name: &'static str,
        future: Pin<Box<dyn Future<Output = Box<dyn Any + Send>> + Send>>,
    },
    /// Play a file in mpv.
    PlayFile(PathBuf),
    /// Play a file in mpv starting at a position (seconds). M5.2 —
    /// transcript-scoped seek: Enter on a Crunchr chunk hands the
    /// chunk's start_sec along with the recording path.
    PlayFileAt(PathBuf, f64),
    /// Request the host to update a plugin's config section and persist to disk.
    UpdateConfig {
        plugin_name: &'static str,
        config_update: Box<dyn Any + Send>,
    },
    /// Submit a Pipeline to the host registry. The host applies
    /// `PipelineRegistry::submit` so the DAG overlay (Shift+D),
    /// `:batches` palette scope, and retry/skip/cancel verbs see the
    /// plugin's work. (C1 phase 2.)
    SubmitPipeline(crate::pipeline::Pipeline),
    /// Mirror a stage state change into the host registry. Plugins
    /// call this on their internal state machine's transitions so
    /// the DAG overlay tracks their in-flight work. (C1 phase 2.)
    UpdateStage {
        stage_id: crate::pipeline::StageId,
        new_state: PipelineStageUpdate,
    },
}

/// Subset of [`crate::pipeline::StageState`] transitions a plugin can
/// drive. Restricted vs the full state enum so plugins can't put a
/// stage into states only the host executor should own (e.g.
/// `Running { started_at_ms }` — the host stamps that).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum PipelineStageUpdate {
    Running,
    Done,
    Failed(String),
    Cancelled,
    Skipped,
}

/// Context provided to plugins during initialization.
pub struct PluginContext<'a> {
    pub config: &'a AppConfig,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
}

/// Plugin manifest schema (M4.4 — yazi audit §5 adapt).
///
/// User-discoverable description of a plugin. Dropped into
/// `~/.config/strivo/plugins/<name>.toml` and scanned at startup by
/// [`scan_user_plugins`]. Today the manifest is informational only —
/// surfaced in the Settings tab so users can audit what's installed.
/// Dynamic loading of out-of-tree Rust plugins (cdylib + libloading)
/// is a separate piece of work tracked in the M4 polish bucket.
///
/// Example:
///
/// ```toml
/// name = "scratchpad"
/// version = "0.1.0"
/// description = "Quick-notes scratchpad pinned to F2"
/// activation_key = "F2"
/// pane = "right"
/// ```
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PluginManifest {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Suggested activation key, e.g. `F2` or `<C-x>`. The TUI keymap
    /// table doesn't bind this automatically yet — see audit follow-up.
    ///
    /// **Deprecated in favor of `activation_letter`** for the `,`
    /// plugin-leader namespace. Set `activation_letter` instead and the
    /// plugin will land under `,<letter>` without colliding with global
    /// bindings.
    #[serde(default)]
    pub activation_key: Option<String>,
    /// Single letter that activates the plugin under the `,` leader
    /// (e.g. `activation_letter = "c"` → `,c`). Preferred over
    /// `activation_key`; future versions will warn when both are set.
    #[serde(default)]
    pub activation_letter: Option<String>,
    /// Where the plugin would prefer to render: "right" (Detail pane
    /// replacement), "overlay", or "statusbar".
    #[serde(default)]
    pub pane: Option<String>,
    /// Path to a future dynamic library (cdylib). Recognized but not
    /// loaded today; reserves the field shape.
    #[serde(default)]
    pub library_path: Option<std::path::PathBuf>,
    /// Path the manifest was loaded from (set by `scan_user_plugins`).
    #[serde(skip)]
    pub manifest_path: Option<std::path::PathBuf>,
}

/// Scan a directory for `*.toml` plugin manifests. Each successfully
/// parsed file becomes a [`PluginManifest`]; parse errors are logged
/// and skipped so a broken manifest doesn't block startup.
pub fn scan_user_plugins(dir: &std::path::Path) -> Vec<PluginManifest> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match toml::from_str::<PluginManifest>(&text) {
            Ok(mut m) => {
                m.manifest_path = Some(path.clone());
                out.push(m);
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "plugin manifest parse failed");
            }
        }
    }
    audit_manifest_conflicts(&out);
    out
}

/// Walk loaded manifests and warn when an `activation_key` collides
/// with the base keymap table (M4.follow.c). Surfaces the user-facing
/// issue at startup rather than silently shadowing the binding —
/// users who notice the warning in the log can pick a different key
/// before they're confused at runtime.
fn audit_manifest_conflicts(manifests: &[PluginManifest]) {
    // Activation-letter collisions inside the `,` namespace. Two
    // plugins claiming `,c` is a hard configuration error the user
    // can fix by renaming one.
    let mut letters_seen: std::collections::HashMap<char, &str> = std::collections::HashMap::new();
    for m in manifests {
        let Some(ref l) = m.activation_letter else {
            continue;
        };
        if let Some(ch) = l.chars().next().filter(|_| l.chars().count() == 1) {
            if let Some(prev) = letters_seen.insert(ch, &m.name) {
                tracing::warn!(
                    plugin = %m.name,
                    conflicts_with = %prev,
                    letter = %ch,
                    "plugin activation_letter collides — only the first registered will fire",
                );
            }
        } else {
            tracing::warn!(
                plugin = %m.name,
                value = %l,
                "plugin activation_letter must be exactly one character",
            );
        }
    }
    for m in manifests {
        if m.activation_key.is_some() && m.activation_letter.is_none() {
            tracing::info!(
                plugin = %m.name,
                "plugin uses deprecated activation_key; please migrate to activation_letter (',X' namespace)",
            );
        }
        // Legacy `activation_key` collision check against the TUI
        // keymap was retired with `src/tui/`. The webui plugin host
        // surfaces activation by name; a new collision check can
        // land if/when a webui-side keybinding system is introduced.
    }
}

/// Default directory `~/.config/strivo/plugins/` where user plugin
/// manifests live. Created on first access only if a write would
/// follow — scanning gracefully no-ops on a missing directory.
pub fn user_plugin_dir() -> std::path::PathBuf {
    crate::config::AppConfig::config_dir().join("plugins")
}

/// Symbol every dynamic plugin cdylib must export. The host calls it
/// once and takes ownership of the returned `Box<dyn Plugin>`.
///
/// The signature is `unsafe extern "C" fn() -> *mut Box<dyn Plugin>`.
/// We return `*mut Box<dyn Plugin>` (a thin pointer at the outer level
/// — the inner `Box<dyn Plugin>` holds the fat pointer with the
/// vtable). Plugin authors:
///
/// ```ignore
/// #[no_mangle]
/// pub extern "C" fn _strivo_plugin_register() -> *mut std::ffi::c_void {
///     let plugin: Box<dyn strivo_core::plugin::Plugin> = Box::new(MyPlugin::new());
///     Box::into_raw(Box::new(plugin)) as *mut std::ffi::c_void
/// }
/// ```
pub const PLUGIN_REGISTER_SYMBOL: &[u8] = b"_strivo_plugin_register";

/// Outcome of [`load_dylib_plugin`]. The caller must keep `library`
/// alive at least as long as the boxed plugin — dlopen/LoadLibrary
/// vtables live in the loaded image.
pub struct LoadedDylibPlugin {
    pub plugin: Box<dyn Plugin>,
    pub library: libloading::Library,
}

/// Load a single dynamic plugin from `path` (a .so / .dylib / .dll).
///
/// SAFETY: Caller MUST guarantee the cdylib was compiled against the
/// same strivo-core build as the host. Rust dyn-trait vtables are
/// only deterministic under matching-toolchain, matching-deps
/// compilation. Mismatch → undefined behavior.
pub fn load_dylib_plugin(path: &std::path::Path) -> anyhow::Result<LoadedDylibPlugin> {
    if !path.exists() {
        anyhow::bail!("plugin library does not exist: {}", path.display());
    }
    // SAFETY: dlopen on an arbitrary path is unsafe by construction;
    // we wrap it for the caller and document the matching-toolchain
    // contract at the docs-comment level.
    let library = unsafe {
        libloading::Library::new(path)
            .map_err(|e| anyhow::anyhow!("dlopen {}: {e}", path.display()))?
    };
    let plugin: Box<dyn Plugin> = unsafe {
        let symbol: libloading::Symbol<unsafe extern "C" fn() -> *mut std::ffi::c_void> =
            library.get(PLUGIN_REGISTER_SYMBOL).map_err(|e| {
                anyhow::anyhow!(
                    "{} missing symbol {}: {e}",
                    path.display(),
                    std::str::from_utf8(PLUGIN_REGISTER_SYMBOL).unwrap_or("?"),
                )
            })?;
        let raw = symbol();
        if raw.is_null() {
            anyhow::bail!("{} register returned null", path.display());
        }
        let outer: Box<Box<dyn Plugin>> = Box::from_raw(raw as *mut Box<dyn Plugin>);
        *outer
    };
    Ok(LoadedDylibPlugin { plugin, library })
}

/// Fieldless mirror of DaemonEvent for event filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonEventKind {
    ChannelsUpdated,
    ChannelWentLive,
    ChannelWentOffline,
    StreamUrlResolved,
    RecordingStarted,
    RecordingProgress,
    RecordingFinished,
    Notification,
    AllRecordingsStopped,
    RecordingsPruned,
    DeviceCodeRequired,
    PlatformAuthenticated,
    PatreonPostFound,
    PatreonState,
    BulkProgress,
    PlaylistList,
    ChannelVods,
    ChannelResolved,
    ScheduleFired,
    Error,
    PipelineStageChanged,
}

impl DaemonEventKind {
    pub fn from_event(event: &DaemonEvent) -> Self {
        match event {
            DaemonEvent::ChannelsUpdated(_) => Self::ChannelsUpdated,
            DaemonEvent::ChannelWentLive(_) => Self::ChannelWentLive,
            DaemonEvent::ChannelWentOffline(_) => Self::ChannelWentOffline,
            DaemonEvent::StreamUrlResolved { .. } => Self::StreamUrlResolved,
            DaemonEvent::RecordingStarted { .. } => Self::RecordingStarted,
            DaemonEvent::RecordingProgress { .. } => Self::RecordingProgress,
            DaemonEvent::RecordingFinished { .. } => Self::RecordingFinished,
            DaemonEvent::Notification { .. } => Self::Notification,
            DaemonEvent::AllRecordingsStopped => Self::AllRecordingsStopped,
            DaemonEvent::RecordingsPruned { .. } => Self::RecordingsPruned,
            DaemonEvent::DeviceCodeRequired { .. } => Self::DeviceCodeRequired,
            DaemonEvent::PlatformAuthenticated { .. } => Self::PlatformAuthenticated,
            DaemonEvent::PatreonPostFound { .. } => Self::PatreonPostFound,
            DaemonEvent::PatreonState { .. } => Self::PatreonState,
            DaemonEvent::BulkProgress { .. } => Self::BulkProgress,
            DaemonEvent::PlaylistList { .. } => Self::PlaylistList,
            DaemonEvent::ChannelVods { .. } => Self::ChannelVods,
            DaemonEvent::ChannelResolved { .. } => Self::ChannelResolved,
            DaemonEvent::ScheduleFired { .. } => Self::ScheduleFired,
            DaemonEvent::Error(_) => Self::Error,
            DaemonEvent::PipelineStageChanged { .. } => Self::PipelineStageChanged,
        }
    }
}

/// The core Plugin trait. All plugins implement this.
#[allow(dead_code, unused)]
/// Where a plugin's `status_line` is rendered. See [`Plugin::status_slot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusSlot {
    /// Right-aligned `[chip]` next to platform indicators. Capped at 3
    /// concurrent chips; overflow collapses into `[+N]`.
    Tray,
    /// Transient one-row banner above the hotkey strip. Reserved for the
    /// telemetry strip work in M4; currently treated as `Tray`.
    Banner,
    /// Do not render this plugin's `status_line`. Useful when the line is
    /// only consumed by other systems (logs, properties modal).
    None,
}

/// Minimal context for plugin verb + event dispatch. `on_event` /
/// `on_verb` only ever need the recording table to resolve UUIDs and
/// the plugin toggles to honour per-plugin disable. Both the headless
/// daemon and the (legacy) TUI build this from their respective state.
pub struct VerbContext<'a> {
    pub recordings: &'a std::collections::HashMap<uuid::Uuid, crate::recording::job::RecordingJob>,
    pub plugin_toggles: &'a std::collections::BTreeMap<String, crate::config::PluginToggle>,
}

pub trait Plugin: Send {
    /// Unique name for this plugin (e.g., "crunchr").
    fn name(&self) -> &'static str;

    /// Human-readable display name.
    fn display_name(&self) -> &str;

    /// Called once after registration.
    fn init(&mut self, ctx: &PluginContext) -> anyhow::Result<()>;

    /// Called on shutdown. Errors are logged by the registry and do not
    /// abort the shutdown of sibling plugins.
    fn shutdown(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Which daemon events this plugin wants to receive. None = all.
    fn event_filter(&self) -> Option<Vec<DaemonEventKind>> {
        None
    }

    /// Handle a daemon event. Return actions for the host to execute.
    ///
    /// Takes [`VerbContext`] so the headless daemon and webui-only host
    /// can dispatch without instantiating TUI state.
    fn on_event(&mut self, _event: &DaemonEvent, _ctx: &VerbContext) -> Vec<PluginAction> {
        Vec::new()
    }

    /// Handle events from the plugin's own async tasks.
    fn on_plugin_event(&mut self, _event: Box<dyn Any + Send>) -> Vec<PluginAction> {
        Vec::new()
    }

    /// Handle an item-scoped verb dispatched from the actions popup
    /// (D5+X5). `verb` is the `PluginCommand.name` the user picked.
    /// `selection` lists the recording UUIDs the verb should act on —
    /// either the multi-select set (if non-empty) or the cursor row.
    /// Default impl ignores; plugins that registered Item-scoped
    /// commands override. (M2.)
    fn on_verb(
        &mut self,
        _verb: &str,
        _selection: &[uuid::Uuid],
        _ctx: &VerbContext,
    ) -> Vec<PluginAction> {
        Vec::new()
    }

    /// Commands this plugin contributes (for help overlay and keybinding dispatch).
    fn commands(&self) -> Vec<PluginCommand> {
        Vec::new()
    }

    /// Optional: contribute a chip-style status string for the host.
    /// Returns plain text; the host (SPA header) decides how to render.
    fn status_line(&self) -> Option<String> {
        None
    }

    /// Where a plugin's `status_line` lives. [`StatusSlot::Tray`] is the
    /// default — right-aligned `[chip]` next to the platform indicators.
    /// [`StatusSlot::Banner`] reserves a transient one-row banner above
    /// the hotkey strip (currently routed back to Tray; banner support
    /// lands with the telemetry strip in M4). [`StatusSlot::None`]
    /// suppresses display, e.g. when the plugin emits status_line for
    /// telemetry/log only.
    fn status_slot(&self) -> StatusSlot {
        StatusSlot::Tray
    }

    /// Downcast support.
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// DAW-vision capability tags this plugin fulfils. Used by the
    /// host to wire cross-plugin pipelines and surface "who provides
    /// what" in the SPA. String-typed so third-party plugins can
    /// declare custom capabilities without trait-API breakage. Well-
    /// known values live as constants on [`capability`]. Default
    /// returns empty so existing plugins keep compiling.
    fn capabilities(&self) -> Vec<&'static str> {
        Vec::new()
    }
}

/// Well-known capability strings. Plugins return these from
/// [`Plugin::capabilities`] so the registry can answer "who
/// provides chapter generation?" — the spine the DAW-for-streaming
/// vision wires every cross-plugin pipeline onto. String constants
/// (not an enum) so third-party plugins can name their own
/// capabilities without trait churn; collisions are merit-of-the
/// matching-string and explicit.
///
/// **Production**: things a plugin emits / writes / catalogs.
/// **Consumption**: things a plugin reads / depends on.
/// Both directions live in the same namespace — the registry
/// computes the bipartite "needs X" / "provides X" graph.
pub mod capability {
    /// Speech-to-text transcript for a recording.
    pub const TRANSCRIPTION: &str = "transcription";
    /// Per-word time offsets (for click-to-seek).
    pub const WORD_TIMESTAMPS: &str = "word_timestamps";
    /// Speaker diarisation labels on the transcript.
    pub const DIARISATION: &str = "diarisation";
    /// Topic segmentation across a transcript.
    pub const TOPIC_SEGMENTATION: &str = "topic_segmentation";
    /// Chapter markers suitable for YouTube/Twitch publishing.
    pub const CHAPTERS: &str = "chapters";
    /// Scene-change cuepoints (visual or audio).
    pub const SCENE_DETECTION: &str = "scene_detection";
    /// Per-frame thumbnail-candidate ranking.
    pub const THUMBNAIL_RANKING: &str = "thumbnail_ranking";
    /// Highlight-segment detection (chat + audio + facecam fusion).
    pub const HIGHLIGHT_DETECTION: &str = "highlight_detection";
    /// Cuts a highlight into a publishable clip file.
    pub const CLIP_EXTRACTION: &str = "clip_extraction";
    /// Machine translation of transcripts / captions.
    pub const TRANSLATION: &str = "translation";
    /// Caption file (.srt / .vtt) generation + export.
    pub const CAPTIONS: &str = "captions";
    /// Audience-retention curve / heatmap source.
    pub const AUDIENCE_RETENTION: &str = "audience_retention";
    /// Viewbot / fraud detection signals.
    pub const FRAUD_DETECTION: &str = "fraud_detection";
    /// Cross-stream comparison + delta surfaces.
    pub const STREAM_COMPARISON: &str = "stream_comparison";
    /// Post-stream Casebook / report writer.
    pub const REPORTING: &str = "reporting";
    /// Content-safety / brand-safety pre-publish gate.
    pub const BRAND_SAFETY: &str = "brand_safety";
    /// Long-term asset catalog of past VODs / clips.
    pub const ASSET_CATALOG: &str = "asset_catalog";
    /// Source-separation (game audio / voice / Discord / music).
    pub const SOURCE_TRACK_SPLIT: &str = "source_track_split";
    /// Cross-format publish queue (YT / Shorts / TikTok / podcast).
    pub const PUBLISH_QUEUE: &str = "publish_queue";
    /// EDL / arrange-view non-destructive editor.
    pub const EDL_EDITOR: &str = "edl_editor";
}
