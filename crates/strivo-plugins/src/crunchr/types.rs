use std::collections::HashSet;
use std::path::PathBuf;
use uuid::Uuid;

/// Word-level timing emitted by alignment-capable backends
/// (whisperx_local, voxtral-local in word-mode). Backends that don't
/// produce word timings leave `Segment::words` as `None`; the Editor
/// plugin falls back to segment-level seek when that's the case.
///
/// Persisted as a JSON array on `segments.word_timings`. Compact field
/// names keep the column small in sqlite for long-form transcripts.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WordTiming {
    /// The word as the backend emitted it (no normalization).
    pub w: String,
    /// Start time in seconds relative to the recording origin.
    pub s: f64,
    /// End time in seconds. Always `>= s`.
    pub e: f64,
    /// Optional alignment confidence (0.0–1.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c: Option<f64>,
}

/// Config modal state for the Crunchr plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigModalState {
    /// Modal is not showing.
    Hidden,
    /// Modal is active (first-run or re-opened via 'c').
    Active {
        /// Which form field is currently selected.
        selected_field: usize,
        /// Whether the selected field is in text-edit mode.
        editing: bool,
        /// Total number of static fields (before channel checklist).
        static_field_count: usize,
    },
}

/// One row in the Speaker Editor modal — a unique speaker label on the
/// currently-selected recording, with quick stats and a path to a cached
/// voice-sample clip.
#[derive(Debug, Clone)]
pub struct SpeakerRow {
    /// Original label from the transcription backend (e.g. "Speaker 0").
    /// Used as the WHERE clause on save.
    pub original_label: String,
    /// Editable display label. Defaults to `original_label`; the user can
    /// rename to "Alice" etc. and Ctrl+S commits it.
    pub display_label: String,
    /// Number of segments this speaker holds.
    pub segment_count: i64,
    /// Total speaking time in seconds.
    pub total_secs: f64,
    /// Cached voice sample clip — `Some` when the slicer ran successfully,
    /// `None` until the cache warms.
    pub sample_path: Option<PathBuf>,
}

/// State for the Speaker Editor modal.
#[derive(Debug, Clone)]
pub enum SpeakerModalState {
    /// Modal is hidden.
    Hidden,
    /// Modal is active for a specific recording.
    Active {
        recording_id: Uuid,
        /// Internal DB primary key for the recording's video row. Used to
        /// scope SQL UPDATE / SELECT to a single transcript.
        video_id: i64,
        /// Source `.mkv` so we can re-mux subtitles on save.
        video_path: PathBuf,
        /// Loaded speaker rows in display order.
        rows: Vec<SpeakerRow>,
        /// Index of the currently-selected row.
        selected_row: usize,
        /// Whether the selected row's `display_label` is being edited.
        editing: bool,
    },
}

impl SpeakerModalState {
    pub fn is_active(&self) -> bool {
        matches!(self, SpeakerModalState::Active { .. })
    }
}

/// View modes within the Crunchr pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrunchrView {
    /// Transcript search view (default).
    Search,
    /// Processing pipeline queue.
    Queue,
    /// Recording picker for manual triggering / batch.
    RecordingPicker,
}

/// Filter for the recording picker list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordingFilter {
    All,
    ByChannel(String),
    ByPlaylist(String),
}

/// State for the recording picker view.
#[derive(Debug, Clone)]
pub struct PickerState {
    pub selected: usize,
    pub selections: HashSet<Uuid>,
    pub filter: RecordingFilter,
    /// Cached sorted list of recording IDs matching current filter.
    pub visible_ids: Vec<Uuid>,
}

impl Default for PickerState {
    fn default() -> Self {
        Self {
            selected: 0,
            selections: HashSet::new(),
            filter: RecordingFilter::All,
            visible_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineState {
    Pending,
    ExtractingAudio,
    Transcribing,
    Chunking,
    Analyzing,
    Complete,
    Failed,
}

impl std::fmt::Display for PipelineState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "Pending"),
            Self::ExtractingAudio => write!(f, "Extracting Audio"),
            Self::Transcribing => write!(f, "Transcribing"),
            Self::Chunking => write!(f, "Chunking"),
            Self::Analyzing => write!(f, "Analyzing"),
            Self::Complete => write!(f, "Complete"),
            Self::Failed => write!(f, "Failed"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProcessingJob {
    pub recording_id: Uuid,
    pub channel_name: String,
    pub title: String,
    pub video_path: PathBuf,
    pub audio_path: Option<PathBuf>,
    pub state: PipelineState,
    pub error: Option<String>,
    /// Host DAG mirror — IDs of the four stages we register against
    /// the host PipelineRegistry when the job is enqueued. None when
    /// the host registry rejected our submission (logged + warned).
    /// (C1 phase 2.)
    pub host_stages: Option<CrunchrStageIds>,
}

/// Stable IDs into the host pipeline for one Crunchr job. (C1 phase 2.)
#[derive(Debug, Clone)]
pub struct CrunchrStageIds {
    pub extract: uuid::Uuid,
    pub transcribe: uuid::Uuid,
    pub subtitle: uuid::Uuid,
    pub analyze: Option<uuid::Uuid>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SearchResult {
    pub chunk_id: i64,
    pub video_title: String,
    pub channel_name: String,
    pub snippet: String,
    pub start_sec: f64,
    pub end_sec: f64,
    pub score: f64,
    pub video_path: Option<String>,
}

/// Events returned from async pipeline tasks.
pub enum PipelineEvent {
    AudioExtracted {
        recording_id: Uuid,
        audio_path: PathBuf,
    },
    TranscriptionComplete {
        recording_id: Uuid,
        segments: Vec<Segment>,
        full_text: String,
    },
    ChunkingComplete {
        recording_id: Uuid,
        video_id: i64,
        chunks: Vec<ChunkData>,
        word_frequencies: Vec<(String, usize)>,
    },
    AnalysisComplete {
        recording_id: Uuid,
        summary: String,
        topics: String,
        sentiment: String,
        /// Token usage + estimated cost (M5.6 cost UI integration).
        /// Zero for models not in the pricing table.
        prompt_tokens: u64,
        completion_tokens: u64,
        cost_cents: u64,
    },
    StageError {
        recording_id: Uuid,
        error: String,
    },
    /// M5.1 — clip export finished. `path` is the resulting MKV file.
    /// `error` is set when ffmpeg failed; the plugin surfaces the
    /// message as last_error so the user sees what went wrong.
    ClipExportComplete {
        path: PathBuf,
        error: Option<String>,
    },
}

/// Chunk data produced by async chunking, to be written to DB.
#[derive(Debug, Clone)]
pub struct ChunkData {
    pub text: String,
    pub start_sec: f64,
    pub end_sec: f64,
    pub token_count: usize,
}

/// Analysis data for a video, fetched from video_analysis table.
#[derive(Debug, Clone, Default)]
pub struct AnalysisData {
    pub summary: String,
    pub topics: Vec<String>,
    pub sentiment: String,
}

#[derive(Debug, Clone, Default)]
pub struct Segment {
    pub index: usize,
    pub start_sec: f64,
    pub end_sec: f64,
    pub text: String,
    pub speaker: Option<String>,
    pub confidence: Option<f64>,
    /// Word-level timings if the backend produced them. Editor plugin
    /// reads this for word-accurate in/out marks; serialized to the
    /// `segments.word_timings` JSON column when present (C5).
    pub words: Option<Vec<WordTiming>>,
}

/// Summary of Crunchr processing for a single recording, used by the properties modal.
#[derive(Debug, Clone, Default)]
pub struct CrunchrRecordingInfo {
    pub status: String,
    pub segment_count: usize,
    pub word_count: usize,
    pub has_analysis: bool,
    pub summary: Option<String>,
    pub topics: Vec<String>,
    pub sentiment: Option<String>,
    /// M5.6 — token usage and estimated cost in USD cents. 0 when the
    /// recording hasn't been analyzed yet or the model is outside the
    /// pricing table.
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cost_cents: u64,
}

/// Crunchr's `[crunchr]` `config.toml` section (legacy TOML key alias:
/// `sloptube`, from before the plugin was renamed — accepted on read via
/// [`strivo_core::config::AppConfig::plugin_section`], never written back).
/// Owned here (not in `strivo-core`) because it's Creator-Edition-only
/// vocabulary — core only stores it as an untyped `AppConfig::extensions`
/// entry so an existing `config.toml`'s `[crunchr]`/`[sloptube]` section
/// round-trips unchanged.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CrunchrConfig {
    /// Whether the plugin is enabled (gates tandem auto-processing).
    #[serde(default)]
    pub enabled: bool,

    /// Whether the first-run config modal has been completed.
    #[serde(default)]
    pub configured: bool,

    #[serde(default = "default_crunchr_backend")]
    pub backend: String,

    /// Env var name for the transcription API key.
    /// Defaults: `OPENROUTER_API_KEY` for `voxtral-openrouter` (default backend),
    /// `MISTRAL_API_KEY` for `voxtral-api`.
    #[serde(default)]
    pub api_key_env: Option<String>,

    /// Base URL for self-hosted Voxtral (vLLM, RunPod, etc.).
    /// Only used when backend = "voxtral-local".
    #[serde(default)]
    pub endpoint: Option<String>,

    /// Preferred whisper model for CLI backend.
    #[serde(default)]
    pub whisper_model: Option<String>,

    /// Max seconds for whisper subprocess before timeout.
    #[serde(default = "default_whisper_timeout")]
    pub whisper_timeout_secs: u64,

    /// When true, request speaker diarization. Only honoured by backends that
    /// can produce speaker labels (`voxtral-api`, `whisperx-local`). Enables
    /// the Speaker Editor modal.
    #[serde(default)]
    pub diarize: bool,

    /// Force a fixed number of speakers in re-diarization (voice-embedding
    /// re-clustering). Set this to the known cast size for consistent content
    /// (e.g. a podcast with a fixed panel); leave unset to auto-detect.
    #[serde(default)]
    pub diarize_speakers: Option<u32>,

    /// When true, mux the generated `.vtt` subtitles back into the recording's
    /// `.mkv` via `mkvmerge` after a transcription job finishes.
    #[serde(default = "default_embed_subs")]
    pub embed_subs: bool,

    #[serde(default)]
    pub analysis: CrunchrAnalysisConfig,

    /// Tandem mode: auto-trigger on RecordingFinished for these channels.
    /// Each entry is "Platform:channel_id" (e.g., "Twitch:123456").
    #[serde(default)]
    pub tandem_channels: Vec<String>,

    /// Tandem mode: auto-trigger for recordings from these playlists.
    #[serde(default)]
    pub tandem_playlists: Vec<String>,

    /// Soft budget for paid transcription/analysis backends, in cents
    /// per month. 0 disables the warning. Crunchr surfaces a status
    /// chip when spend ≥80% and refuses pre-submission of jobs that
    /// would tip spend over budget unless --force-spend is passed.
    /// (C2.) Backwards-compatible default keeps the warning off so
    /// existing configs upgrade silently.
    #[serde(default)]
    pub budget_cents_per_month: u64,

    /// Active preset name from the user's preset library (C1). When
    /// empty, the historical `backend` field path is used.
    #[serde(default)]
    pub active_preset: Option<String>,
}

impl Default for CrunchrConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            configured: false,
            backend: default_crunchr_backend(),
            api_key_env: None,
            endpoint: None,
            whisper_model: None,
            whisper_timeout_secs: default_whisper_timeout(),
            diarize: false,
            diarize_speakers: None,
            embed_subs: default_embed_subs(),
            analysis: CrunchrAnalysisConfig::default(),
            tandem_channels: Vec::new(),
            tandem_playlists: Vec::new(),
            budget_cents_per_month: 0,
            active_preset: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CrunchrAnalysisConfig {
    #[serde(default)]
    pub enabled: bool,

    /// Env var name for OpenRouter API key (e.g., "OPENROUTER_API_KEY").
    #[serde(default)]
    pub openrouter_api_key_env: Option<String>,

    /// OpenRouter model ID for analysis (e.g., "mistralai/mistral-7b-instruct").
    #[serde(default = "default_analysis_model")]
    pub model: String,
}

impl Default for CrunchrAnalysisConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            openrouter_api_key_env: None,
            model: default_analysis_model(),
        }
    }
}

fn default_crunchr_backend() -> String {
    "voxtral-openrouter".to_string()
}

fn default_whisper_timeout() -> u64 {
    7200
}

fn default_embed_subs() -> bool {
    true
}

fn default_analysis_model() -> String {
    "mistralai/mistral-7b-instruct".to_string()
}
