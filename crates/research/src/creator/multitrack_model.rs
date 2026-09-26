//! The vocabulary of a multitrack session: what a track is, what a refusal is,
//! and what an imported session is made of.
//!
//! Split out of [`super::multitrack`] because these are types and that module is
//! an operation. A caller reads the model to know what a track is and calls the
//! operation once; keeping them apart means neither file has to hold both, and a
//! change to a field does not read as a change to the import.
//!
//! **Every field here is a measurement or a bound derived from one.** There is no
//! `f64`, no millisecond duration and no seconds column: a position is a
//! [`MediaTime`], a rate is a [`TimeBase`] or an exact rational, and the accuracy
//! a caller may rely on is [`MediaGap::drift_after_micros`] plus, per hole, the
//! measured `keyframe_distance`.
//!
//! A `label` is untrusted text from an external recorder. It is carried verbatim,
//! never interpreted, and never handed to a shell.

use std::path::Path;

use strivo_editor::time::{ClockMap, MediaTime, TimeBase};
use strivo_multitrack::TrackKind;

use super::ids::{CaptureSessionId, ProjectId, SourceAssetId};
use super::import::{AdmittedFile, ImportManifest};
use super::multitrack_align::{self, MeasuredGap, MediaGap, SessionTimeline};
use super::multitrack_session::SessionAttributes;
use crate::ResearchError;

/// The most tracks one container may contribute.
///
/// Checked before any row is written, so the bound is a refusal rather than a
/// resource ceiling discovered halfway through. OBS's own multitrack recording
/// is six sources at most; 64 is two orders of magnitude of headroom, and a
/// session past it is far more likely to be a container that is not a
/// multitrack recording than one with 65 useful stems.
pub const MAX_TRACKS: usize = 64;

/// One stream's measured facts. What a probe read, and nothing inferred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackMeasurement {
    /// The container's stream index. Unique within a session; a duplicate is a
    /// measurement bug and is refused rather than silently collapsed.
    pub stream_index: u32,
    /// The name the recorder stamped on the stream.
    pub label: String,
    /// The stream's own rate: frames per second for a picture, samples per
    /// second for audio. Named, exact, and a real converter — not a description.
    pub time_base: TimeBase,
    /// The first presentation position, which for the streams of one container
    /// is a position on the session's own timeline.
    pub start: MediaTime,
    /// How much timeline the stream covers, holes excluded.
    pub coverage: MediaTime,
    /// Holes found by walking its presentation times, in stream-position order.
    pub gaps: Vec<MeasuredGap>,
    /// Whether the probe read the stream to its end. `false` means the scan hit
    /// its ceiling, so a gap past that point was never looked for and every
    /// claim below it rests on a partial measurement. A truncated scan is
    /// reported, never presented as a clean one.
    pub complete: bool,
}

/// Why a multitrack session was refused. Each is one decision a caller can act
/// on differently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultitrackRejection {
    /// The container declared no audio stream. A silent-video file is a legal
    /// recording and an illegal multitrack session.
    NoTracks,
    /// More streams than [`MAX_TRACKS`].
    TooManyTracks { found: usize, limit: usize },
    /// A stream with no measurable content, so there is no position to place it
    /// at and no length to align.
    EmptyTrack { stream: u32 },
    /// A hole that starts at or past the end of the track, or overlaps another.
    /// The measurement is wrong, and clipping it here would invent a hole that
    /// the bytes do not contain.
    GapOutsideTrack { stream: u32, at_micros: i64 },
    /// Two measurements claiming one stream index, so the track set has no
    /// stable address.
    DuplicateStream { stream: u32 },
    /// A label list that does not line up with the tracks, so a label either
    /// names nothing or names the wrong track.
    LabelCount { labels: usize, tracks: usize },
}

impl MultitrackRejection {
    /// Stable machine code, the key a caller branches on.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoTracks => "multitrack/no-tracks",
            Self::TooManyTracks { .. } => "multitrack/too-many-tracks",
            Self::EmptyTrack { .. } => "multitrack/empty-track",
            Self::GapOutsideTrack { .. } => "multitrack/gap-outside-track",
            Self::DuplicateStream { .. } => "multitrack/duplicate-stream",
            Self::LabelCount { .. } => "multitrack/label-count",
        }
    }
}

impl std::error::Error for MultitrackRejection {}

impl std::fmt::Display for MultitrackRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoTracks => f.write_str("the container declares no audio stream"),
            Self::TooManyTracks { found, limit } => {
                write!(f, "{found} streams is past the {limit}-stream limit")
            }
            Self::EmptyTrack { stream } => write!(f, "stream {stream} carries no content"),
            Self::GapOutsideTrack { stream, at_micros } => {
                write!(
                    f,
                    "stream {stream} has a hole at {at_micros} us, past its end"
                )
            }
            Self::DuplicateStream { stream } => {
                write!(f, "two measurements both claim stream {stream}")
            }
            Self::LabelCount { labels, tracks } => {
                write!(f, "{labels} labels for {tracks} tracks")
            }
        }
    }
}

/// Why a multitrack import did not produce a session.
///
/// A [`MultitrackRejection`] is a decision the caller made wrongly and can fix;
/// [`ResearchError`] is the store or the filesystem failing underneath. They
/// stay apart because only the first has a stable code a UI can branch on, and
/// collapsing them would leave the route guessing which it is looking at.
#[derive(Debug, thiserror::Error)]
pub enum MultitrackError {
    /// The measurements were refused. Nothing was written.
    #[error(transparent)]
    Rejected(#[from] MultitrackRejection),
    /// The store, the fingerprint or the filesystem failed.
    #[error(transparent)]
    Research(#[from] ResearchError),
}

impl MultitrackError {
    /// Stable machine code. A refusal the caller can act on names itself; a
    /// failure underneath is named as such rather than borrowing a code that
    /// would send a UI looking for a malformed file.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Rejected(r) => r.code(),
            Self::Research(_) => "multitrack/internal",
        }
    }
}

/// One multitrack session, as the caller states it. Grouped because these are
/// not independent parameters: the fingerprint is taken over the container, the
/// tracks describe that same container, and the job is submitted for the
/// project.
#[derive(Debug, Clone, Copy)]
pub struct MultitrackRequest<'a> {
    /// The project the session joins — the same collection a capture writes to.
    pub project: ProjectId,
    /// The container, already admitted. There is no `PathBuf` entry point and
    /// no string-parsing one either.
    pub container: &'a AdmittedFile,
    /// One measurement per audio stream in it.
    pub tracks: &'a [TrackMeasurement],
    /// The picture's measurement, where the container has a video stream. The
    /// session's own timeline and its holes come from it.
    pub picture: Option<&'a TrackMeasurement>,
    /// The user's labels, one per track in the same order, overriding what the
    /// container's own names were read to mean.
    pub labels: Option<&'a [String]>,
    /// The import job's scratch root. Nothing is written into it.
    pub scratch_root: &'a Path,
    /// When the import happened, as the caller's clock states it. No clock is
    /// read here, so the same request twice produces the same rows.
    pub at: &'a str,
}

/// A track: what it is, where it is, and how it lines up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentifiedTrack {
    /// The container stream index.
    pub stream_index: u32,
    /// The name in force: the user's where they gave one, else the container's.
    pub label: String,
    /// The name the container itself carried, so a later read can see what an
    /// override replaced rather than finding the original name gone.
    pub measured_label: String,
    /// The role. `Other` when nothing determined it.
    pub role: TrackKind,
    /// Whether `role` was determined rather than defaulted. A caller showing a
    /// colour per role must not paint `Other` as if it knew.
    pub identified: bool,
    /// The stream's own rate, exact.
    pub time_base: TimeBase,
    /// The source this track's bytes are registered under. Every track of one
    /// container is the same file, so this is where track identity lives —
    /// content identity cannot tell two stems of the same container apart.
    pub source: SourceAssetId,
    /// Where this track's content begins on the session timeline. The constant
    /// shift, stated rather than absorbed.
    pub offset: MediaTime,
    /// The alignment: the session's real-time axis in milliseconds against this
    /// track's own content positions, with a bound on every answer. One segment
    /// per continuous run, so a hole in the session answers `OutsideClockMap`
    /// rather than clamping to the nearest frame.
    pub aligned: ClockMap,
    /// Every hole in this track, head included, each with the bound a caller
    /// must carry for anything after it.
    pub gaps: Vec<MediaGap>,
    /// Whether the probe read this track to its end.
    pub complete: bool,
}

/// Which track the session's timeline is read through, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceTrack {
    /// The stream index chosen.
    pub stream_index: u32,
    /// Its label, so the choice is legible.
    pub label: String,
    /// Whether it is the downmixed fallback. The rule is fixed: the fallback
    /// when there is one, else the track covering the most timeline, ties going
    /// to the lowest stream index. A caller that trusts nothing else still has
    /// one synchronised timeline, and it is named rather than implied.
    pub mixed: bool,
}

/// The result of importing one multitrack session.
#[derive(Debug, Clone, PartialEq)]
pub struct MultitrackSession {
    /// Todo 33's manifest for the container. The stems were not imported as
    /// separate files, because they are not separate files.
    pub manifest: ImportManifest,
    /// The capture session this import's measurements were recorded against.
    pub capture: CaptureSessionId,
    /// The stored measurements — the alignment's *input*, never its output. See
    /// [`multitrack_session`] for why the derived form is re-computed on read
    /// rather than written down.
    pub attributes: SessionAttributes,
    /// The session's own timeline and the holes in the picture.
    pub timeline: SessionTimeline,
    /// The track the timeline is read through.
    pub reference: ReferenceTrack,
    /// Every identified, aligned track, in the order they were measured.
    pub tracks: Vec<IdentifiedTrack>,
}

impl MultitrackSession {
    /// The largest bound any answer in this session carries, in microseconds.
    /// The number a caller shows, so the drift is a figure rather than an
    /// assurance.
    #[must_use]
    pub fn worst_drift_micros(&self) -> i64 {
        let tracks = self.tracks.iter().map(|t| (&t.aligned, t.gaps.as_slice()));
        // The session's own timeline counts too. A hole in the *picture* is a
        // session-level fact no track carries — a continuous audio stem really
        // does have content at 1.0 s of a second whose picture does not — so
        // reading the bound off the tracks alone would report zero for a session
        // that is missing a second of video.
        multitrack_align::worst_bound(tracks.chain(std::iter::once((
            &self.timeline.clock,
            self.timeline.gaps.as_slice(),
        ))))
    }
}
