//! What a multitrack import persists, and how it is read back.
//!
//! Split out of [`super::multitrack`] because this is the *storage* half: the
//! measurements a probe took, kept in one place so a later reader can re-derive
//! the same alignment instead of trusting a stored copy of it.
//!
//! ## The measurements are stored; the alignment is not
//!
//! A [`SessionAttributes`] row is the **input** to the alignment — the origin,
//! and per stream its start, coverage, holes, role and timebase. The alignment
//! itself ([`super::multitrack_align`]) is a pure function of that row, and is
//! never written down.
//!
//! That is the whole reason there is no `aligned_json` column and no
//! `creator_clock_segments` row here. Storing the derived answer alongside its
//! input is two sources for one fact, and the first writer to update one without
//! the other leaves a project whose stored alignment disagrees with its stored
//! measurements — a defect nothing downstream can detect, because both halves
//! look well-formed. Re-deriving costs a few microseconds of arithmetic and
//! cannot drift.
//!
//! ## Where it lives, and why
//!
//! [`super::entities::CaptureSession`]'s `attributes_json`, whose own field
//! documents itself as *"Recording parts and event clocks"*. One multitrack
//! recording is one capture session, and the alternative — a new table — is a
//! migration, a `down` file, and a second place to look for a track. The
//! container's own [`SourceAsset`] row carries only what a *source* is: that it
//! is a multitrack recording and how long it runs. It cannot carry a role,
//! because a container has four and a row has one column — the first draft
//! wrote the role per track and the fourth track silently overwrote the first
//! three.
//!
//! [`VERSION`] is a field rather than a schema migration because this document
//! has no relational shape to migrate: an older build reading a newer one finds
//! a version it does not recognise and refuses, which is the same posture
//! [`crate::creator::schema::latest_version`] takes for the tables.

use serde::{Deserialize, Serialize};
use strivo_editor::time::MediaTime;
use strivo_multitrack::TrackKind;

use super::ids::{CaptureSessionId, ProjectId, SourceAssetId};
use super::multitrack_align::{self, Aligned};
use super::multitrack_model::TrackMeasurement;
use super::store::Store;
use super::CaptureSession;
use crate::Result;

/// The shape version of a stored session document. Bump it when a field changes
/// meaning; a reader that does not recognise it refuses rather than guessing.
pub const VERSION: u32 = 1;

/// One stream's measured facts, as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackAttributes {
    /// The container stream index.
    pub stream_index: u32,
    /// The name in force.
    pub label: String,
    /// The name the container itself carried.
    pub measured_label: String,
    /// [`TrackKind::as_str`], the one spelling of the role.
    pub role: String,
    /// Whether the role was determined rather than defaulted.
    pub identified: bool,
    /// Ticks per second, exactly.
    pub time_base: [i64; 2],
    /// The first presentation position.
    pub start_us: i64,
    /// How much timeline the stream covers, holes excluded.
    pub coverage_us: i64,
    /// Whether the probe read the stream to its end.
    pub complete: bool,
    /// Its holes, as measured.
    pub gaps: Vec<GapAttributes>,
}

/// One measured hole, as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GapAttributes {
    pub at_us: i64,
    pub len_us: i64,
    /// The measured keyframe distance across the hole.
    pub keyframe_us: i64,
}

/// A whole multitrack session's measurements, as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAttributes {
    /// Always [`VERSION`] in a row this build wrote.
    pub version: u32,
    /// The session's first media position; every track's session position is
    /// measured from it.
    pub origin_us: i64,
    /// How long the session runs.
    pub length_us: i64,
    /// Which stream the session's timeline is read through.
    pub reference_stream: u32,
    /// Whether that stream is the downmixed fallback.
    pub reference_mixed: bool,
    /// The largest bound any answer in the session carries, in microseconds.
    pub worst_drift_micros: i64,
    /// Every track, in measurement order.
    pub tracks: Vec<TrackAttributes>,
}

/// Every track re-placed on the session timeline, from a stored row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredSession {
    /// The measurements the row recorded.
    pub attributes: SessionAttributes,
    /// One entry per track, in the same order, with the placement re-derived.
    pub placements: Vec<Placement>,
}

/// One restored track's placement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub stream_index: u32,
    pub label: String,
    pub role: TrackKind,
    pub identified: bool,
    /// Where the track's content begins on the session timeline.
    pub offset: MediaTime,
    /// The alignment, re-derived from the stored measurements.
    pub aligned: multitrack_align::AlignedTrack,
}

/// The stored document for `tracks`, built from the measurements as they were
/// measured. Carries no derived alignment, so a stored row can never disagree
/// with what re-deriving it produces.
#[must_use]
pub fn document(
    origin: MediaTime,
    length: MediaTime,
    reference_stream: u32,
    reference_mixed: bool,
    worst_drift_micros: i64,
    tracks: &[TrackMeasurement],
    roles: &[(String, bool, String, String)],
) -> SessionAttributes {
    SessionAttributes {
        version: VERSION,
        origin_us: origin.micros(),
        length_us: length.micros(),
        reference_stream,
        reference_mixed,
        worst_drift_micros,
        tracks: tracks
            .iter()
            .zip(roles)
            .map(
                |(m, (role, identified, label, measured_label))| TrackAttributes {
                    stream_index: m.stream_index,
                    label: label.clone(),
                    measured_label: measured_label.clone(),
                    role: role.clone(),
                    identified: *identified,
                    time_base: [m.time_base.rate().0, m.time_base.rate().1],
                    start_us: m.start.micros(),
                    coverage_us: m.coverage.micros(),
                    complete: m.complete,
                    gaps: m
                        .gaps
                        .iter()
                        .map(|g| GapAttributes {
                            at_us: g.at.micros(),
                            len_us: g.len.micros(),
                            keyframe_us: g.keyframe_distance.micros(),
                        })
                        .collect(),
                },
            )
            .collect(),
    }
}

/// The measurements back, in the shape [`document`] takes.
pub fn measurements(attrs: &SessionAttributes) -> Result<Vec<TrackMeasurement>> {
    if attrs.version != VERSION {
        return Err(crate::ResearchError::Validation(format!(
            "multitrack session document is version {}, this build reads {VERSION}",
            attrs.version
        )));
    }
    attrs
        .tracks
        .iter()
        .map(|t| {
            let (num, den) = (t.time_base[0], t.time_base[1]);
            Ok(TrackMeasurement {
                stream_index: t.stream_index,
                label: t.label.clone(),
                time_base: strivo_editor::time::TimeBase::from_rate(num, den)
                    .map_err(|e| crate::ResearchError::Validation(format!("{}: {e}", t.label)))?,
                start: micros(t.start_us, &t.label)?,
                coverage: micros(t.coverage_us, &t.label)?,
                complete: t.complete,
                gaps: t
                    .gaps
                    .iter()
                    .map(|g| {
                        Ok(multitrack_align::MeasuredGap {
                            at: micros(g.at_us, &t.label)?,
                            len: micros(g.len_us, &t.label)?,
                            keyframe_distance: micros(g.keyframe_us, &t.label)?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
            })
        })
        .collect()
}

fn micros(value: i64, what: &str) -> Result<MediaTime> {
    MediaTime::from_micros(value)
        .map_err(|e| crate::ResearchError::Validation(format!("{what}: {e}")))
}

/// Re-derive every track's placement from a stored row. The same arithmetic the
/// import ran, over the same measurements, so the two cannot disagree.
pub fn restore_multitrack_session(attrs: &SessionAttributes) -> Result<RestoredSession> {
    let origin = micros(attrs.origin_us, "session")?;
    let measured = measurements(attrs)?;
    let mut placements = Vec::with_capacity(measured.len());
    for (m, stored) in measured.iter().zip(&attrs.tracks) {
        let placed: Aligned = multitrack_align::align(m.start, m.coverage, &m.gaps, origin)
            .map_err(|e| crate::ResearchError::Validation(e.to_string()))?;
        let role = stored.role();
        placements.push(Placement {
            stream_index: m.stream_index,
            label: stored.label.clone(),
            role,
            identified: stored.identified,
            offset: placed.track.offset,
            aligned: placed.track,
        });
    }
    Ok(RestoredSession {
        attributes: attrs.clone(),
        placements,
    })
}

/// The role a stored word names, or `Other` for a word this build does not have.
///
/// Parsed from the row rather than re-inferred from the label: the stored word is
/// what the row says, and re-deriving it would be a second answer. A word from a
/// newer build reads as `Other`, which is the stated absence rather than a
/// silent substitution.
impl TrackAttributes {
    #[must_use]
    pub fn role(&self) -> TrackKind {
        [
            TrackKind::Mixed,
            TrackKind::Mic,
            TrackKind::Game,
            TrackKind::Discord,
            TrackKind::Music,
            TrackKind::Browser,
            TrackKind::Other,
        ]
        .into_iter()
        .find(|k| k.as_str() == self.role)
        .unwrap_or(TrackKind::Other)
    }
}

/// Read a stored multitrack session's measurements back.
///
/// Absent where the id names no capture session, which is `None` rather than an
/// error: a caller asking about a session that is not there is asking a
/// question, and an unrecognised *document* is an error.
pub fn read_multitrack_session(
    store: &Store,
    capture: CaptureSessionId,
) -> Result<Option<SessionAttributes>> {
    let Some(session) = store.capture_session(capture)? else {
        return Ok(None);
    };
    let attrs: SessionAttributes = serde_json::from_str(&session.attributes_json)
        .map_err(|e| crate::ResearchError::Validation(format!("multitrack session: {e}")))?;
    Ok(Some(attrs))
}

/// The capture-session row a multitrack import writes: the container as the
/// session's source, and the measurements in the field documented for them.
#[must_use]
pub fn session_row(
    id: CaptureSessionId,
    project: ProjectId,
    container: SourceAssetId,
    at: &str,
    attrs: &SessionAttributes,
) -> CaptureSession {
    CaptureSession {
        id,
        project_id: project,
        source_asset_id: Some(container),
        started_at: at.to_string(),
        ended_at: Some(at.to_string()),
        finalized: true,
        // A serialisation that failed would leave a row claiming to be a
        // multitrack session with nothing in it, which is worse than a refusal
        // upstream — so the caller checks the rendered document, not this.
        attributes_json: serde_json::to_string(attrs).unwrap_or_else(|_| "{}".into()),
        created_at: at.to_string(),
    }
}
