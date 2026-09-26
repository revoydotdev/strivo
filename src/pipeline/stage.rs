//! Pipeline + Stage types. Pure data — no execution logic here.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub type PipelineId = Uuid;
pub type StageId = Uuid;

/// Executable destination for a stage. The graph model stays host-owned while
/// plugins own implementation details behind a stable `(plugin, verb)` pair.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageDispatch {
    pub plugin: String,
    pub verb: String,
    #[serde(default)]
    pub selection: Vec<Uuid>,
    #[serde(default)]
    pub payload: serde_json::Value,
}

impl StageDispatch {
    pub fn new(plugin: impl Into<String>, verb: impl Into<String>) -> Self {
        Self {
            plugin: plugin.into(),
            verb: verb.into(),
            selection: Vec::new(),
            payload: serde_json::Value::Null,
        }
    }

    pub fn for_recording(mut self, recording_id: Uuid) -> Self {
        self.selection = vec![recording_id];
        self
    }
}

/// What kind of work a stage performs. Identifier-only — the executor maps
/// kinds to backends via a dispatch table maintained by plugins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StageKind {
    /// Extract audio from a video file (ffmpeg).
    Extract,
    /// Run a transcription provider against an audio file.
    Transcribe { provider: String },
    /// Run a diarization provider over a transcript.
    Diarize { provider: String },
    /// Write subtitle sidecars from a transcript.
    Subtitle,
    /// Run an analysis backend (LLM summary, topic extract, sentiment).
    Analyze { provider: String },
    /// ffmpeg clip export with in/out timestamps.
    ExportClip,
    /// Lossless concat of N clips into one file.
    Concat,
    /// Pull a single VOD via yt-dlp.
    Archive,
    /// Plugin-provided stage. Carries an opaque identifier the plugin
    /// registers a dispatcher for.
    Custom(String),
}

impl StageKind {
    pub fn label(&self) -> String {
        match self {
            Self::Extract => "extract".into(),
            Self::Transcribe { provider } => format!("transcribe({provider})"),
            Self::Diarize { provider } => format!("diarize({provider})"),
            Self::Subtitle => "subtitle".into(),
            Self::Analyze { provider } => format!("analyze({provider})"),
            Self::ExportClip => "clip".into(),
            Self::Concat => "concat".into(),
            Self::Archive => "archive".into(),
            Self::Custom(s) => format!("custom({s})"),
        }
    }
}

/// Resource lock a stage needs. Held while the stage runs, released when
/// it terminates (success, failure, or cancellation).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResourceLock {
    /// Mutually exclusive across all stages requesting it. For GPU-bound
    /// providers like whisperx-local, voxtral-local.
    Gpu,
    /// CPU-heavy local transforms. Capacity is declared by the template so
    /// several FFmpeg/analysis stages cannot saturate every core at once.
    Cpu { cap: usize },
    /// High-throughput media reads/writes. Separate from file exclusivity:
    /// different recordings may still contend for one physical disk.
    Disk { cap: usize },
    /// Bounded counting semaphore for an external API. The `cap` is the
    /// max concurrent requests; multiple stages can share one provider.
    Api { name: String, cap: usize },
    /// File-level mutex keyed by a path. Two stages writing the same
    /// recording's sidecars block on this.
    File { path: String },
}

impl ResourceLock {
    /// A lock as one line: the kind and the capacity that bounds it. `File`
    /// carries a path, so this is text a caller must treat as untrusted.
    pub fn label(&self) -> String {
        match self {
            Self::Gpu => "gpu".to_string(),
            Self::Cpu { cap } => format!("cpu:{cap}"),
            Self::Disk { cap } => format!("disk:{cap}"),
            Self::Api { name, cap } => format!("api:{name}:{cap}"),
            Self::File { path } => format!("file:{path}"),
        }
    }
}

/// One violated rule: its stable id, the executor's own message, and which
/// stage/edge/lock broke it where the rule has one.
///
/// `reason` is `&'static str` because it is the exact string the executor has
/// always returned for that rule and this struct must not be able to change
/// it. `detail` is the new part — an editor can only show a useful error if
/// the refusal points at something the user can delete or edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleViolation {
    pub rule: &'static str,
    pub reason: &'static str,
    pub detail: Option<String>,
}

/// Where a stage is in its lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StageState {
    Pending,
    Running {
        started_at_ms: u128,
    },
    Done,
    Failed {
        error: String,
        attempt: u8,
    },
    /// Permanent failure — out of retries. Pipeline marks itself Failed.
    Exhausted {
        error: String,
    },
    /// User-requested cancellation. No further attempts.
    Cancelled,
    /// Earlier stage's failure means this one will never run.
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stage {
    pub id: StageId,
    pub name: String,
    pub kind: StageKind,
    /// IDs of stages that must reach [`StageState::Done`] before this one
    /// can dispatch. Empty = ready immediately.
    pub inputs: Vec<StageId>,
    pub state: StageState,
    pub attempts: u8,
    pub max_attempts: u8,
    /// Optional fallback provider for the next attempt. The executor
    /// rewrites the kind's provider field on retry when set.
    pub fallback_provider: Option<String>,
    /// Resource locks this stage needs to hold while running.
    pub requires: Vec<ResourceLock>,
    /// Last estimated cost in cents. Plugins fill this; the executor
    /// aggregates per-pipeline for the budget warning.
    pub cost_cents_estimate: u32,
    /// Daemon dispatch target. `None` is valid for a source/artifact node, but
    /// an executable pending stage must carry a target.
    #[serde(default)]
    pub dispatch: Option<StageDispatch>,
    /// Structured outputs emitted by the executor. Downstream plugins receive
    /// these through the pipeline snapshot rather than private database reach-ins.
    #[serde(default)]
    pub artifacts: Vec<serde_json::Value>,
    /// Cancellation handle. Skipped during serialization — recreated when
    /// the pipeline reloads (M5 persistence).
    #[serde(skip, default = "CancellationToken::new")]
    pub cancel: CancellationToken,
}

impl Stage {
    pub fn new(name: impl Into<String>, kind: StageKind) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            kind,
            inputs: Vec::new(),
            state: StageState::Pending,
            attempts: 0,
            max_attempts: 3,
            fallback_provider: None,
            requires: Vec::new(),
            cost_cents_estimate: 0,
            dispatch: None,
            artifacts: Vec::new(),
            cancel: CancellationToken::new(),
        }
    }

    pub fn with_inputs(mut self, inputs: Vec<StageId>) -> Self {
        self.inputs = inputs;
        self
    }

    pub fn with_requires(mut self, requires: Vec<ResourceLock>) -> Self {
        self.requires = requires;
        self
    }

    pub fn with_max_attempts(mut self, n: u8) -> Self {
        self.max_attempts = n;
        self
    }

    pub fn with_fallback(mut self, provider: impl Into<String>) -> Self {
        self.fallback_provider = Some(provider.into());
        self
    }

    pub fn with_cost(mut self, cents: u32) -> Self {
        self.cost_cents_estimate = cents;
        self
    }

    pub fn with_dispatch(mut self, dispatch: StageDispatch) -> Self {
        self.dispatch = Some(dispatch);
        self
    }

    /// Backoff between attempts: 5s, 10s, 30s. Same shape a plugin's own
    /// retry logic uses today for its long-running stages.
    pub fn backoff_after(attempt: u8) -> Duration {
        match attempt {
            0 => Duration::from_secs(0),
            1 => Duration::from_secs(5),
            2 => Duration::from_secs(10),
            _ => Duration::from_secs(30),
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.state,
            StageState::Done
                | StageState::Exhausted { .. }
                | StageState::Cancelled
                | StageState::Skipped
        )
    }
}

/// Top-level lifecycle of a Pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PipelineState {
    Pending,
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pipeline {
    pub id: PipelineId,
    pub name: String,
    pub stages: Vec<Stage>,
    pub state: PipelineState,
    pub started_at_secs: Option<u64>,
    pub completed_at_secs: Option<u64>,
    /// Recording or other domain object this run is rooted in.
    #[serde(default)]
    pub subject_id: Option<Uuid>,
    /// `manual`, `recording_finished`, `daily`, or a future trigger name.
    #[serde(default = "default_trigger")]
    pub trigger: String,
    /// Last terminal error, suitable for UI display and audit logs.
    #[serde(default)]
    pub error: Option<String>,
    /// Sub-millisecond timestamp for stable ordering in the registry. Not
    /// persisted — `started_at_secs` is enough for the UI.
    #[serde(skip, default = "Instant::now")]
    pub created_at: Instant,
}

impl Pipeline {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            stages: Vec::new(),
            state: PipelineState::Pending,
            started_at_secs: None,
            completed_at_secs: None,
            subject_id: None,
            trigger: default_trigger(),
            error: None,
            created_at: Instant::now(),
        }
    }

    /// Append a stage and return its id so subsequent stages can reference
    /// it as an input.
    pub fn add_stage(&mut self, stage: Stage) -> StageId {
        let id = stage.id;
        self.stages.push(stage);
        id
    }

    pub fn for_recording(mut self, recording_id: Uuid) -> Self {
        self.subject_id = Some(recording_id);
        self
    }

    pub fn with_trigger(mut self, trigger: impl Into<String>) -> Self {
        self.trigger = trigger.into();
        self
    }

    /// Reject cyclic dependencies. Called once at submission; the executor
    /// trusts the graph thereafter.
    pub fn assert_acyclic(&self) -> Result<(), &'static str> {
        self.cycle()
            .map(|_| "pipeline DAG contains a cycle")
            .map_or(Ok(()), Err)
    }

    /// The cycle, if there is one, as the ordered stage ids that close it —
    /// so the offending edge is the last hop. `assert_acyclic` reports only
    /// that a cycle exists, which over ten stages names nothing a user can
    /// delete.
    pub fn cycle(&self) -> Option<Vec<StageId>> {
        let mut visited: HashSet<StageId> = HashSet::new();
        let mut stack: Vec<StageId> = Vec::new();
        for s in &self.stages {
            if let Some(loop_) = Self::dfs(self, s.id, &mut visited, &mut stack) {
                return Some(loop_);
            }
        }
        None
    }

    /// `None` when `node` closes no cycle; otherwise the cycle in walk order,
    /// starting and ending at the same stage, so the offending edge is the
    /// last hop. `stack` doubles as the current path, which is what lets the
    /// cycle be reconstructed rather than merely detected.
    fn dfs(
        pipe: &Pipeline,
        node: StageId,
        visited: &mut HashSet<StageId>,
        stack: &mut Vec<StageId>,
    ) -> Option<Vec<StageId>> {
        if let Some(at) = stack.iter().position(|id| *id == node) {
            let mut loop_ = stack[at..].to_vec();
            loop_.push(node);
            return Some(loop_);
        }
        if visited.contains(&node) {
            return None;
        }
        stack.push(node);
        if let Some(s) = pipe.stages.iter().find(|s| s.id == node) {
            for &input in &s.inputs {
                if let Some(loop_) = Self::dfs(pipe, input, visited, stack) {
                    return Some(loop_);
                }
            }
        }
        stack.pop();
        visited.insert(node);
        None
    }

    pub fn cancel_all(&self) {
        for s in &self.stages {
            s.cancel.cancel();
        }
    }

    /// Sum of stage cost estimates. Used by a plugin's cost dashboard +
    /// pre-submit budget warning.
    pub fn total_cost_cents(&self) -> u32 {
        self.stages.iter().map(|s| s.cost_cents_estimate).sum()
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.state,
            PipelineState::Done | PipelineState::Failed | PipelineState::Cancelled
        )
    }
}

fn default_trigger() -> String {
    "manual".to_string()
}
