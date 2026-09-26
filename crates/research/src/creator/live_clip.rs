//! Live clipping during capture, on the Creator side (CE-P8).
//!
//! `strivo_core::recording::live_clip` decides **which bytes of a running
//! capture are safe to read**; this module is what a clip is made of them
//! once they are. The split is the dependency graph, not a preference:
//! `strivo-core` sits below `strivo-research`, so the PVR half cannot name
//! `strivo_editor::render::CompiledPlan` and this half cannot see
//! `ActiveRecording`. The tail therefore crosses the seam as
//! [`LiveCaptureTail`] — the same values, carried as data, with no second
//! source of truth about what is finalized.
//!
//! ## Nothing here is a second render path, plan, or lifecycle
//!
//! A live clip is compiled by [`strivo_editor::render::compile`] and stored
//! by [`super::render_plan::compile_render_plan`], so it is todo 23's plan
//! and todo 23's row. It runs as a [`super::jobs::Job`] through
//! [`Store::submit_job`](super::store_jobs::Store::submit_job) and finishes
//! through [`Store::complete`](super::store_jobs::Store::complete) with
//! [`Store::validate_artifact`](super::store_jobs::Store::validate_artifact)
//! as the only way to reach `done`, so it is todo 24's lifecycle. There is
//! no `LiveClipState` enum and no `live_clip_jobs` table, because a second
//! vocabulary for the same machine is how a reader ends up with two answers
//! to "is it finished".
//!
//! ## A signal is a stream event, not a new table
//!
//! An extractor tailing the live segment produces a *point* on the capture
//! clock with a label and a category. That is precisely
//! [`super::stream_event::StreamEvent`] with
//! [`EventKind::Category`](super::stream_event::EventKind::Category), already
//! placed through the same [`ClockMap`](strivo_editor::time::ClockMap) and
//! already carrying the per-sample uncertainty. So the signals of a capture
//! in progress live in `creator_stream_events` from the first event onward,
//! and a new table for them would be a second place holding the same fact.
//! What makes them *live* is not the table: it is that
//! `creator_capture_sessions.finalized` stays `0` while they are written, and
//! `append_stream_events` never checks it. So visibility during capture is a
//! property of the schema, asserted in `tests/live_clip.rs`.

use std::fmt;
use std::path::{Path, PathBuf};

use strivo_editor::render::compile;
use strivo_editor::render::{
    CaptionMode, CaptionSpec, ClipRequest, Colour, Container, LayerKind, LayerSpec, OutputProfile,
    RenderRequest, TrackSpec, Transform, VideoIntent,
};
use strivo_editor::time::{Coordinate, Interval, MediaTime, Rate, SourceRef, SourceSpan, TimeBase};

use super::entities::{Artifact, RenderPlan, Variant};
use super::ids::{
    ArtifactId, CaptureSessionId, JobId, ProjectId, RenderPlanId, SourceAssetId, VariantId,
};
use super::jobs::{immutable_output_path, CacheKey, Job, JobInputs, MAX_ATTEMPTS};
use super::render_plan::compile_render_plan;
use super::store::Store;
use crate::{ResearchError, Result};

/// Shortest clip a submission will attempt. The same ceiling
/// `strivo_core::recording::live_clip::MIN_CLIP_MS` states, restated rather
/// than imported because the PVR crate is not visible here — the two are
/// pinned to the same value by a test in `tests/live_clip.rs` that reads the
/// PVR source.
///
/// Whole microseconds, because `crates/research/build.rs` refuses a
/// millisecond duration anywhere under `src/creator` and this is a duration.
pub const MIN_CLIP_US: i64 = 1_000_000;

/// A duration in whole milliseconds, for a message. The only place a
/// millisecond appears in this module, and it is a rendering of a
/// [`MediaTime`] rather than a stored value.
fn ms(t: MediaTime) -> i64 {
    t.micros() / 1_000
}

/// `a + b`, saturating at the last representable position rather than
/// wrapping into a plausible-looking earlier time.
fn add(a: MediaTime, b: MediaTime) -> MediaTime {
    a.micros()
        .checked_add(b.micros())
        .and_then(|v| MediaTime::from_micros(v).ok())
        .unwrap_or(MediaTime::ZERO)
}

/// One part of a capture, as the tail reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailPart {
    /// 1-based, matching `segments::segment_path(base, n)` on the PVR side.
    pub index: u32,
    /// A **location**, not an identity. The job's identity is the source
    /// asset id, which is why [`LiveClipRequest`] carries one.
    pub path: PathBuf,
    /// Media time, not wall clock: a reconnect leaves a hole in the wall
    /// clock and not in the media, and a clip is a cut out of the media.
    pub start: MediaTime,
    pub duration: MediaTime,
    /// `false` while a recorder is still writing this file. A live MKV has
    /// no duration and no complete index, so a part is not readable until
    /// its writer exits; this flag is that fact, carried.
    pub finalized: bool,
}

/// The capture tail, as the render side receives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveCaptureTail {
    pub running: bool,
    pub parts: Vec<TailPart>,
}

impl LiveCaptureTail {
    /// Media the capture has finished, on the capture's own clock.
    #[must_use]
    pub fn finalized_end(&self) -> MediaTime {
        self.parts
            .iter()
            .filter(|p| p.finalized)
            .map(|p| add(p.start, p.duration))
            .max()
            .unwrap_or(MediaTime::ZERO)
    }

    fn intersects(part: &TailPart, start: MediaTime, end: MediaTime) -> bool {
        part.start < end && start < add(part.start, part.duration)
    }
}

/// Why a live clip submission was refused.
///
/// The variants mirror `strivo_core::recording::live_clip::LiveClipError`
/// one for one, and the mirrored names are deliberate: a caller that has
/// read one refusal has read the other, and a test that pins the mapping
/// catches a divergence rather than leaving two vocabularies to translate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveClipRejection {
    CaptureStopped,
    InvertedRange {
        start: MediaTime,
        end: MediaTime,
    },
    ClipTooShort {
        span: MediaTime,
        minimum: MediaTime,
    },
    NothingFinalized {
        requested_start: MediaTime,
        finalized_end: MediaTime,
    },
    PartStillOpen {
        index: u32,
        path: PathBuf,
        finalized_end: MediaTime,
    },
}

impl fmt::Display for LiveClipRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CaptureStopped => {
                f.write_str("the capture has stopped, so this is not a live clip")
            }
            Self::InvertedRange { start, end } => write!(
                f,
                "clip range is empty or reversed: {} ms to {} ms",
                ms(*start),
                ms(*end)
            ),
            Self::ClipTooShort { span, minimum } => write!(
                f,
                "clip spans {} ms, below the {} ms minimum",
                ms(*span),
                ms(*minimum)
            ),
            Self::NothingFinalized {
                requested_start,
                finalized_end,
            } => write!(
                f,
                "no finalized capture media covers {} ms yet; capture has finalized {} ms",
                ms(*requested_start),
                ms(*finalized_end)
            ),
            Self::PartStillOpen {
                index,
                path,
                finalized_end,
            } => write!(
                f,
                "capture part {index} ({}) is still being written, so it is not a legal clip \
                 input; capture has finalized {} ms",
                path.display(),
                ms(*finalized_end)
            ),
        }
    }
}

/// A submission. Everything the render needs, and nothing it may invent.
#[derive(Debug, Clone)]
pub struct LiveClipRequest {
    pub project: ProjectId,
    pub session: CaptureSessionId,
    /// The recording's own identity. A path is a location; this is what the
    /// job's cache key is derived from, and what a moved file must not
    /// change.
    pub source_asset: SourceAssetId,
    /// The destination-shaped variant the clip is compiled for. Required,
    /// not optional: `creator_render_plans.variant_id` is a plain foreign
    /// key, and a live clip rendered as authored is a variant with empty
    /// `format_overrides_json` rather than a sentinel.
    pub variant: Variant,
    /// The tail at the moment of the request, open part included.
    pub tail: LiveCaptureTail,
    pub start: MediaTime,
    pub end: MediaTime,
    /// Canonicalised render settings, already in the form the cache key
    /// hashes. A caller that has not canonicalised them has not got a
    /// cache key.
    pub settings: String,
    /// `(name, version)` for every tool the clip's render invokes.
    pub tool_versions: Vec<(String, String)>,
    pub scratch_root: PathBuf,
    pub now: String,
}

/// The finalized parts a request may read, and nothing else.
pub fn validate_live_clip(
    tail: &LiveCaptureTail,
    start: MediaTime,
    end: MediaTime,
) -> std::result::Result<Vec<TailPart>, LiveClipRejection> {
    if !tail.running {
        return Err(LiveClipRejection::CaptureStopped);
    }
    if end <= start {
        return Err(LiveClipRejection::InvertedRange { start, end });
    }
    if end.micros() - start.micros() < MIN_CLIP_US {
        return Err(LiveClipRejection::ClipTooShort {
            span: MediaTime::from_micros(end.micros() - start.micros()).expect("checked above"),
            minimum: MediaTime::from_micros(MIN_CLIP_US).expect("a positive duration"),
        });
    }
    let finalized_end = tail.finalized_end();
    if start >= finalized_end {
        return Err(LiveClipRejection::NothingFinalized {
            requested_start: start,
            finalized_end,
        });
    }
    // Checked against the **requested** end, not the finalized one: clipping
    // first and checking second would answer a request for 150 s with a
    // silent 10 s clip, which is the same observable outcome as rendering a
    // file that is still being written.
    for part in &tail.parts {
        if LiveCaptureTail::intersects(part, start, end) && !part.finalized {
            return Err(LiveClipRejection::PartStillOpen {
                index: part.index,
                path: part.path.clone(),
                finalized_end,
            });
        }
    }
    let parts: Vec<TailPart> = tail
        .parts
        .iter()
        .filter(|p| p.finalized && LiveCaptureTail::intersects(p, start, end))
        .cloned()
        .collect();
    if parts.is_empty() {
        return Err(LiveClipRejection::NothingFinalized {
            requested_start: start,
            finalized_end,
        });
    }
    Ok(parts)
}

fn source_span(path: &Path, start: MediaTime, end: MediaTime) -> SourceSpan {
    SourceSpan::new(
        SourceRef::from_locator(&path.display().to_string()),
        TimeBase::from_rate(1, 1_000).expect("1/1000 is a positive rational"),
        Interval::new(start, end).expect("the range is ordered, checked above"),
        Rate::ONE,
    )
    .expect("a non-empty source span")
}

/// A submitted live clip: the plan, the row, the job, and where its output
/// will land.
#[derive(Debug, Clone)]
pub struct LiveClipSubmission {
    pub parts: Vec<TailPart>,
    pub plan_id: RenderPlanId,
    pub variant_id: VariantId,
    pub job: Job,
    /// `immutable_output_path` for attempt 1: derived from the job id, so
    /// a retry cannot overwrite the attempt before it and no two jobs share
    /// one directory.
    pub output: PathBuf,
    pub start: MediaTime,
    pub end: MediaTime,
}

impl LiveClipSubmission {
    /// The job's identity, for a caller that wants to report it.
    #[must_use]
    pub fn job_id(&self) -> JobId {
        self.job.id
    }
}

impl Store {
    /// Compile a live clip and submit it as a job.
    ///
    /// The order is the whole contract: the tail is validated **first**, so
    /// no plan, row or job can exist for a clip that would have been refused
    /// for reaching into a part a recorder is still writing. A job row for an
    /// impossible render is worse than no row, because it is a durable claim
    /// somebody will retry.
    pub fn submit_live_clip(
        &self,
        request: &LiveClipRequest,
    ) -> std::result::Result<LiveClipSubmission, LiveClipSubmitError> {
        let parts = validate_live_clip(&request.tail, request.start, request.end)?;
        let plan = self
            .compile_and_store_live_plan(request, &parts)
            .map_err(map_research)?;
        let job = self.submit_live_job(request).map_err(map_research)?;
        Ok(LiveClipSubmission {
            parts,
            plan_id: plan.id,
            variant_id: plan.variant_id,
            output: immutable_output_path(&request.scratch_root, &job.id, 1),
            job,
            start: request.start,
            end: request.end,
        })
    }

    /// Compile the request through todo 23's compiler and store todo 23's
    /// row. Split out only so the ordering above reads as one step.
    fn compile_and_store_live_plan(
        &self,
        request: &LiveClipRequest,
        parts: &[TailPart],
    ) -> Result<RenderPlan> {
        let clips: Vec<ClipRequest> = parts
            .iter()
            .enumerate()
            .map(|(cut, part)| {
                let start = request.start.max(part.start);
                let end = request.end.min(add(part.start, part.duration));
                ClipRequest {
                    cut,
                    span: source_span(&part.path, start, end),
                    fade_in: MediaTime::ZERO,
                    fade_out: MediaTime::ZERO,
                    overlay_at: None,
                }
            })
            .collect();
        let cuts: Vec<usize> = (0..clips.len()).collect();
        let render_request = RenderRequest {
            subject: request.session.to_string(),
            revision: request.variant.composition_revision_id.to_string(),
            layers: vec![LayerSpec {
                id: "programme".into(),
                kind: LayerKind::Video,
                transform: Transform::default(),
                colour: Colour::default(),
                clips: cuts.clone(),
                visible: true,
                keyframes: Vec::new(),
            }],
            tracks: vec![TrackSpec {
                label: "programme".into(),
                bus: "master".into(),
                gain_db: 0.0,
                pan: 0.0,
                muted: false,
                solo: false,
                insert: String::new(),
                clips: cuts,
            }],
            transitions: Vec::new(),
            effects: Vec::new(),
            loudness: None,
            captions: CaptionSpec {
                mode: CaptionMode::None,
                style_json: "{}".into(),
                coordinate: Coordinate::Source,
                safe_area: true,
                cues: Vec::new(),
            },
            video: VideoIntent {
                filter_complex: String::new(),
                colour: None,
                frames: None,
            },
            container: Container {
                extension: "mkv".into(),
                video_codec: "copy".into(),
                audio_codec: "copy".into(),
                stream_copy: true,
            },
            profile: OutputProfile {
                name: "live-clip".into(),
                renderer: "ffmpeg".into(),
                renderer_version: tool_version(&request.tool_versions, "ffmpeg")
                    .unwrap_or_else(|| "unrecorded".into()),
            },
            clips,
        };
        let compiled = compile(&render_request)
            .map_err(|e| ResearchError::Validation(format!("live clip does not compile: {e}")))?;
        let row = compile_render_plan(request.project, &request.variant, &compiled, &request.now)?;
        self.insert_render_plan(&row)?;
        Ok(row)
    }

    /// Submit todo 24's job for a live clip. The cache key is derived from
    /// the source asset's identity, never from the segment paths: a file
    /// whose mtime moved is not a different input.
    fn submit_live_job(&self, request: &LiveClipRequest) -> Result<Job> {
        let assets = [request.source_asset];
        let session = request.session.to_string();
        let tool_versions: Vec<(&str, &str)> = request
            .tool_versions
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        let key = CacheKey::of(&JobInputs {
            subject_kind: "live_clip",
            subject_id: &session,
            assets: &assets,
            transcript_revisions: &[],
            composition_revision: Some(request.variant.composition_revision_id),
            settings: &request.settings,
            tool_versions: &tool_versions,
        });
        if let Some(existing) = self.reusable(&key)? {
            // The cache half of the contract: an unchanged set of inputs
            // must not render again.
            return Err(ResearchError::Validation(format!(
                "an artifact for these live-clip inputs already exists: {existing}"
            )));
        }
        self.submit_job(&super::store_jobs::NewJob {
            id: JobId::generate(),
            project_id: request.project,
            subject_kind: "live_clip".into(),
            subject_id: session.clone(),
            cache_key: key,
            scratch_root: request.scratch_root.clone(),
            max_attempts: MAX_ATTEMPTS,
            at: request.now.clone(),
        })
    }

    /// Finish a live clip: insert the artifact, validate its bytes, and
    /// point the job at it.
    ///
    /// `complete` is todo 24's and validates nothing itself — it trusts the
    /// artifact row — so the validation is called **before** it rather than
    /// after, because a job that reaches `done` is final and a state nothing
    /// can leave is a bad place to discover a truncated render.
    pub fn complete_live_clip(
        &self,
        submission: &LiveClipSubmission,
        output: &std::path::Path,
        duration: Option<MediaTime>,
        now: &str,
    ) -> Result<Artifact> {
        // `import::fingerprint` already prefixes the algorithm; the
        // validation error above it says the row "hashes to <hex>" beside
        // what it "claims", so the row must carry the same spelling.
        let content_hash = super::import::fingerprint(output)?;
        let bytes = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
        if bytes == 0 {
            return Err(ResearchError::Validation(
                "a live clip of zero bytes cannot be completed".into(),
            ));
        }
        let artifact = Artifact {
            id: ArtifactId::generate(),
            project_id: submission.job.project_id,
            variant_id: Some(submission.variant_id),
            render_plan_id: Some(submission.plan_id),
            kind: "clip".into(),
            content_hash,
            locator: Some(output.to_path_buf()),
            bytes: Some(bytes),
            width: None,
            height: None,
            duration,
            created_at: now.to_string(),
        };
        self.insert_artifact(&artifact)?;
        self.validate_artifact(artifact.id, now)?;
        self.complete(submission.job.id, artifact.id, now)?;
        Ok(artifact)
    }
}

fn tool_version(versions: &[(String, String)], name: &str) -> Option<String> {
    versions
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.clone())
}

/// A [`ResearchError`] from inside the submission, carried out of a function
/// whose *own* failure mode is typed. A store error is a different thing
/// from a refused clip and must not be flattened into one.
fn map_research(e: ResearchError) -> LiveClipSubmitError {
    LiveClipSubmitError::Store(e)
}

/// The one error a live-clip submission has.
#[derive(Debug)]
pub enum LiveClipSubmitError {
    /// The tail refused the request. Nothing was written.
    Rejected(LiveClipRejection),
    /// The store refused a write. The plan row may exist; the job does not.
    Store(ResearchError),
}

impl fmt::Display for LiveClipSubmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(r) => write!(f, "{r}"),
            Self::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LiveClipSubmitError {}

impl LiveClipSubmitError {
    /// The refusal, when this was a refusal. A store error is a different
    /// thing from a refused clip and must not be read as one, so a caller
    /// asking "was it the tail?" gets `None` rather than a guess.
    #[must_use]
    pub fn rejection(&self) -> Option<&LiveClipRejection> {
        match self {
            Self::Rejected(r) => Some(r),
            Self::Store(_) => None,
        }
    }
}

impl From<LiveClipRejection> for LiveClipSubmitError {
    fn from(r: LiveClipRejection) -> Self {
        Self::Rejected(r)
    }
}
