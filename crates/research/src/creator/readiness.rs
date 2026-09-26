//! Ingest readiness: what a source still needs before it can be edited, one
//! step at a time.
//!
//! Report item C08 (`docs/CREATOR-EDITION-REPORT-2026-09-07.md:287`): probe
//! imported media, identify its tracks, build proxies, waveform pyramids,
//! filmstrips and thumbnails, and flag missing external tools.
//!
//! ## The deliverable is the per-step state, not a verdict
//!
//! **A checklist that answers "is this source ready?" with one boolean has
//! built nothing.** The questions a caller actually has are per step — is the
//! picture done, is there a waveform, is a *named* tool missing — and one
//! boolean cannot answer any of them separately. So [`StepState`] is a
//! five-value vocabulary attached to each of the five [`IngestStep`]s, and
//! [`Readiness::state`] reads one of them. The overall answer
//! ([`Readiness::editable`]) is *derived from exactly one step*, the probe, and
//! says so: a timeline is addressed in the source's own tracks and duration, so
//! it needs the probe and nothing else. That is the plan's "a user must be
//! able to start editing ready media while optional analysis continues", and it
//! is why a missing ffmpeg does not stop editing.
//!
//! The vocabulary, and what a consumer does with each value:
//!
//! | state | what it means | what the consumer does |
//! | --- | --- | --- |
//! | [`Ready`](StepState::Ready) | the facts this step needs are in hand | use it |
//! | [`Pending`](StepState::Pending) | applicable, not produced yet | nothing — not a problem to chase |
//! | [`NotApplicable`](StepState::NotApplicable) | the source has no track this step needs | nothing; **not a failure** |
//! | [`Blocked`](StepState::Blocked) | a named precondition is unmet | do exactly [`StepState::next_step`] |
//! | [`Failed`](StepState::Failed) | the work ran and produced nothing | read the attached reason |
//!
//! `NotApplicable` is the state this module exists to get right. A source with
//! no audio has no waveform, and reporting that as a failure sends a user after
//! a non-problem; it is a **fact about the source**, decided per step from the
//! tracks the probe found.
//!
//! There is deliberately **no `InProgress` state**. A step is either produced or
//! not, so a cancelled probe cannot strand one permanently mid-flight: the step
//! reads `Pending` and nothing is latched. That is the whole cancel-resume
//! argument, and it holds because the vocabulary has no state to be stranded in.
//!
//! ## Nothing here probes, transcodes or measures
//!
//! This module is a **report**, and it is handed its measurements. The probe is
//! the web crate's (todo 13's `ffprobe` path), and the proxies, waveforms,
//! filmstrips and thumbnails come from the same paths; this module decides what
//! their absence means. It therefore re-implements none of them, spawns no
//! child process, and owns no deadline.
//!
//! **The bounds are not restated here, and that is deliberate.** The probe pool
//! (two slots), the thumbnail limit (one) and the 30 s probe/thumbnail deadline
//! are enforced at `crates/strivo-web/src/server.rs:176-177` and
//! `crates/strivo-web/src/routes/settings.rs:325,355,1209,1308`, where todo 13
//! measured them. Copying those numbers into a second module would create a
//! second source of truth that nothing keeps in step, so this module names the
//! site in prose and reports no number. A checklist asserting a third bound
//! would be worse than one that asserts none.
//!
//! Two properties of those bounds belong to that lane and are **reported, not
//! fixed** here: the semaphores are unbounded at the acquisition site and are
//! safe only because every holder has a hard deadline plus `kill_on_drop`, and
//! nothing in the type system couples those two. Changing them means editing
//! todo 13's files.
//!
//! ## Readiness is derived, never stored
//!
//! There is no `creator_readiness` table, for the same reason
//! [`SourceAsset::availability`](super::SourceAsset::availability) is not a
//! column: a stored readiness is a second claim about the same bytes, and it
//! goes stale the moment one of them changes. What makes a held checklist
//! detectable as stale is the **content identity the probe ran against** —
//! [`ReadinessError::ContentChanged`] refuses to build one from a report of
//! different content, and [`Readiness::stale_for`] says so about a checklist
//! built before a swap. Identity, not a timestamp.
//!
//! ## No route, no persisted form
//!
//! The first lane that mounts this over HTTP owns the route and the `serde`
//! derives; nothing here is served yet, and no shape is committed to on its
//! behalf.

use strivo_editor::time::MediaTime;
use strivo_multitrack::AudioTrack;

use super::entities::SourceAsset;

/// An external binary a step cannot run without. The identity, not its state:
/// whether it is present is [`Toolchain`]'s business.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    Ffmpeg,
    Ffprobe,
}

impl Tool {
    /// The binary's name, which is also how a user identifies it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ffmpeg => "ffmpeg",
            Self::Ffprobe => "ffprobe",
        }
    }

    /// What to do about this tool being absent.
    ///
    /// It names the tool and then defers the *install command* to
    /// `strivo doctor`, which already prints the right one per platform
    /// (`crates/strivo-bin/src/main.rs:920-932`). Restating those three
    /// platform variants here would be a fourth copy that drifts.
    #[must_use]
    pub const fn next_step(self) -> &'static str {
        match self {
            Self::Ffmpeg => "ffmpeg is not on PATH: install it, then re-run readiness",
            Self::Ffprobe => "ffprobe is not on PATH: install it, then re-run readiness",
        }
    }
}

/// Which of the two tools this host can actually run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Toolchain {
    /// ffmpeg: proxies, waveforms, filmstrips and thumbnails.
    pub ffmpeg: bool,
    /// ffprobe: the probe step, and therefore everything downstream of it.
    pub ffprobe: bool,
}

impl Toolchain {
    /// Whether `tool` can be run here.
    #[must_use]
    pub const fn present(&self, tool: Tool) -> bool {
        match tool {
            Tool::Ffmpeg => self.ffmpeg,
            Tool::Ffprobe => self.ffprobe,
        }
    }
}

/// One step of ingest: the five the report names — probe, proxies, waveforms,
/// filmstrips, thumbnails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IngestStep {
    Probe,
    Proxies,
    Waveform,
    Filmstrip,
    Thumbnail,
}

impl IngestStep {
    /// Every step, in pipeline order. A fixed list rather than a growable one: a
    /// checklist whose step set can change under a consumer is a checklist no
    /// consumer can match exhaustively.
    pub const ALL: [Self; 5] = [
        Self::Probe,
        Self::Proxies,
        Self::Waveform,
        Self::Filmstrip,
        Self::Thumbnail,
    ];

    /// The step's own name, for a wire form or a log line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Probe => "probe",
            Self::Proxies => "proxies",
            Self::Waveform => "waveform",
            Self::Filmstrip => "filmstrip",
            Self::Thumbnail => "thumbnail",
        }
    }

    /// The external binary this step runs. Every step but the probe needs
    /// ffmpeg; the probe needs ffprobe and nothing else does.
    #[must_use]
    pub const fn tool(self) -> Tool {
        match self {
            Self::Probe => Tool::Ffprobe,
            Self::Proxies | Self::Waveform | Self::Filmstrip | Self::Thumbnail => Tool::Ffmpeg,
        }
    }

    /// Position in [`Self::ALL`], which is how a step's state is stored.
    const fn index(self) -> usize {
        match self {
            Self::Probe => 0,
            Self::Proxies => 1,
            Self::Waveform => 2,
            Self::Filmstrip => 3,
            Self::Thumbnail => 4,
        }
    }
}

/// The video stream's frame cadence, as the probe measured it.
///
/// Three values, not a rate. `Unknown` is the load-bearing one: ffprobe prints
/// `avg_frame_rate` of `0/0` for a stream it did not measure, and a source whose
/// cadence was never established must not be reported on a constant-frame
/// assumption. There is no `f64` frame rate in this module for the same reason
/// there is no millisecond duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameTiming {
    Constant,
    Variable,
    Unknown,
}

/// The video stream's facts — only what readiness needs. A source with no video
/// is a different shape of source, and everything else about the picture belongs
/// to the surfaces that render it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoStream {
    /// What the probe measured about the frame cadence.
    pub timing: FrameTiming,
    /// The measured distance between keyframes, when the probe read placement.
    /// Carried rather than judged: whether an interval is dense enough for
    /// time-addressed frame extraction is a tolerance the consumer states, not a
    /// number this module may invent.
    pub keyframe_interval: Option<MediaTime>,
}

/// What a probe measured about a source.
///
/// Handed in, never obtained here. A probe that ran and read nothing is
/// representable — no video, no audio, no duration — which is how a failed probe
/// is told apart from one still in flight.
#[derive(Debug, Clone)]
pub struct ProbeReport {
    /// The content identity the probe ran against, checked against the source's
    /// own before a checklist is built.
    pub fingerprint: String,
    /// The source's own length, when the container stated one. `None` on a
    /// container that did not, which is not the same as a zero length.
    pub duration: Option<MediaTime>,
    /// The video stream, or `None` for a container with no picture.
    pub video: Option<VideoStream>,
    /// The audio streams, identified by the one place that answers "which
    /// mic/game/voice-chat track is this" (`strivo_multitrack`). Reused rather
    /// than restated, because a second track shape is a second answer.
    pub audio: Vec<AudioTrack>,
}

impl ProbeReport {
    /// Whether the probe read anything decodable at all.
    fn found_a_stream(&self) -> bool {
        self.duration.is_some() || self.video.is_some() || !self.audio.is_empty()
    }
}

/// A named reason a step cannot proceed. Named rather than a bare flag because
/// each one is a different action for whoever reads the checklist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocker {
    /// The step's external binary is not installed.
    Tool(Tool),
    /// The frame cadence is not the constant cadence a time-addressed filmstrip
    /// assumes. Carries the reading, because "establish it" and "it is variable,
    /// so seek per measured timestamp" are different instructions.
    FrameTiming(FrameTiming),
}

impl Blocker {
    /// The reason's stable lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tool(_) => "tool",
            Self::FrameTiming(_) => "frame-timing",
        }
    }

    /// Exactly what to do about it.
    #[must_use]
    pub const fn next_step(self) -> &'static str {
        match self {
            Self::Tool(tool) => tool.next_step(),
            Self::FrameTiming(FrameTiming::Variable) => {
                "the frame cadence is variable: build the filmstrip from measured \
                 presentation timestamps, not from evenly-spaced seeks"
            }
            Self::FrameTiming(_) => {
                "the frame cadence was never established: probe presentation \
                 timestamps before building a time-addressed filmstrip"
            }
        }
    }
}

/// One step's own state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    /// The facts this step needs are in hand and its output can be produced. Not
    /// a claim that the output *exists* — this reports readiness, and the
    /// surfaces that produce the bytes report their own completion.
    Ready,
    /// Applicable, not produced yet. The ordinary state for a source that has
    /// just been imported.
    Pending,
    /// The source has no track this step needs. A state, not a failure: there is
    /// nothing to wait for and nothing to fix.
    NotApplicable,
    /// A named precondition is unmet; [`Self::next_step`] says which.
    Blocked(Blocker),
    /// The work ran and produced nothing, with the reason attached.
    Failed(&'static str),
}

impl StepState {
    /// The state's stable lowercase name.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Pending => "pending",
            Self::NotApplicable => "not-applicable",
            Self::Blocked(_) => "blocked",
            Self::Failed(_) => "failed",
        }
    }

    /// Whether this step has what it needs.
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }

    /// What a consumer should do, or `None` when there is nothing to do — which
    /// includes every `Pending` and `NotApplicable` step, so "waiting" and "not
    /// applicable" are never reported as work.
    #[must_use]
    pub const fn next_step(&self) -> Option<&'static str> {
        match self {
            Self::Blocked(blocker) => Some(blocker.next_step()),
            Self::Failed(detail) => Some(detail),
            Self::Ready | StepState::Pending | StepState::NotApplicable => None,
        }
    }
}

/// Why no checklist could be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadinessError {
    /// The probe report describes different content than the source. A readiness
    /// over bytes that no longer exist is the stale-state failure, and the
    /// content identity is the only thing that catches it — a timestamp and a
    /// cache flag both survive a rewrite.
    ContentChanged {
        /// The identity the source has now.
        source: String,
        /// The identity the probe ran against.
        probed: String,
    },
}

impl std::fmt::Display for ReadinessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ContentChanged { source, probed } => write!(
                f,
                "this source is {source} and the probe read {probed}: the bytes changed \
                 under the measurement, so nothing measured still describes them"
            ),
        }
    }
}

impl std::error::Error for ReadinessError {}

/// One source's ingest readiness: five states, and the measurements they were
/// derived from.
///
/// Derived and never persisted — see the module doc. Holds the [`SourceAsset`]
/// it describes rather than a copy of its fields, so a caller reading
/// [`Self::source`] reads the row's own value.
#[derive(Debug, Clone, PartialEq)]
pub struct Readiness {
    source: SourceAsset,
    probed: String,
    steps: [StepState; 5],
    duration: Option<MediaTime>,
    audio_streams: usize,
    video: Option<VideoStream>,
}

impl Readiness {
    /// Derive one source's readiness.
    ///
    /// `report` is `None` while the probe has not completed, which is a
    /// different fact from a probe that completed and found nothing: the first
    /// leaves every step `Pending`, the second fails the probe and marks the
    /// track-dependent steps `NotApplicable`. A report whose fingerprint is not
    /// the source's own is refused, because a measurement of other bytes cannot
    /// say anything about these.
    pub fn evaluate(
        source: &SourceAsset,
        report: Option<&ProbeReport>,
        tools: &Toolchain,
    ) -> Result<Self, ReadinessError> {
        if let Some(report) = report {
            if report.fingerprint != source.media_fingerprint {
                return Err(ReadinessError::ContentChanged {
                    source: source.media_fingerprint.clone(),
                    probed: report.fingerprint.clone(),
                });
            }
        }
        let mut steps = [StepState::Pending; 5];
        for step in IngestStep::ALL {
            steps[step.index()] = match report {
                // Applicability is decided first, and it outranks a missing tool:
                // a source with no audio needs no waveform whether or not ffmpeg
                // is installed, and reporting that as blocked would send a user
                // to install a tool for a file which will never have one. With no
                // report there is nothing to be applicable *to*, so a missing
                // tool is the only thing that is known yet.
                Some(report) if applicability(step, report).is_none() => StepState::NotApplicable,
                _ if !tools.present(step.tool()) => StepState::Blocked(Blocker::Tool(step.tool())),
                None => StepState::Pending,
                Some(report) => match step {
                    // The probe is the step that establishes what the others apply
                    // to, so it has no applicability test of its own.
                    IngestStep::Probe => {
                        if report.found_a_stream() {
                            StepState::Ready
                        } else {
                            StepState::Failed("the probe read no decodable stream")
                        }
                    }
                    IngestStep::Filmstrip => match report.video {
                        Some(video) if video.timing == FrameTiming::Constant => StepState::Pending,
                        Some(video) => StepState::Blocked(Blocker::FrameTiming(video.timing)),
                        None => StepState::NotApplicable,
                    },
                    _ => StepState::Pending,
                },
            };
        }
        Ok(Self {
            source: source.clone(),
            probed: report.map_or_else(String::new, |r| r.fingerprint.clone()),
            steps,
            duration: report.and_then(|r| r.duration),
            audio_streams: report.map_or(0, |r| r.audio.len()),
            video: report.and_then(|r| r.video),
        })
    }

    /// One step's own state.
    #[must_use]
    pub fn state(&self, step: IngestStep) -> StepState {
        self.steps[step.index()]
    }

    /// Whether a timeline can be built on this source now.
    ///
    /// Derived from the **probe step alone**, and that is the whole argument: a
    /// timeline addresses the source's own tracks and duration, so a source
    /// nobody has probed cannot be cut, while a source with no proxy, waveform,
    /// filmstrip or thumbnail can be. A missing optional analysis tool therefore
    /// never blocks editing, which is the plan's "must not block the timeline on
    /// a missing optional provider".
    #[must_use]
    pub fn editable(&self) -> bool {
        self.state(IngestStep::Probe).is_ready()
    }

    /// Whether these states were derived from content this source no longer has
    /// — the check a caller makes before trusting a checklist it has held.
    #[must_use]
    pub fn stale_for(&self, source: &SourceAsset) -> bool {
        self.probed != source.media_fingerprint
    }

    /// The source these states describe.
    #[must_use]
    pub const fn source(&self) -> &SourceAsset {
        &self.source
    }

    /// The video stream's measured cadence, or `Unknown` for a source with no
    /// picture and for one whose cadence was never probed.
    #[must_use]
    pub fn frame_timing(&self) -> FrameTiming {
        self.video.map_or(FrameTiming::Unknown, |v| v.timing)
    }

    /// The measured keyframe interval, when a probe read one. `None` means the
    /// probe did not, which is not the same as a dense one.
    #[must_use]
    pub fn keyframe_interval(&self) -> Option<MediaTime> {
        self.video.and_then(|v| v.keyframe_interval)
    }

    /// The source's length as the probe measured it, in the one time model.
    #[must_use]
    pub const fn duration(&self) -> Option<MediaTime> {
        self.duration
    }

    /// How many audio streams the probe identified.
    #[must_use]
    pub const fn audio_streams(&self) -> usize {
        self.audio_streams
    }

    /// What this source *is*, in the vocabulary the source row's single
    /// `track_type` column can hold.
    ///
    /// One value, never a per-track one: a recording has one track type, and
    /// writing four of them into that one column kept only the last (todo 33's
    /// finding). The individual roles are [`Self::audio_streams`]' business and
    /// live on the multitrack session, not here.
    #[must_use]
    pub const fn track_type(&self) -> &'static str {
        if self.video.is_some() {
            "video"
        } else if self.audio_streams > 0 {
            "audio"
        } else {
            "unknown"
        }
    }
}

/// Whether `step` has a track to work on at all. `None` is the
/// `NotApplicable` answer, and it is decided here so the decision lives in one
/// place: a waveform needs audio, a filmstrip or a thumbnail needs video, a
/// proxy needs either, and the probe needs nothing because it is what
/// establishes what the rest have.
fn applicability(step: IngestStep, report: &ProbeReport) -> Option<()> {
    match step {
        IngestStep::Probe => Some(()),
        // A proxy needs something to proxy: an audio-only source still has one,
        // and a container with no decodable track has none.
        IngestStep::Proxies => {
            if report.video.is_some() || !report.audio.is_empty() {
                Some(())
            } else {
                None
            }
        }
        IngestStep::Waveform => {
            if report.audio.is_empty() {
                None
            } else {
                Some(())
            }
        }
        IngestStep::Filmstrip | IngestStep::Thumbnail => {
            if report.video.is_some() {
                Some(())
            } else {
                None
            }
        }
    }
}
