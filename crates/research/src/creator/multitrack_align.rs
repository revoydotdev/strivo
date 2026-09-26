//! Placing measured tracks on one session timeline, with the bound stated.
//!
//! Split out of [`super::multitrack`] because this half is pure arithmetic over
//! measurements and touches no store: it is the part whose every claim can be
//! checked without a database, and the part that has to be right for any claim
//! about synchronisation to be worth anything.
//!
//! ## The model, and the one claim it is willing to make
//!
//! **Every stream in one container is on one clock.** That is not an assumption
//! — a Matroska presentation timestamp is a position on the container's own
//! timeline, and every stream of a container is indexed against it. So a track's
//! alignment needs no searching, no cross-correlation and no drift fit: its
//! measured start is its offset, and the mapping between the session's timeline
//! and the track's content is **exact**, with a drift bound of zero. The rate
//! `1/1` says so inside the [`ClockSegment`], so the bound is derived from the
//! rate rather than stored beside it and cannot contradict it.
//!
//! ## What it is *not* claiming
//!
//! Exact in **container-timeline** terms. It does not claim a decoded-sample
//! position, because seeking into Matroska with `-c copy` lands on the nearest
//! keyframe (todo 23's documented limitation) and that distance is a property of
//! the encode, not of the clock. So every hole carries a **measured**
//! `keyframe_distance`, and a caller who needs a sample-accurate position gets
//! the number rather than the assurance.
//!
//! ## A hole is a hole
//!
//! A presentation-time jump produces two [`ClockSegment`]s with a wall-clock gap
//! between them, so a session position inside the gap answers
//! [`TimeError::OutsideClockMap`] — nothing was captured there, and the nearest
//! edge is not an answer. Describing such a track with one segment would have
//! swallowed the hole into a constant offset, which is the failure this module
//! exists to prevent. The bound that follows a hole is the content lost across
//! it, and it accumulates: a caller reasoning about anything after the second
//! hole is off by both, so the second carries the sum.
//!
//! ## The origin is the earliest position any stream reaches
//!
//! Not zero. A stream can start before the reference does, and a session axis
//! beginning at zero would put it at a negative position — which [`MediaTime`]
//! refuses, correctly. Shifting the origin to the earliest start keeps every
//! position representable, and is why `origin` is reported rather than assumed.

use strivo_editor::time::{ClockMap, ClockSegment, Interval, MediaTime, TimeError};

/// A measured hole in one stream's own presentation timeline.
///
/// Positions are that stream's presentation timestamps, which for the streams of
/// one container are positions on the session's timeline — the reason a probe
/// can report this knowing nothing about the other streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeasuredGap {
    /// The first presentation position nothing was recorded at.
    pub at: MediaTime,
    /// How much timeline is missing.
    pub len: MediaTime,
    /// How far a `-c copy` seek can land from the far side, measured as the
    /// distance to the next keyframe. Zero when the far side is itself a
    /// keyframe, which is what a recorder told to seek produces after a stall.
    pub keyframe_distance: MediaTime,
}

/// A hole as the alignment reports it, with the bound the alignment adds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaGap {
    /// Where on the session timeline the hole starts.
    pub at: MediaTime,
    /// How much of the session timeline the hole covers.
    pub len: MediaTime,
    /// The measured keyframe distance across it.
    pub keyframe_distance: MediaTime,
    /// How far anything after this hole may sit from where a continuous track
    /// would have put it: the content lost across this hole and every earlier
    /// one. Zero at the head of a session whose only gap is a track that started
    /// late, because a constant offset is not drift.
    pub drift_after_micros: i64,
}

/// One track placed on the session timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlignedTrack {
    /// Where the track's content begins on the session timeline. The constant
    /// shift, stated rather than absorbed.
    pub offset: MediaTime,
    /// The session's real-time axis in milliseconds against this track's own
    /// content positions, with a bound on every answer. One segment per
    /// continuous run, so a hole stays a hole.
    pub aligned: ClockMap,
    /// Every hole in this track, head included, each with its bound.
    pub gaps: Vec<MediaGap>,
}

/// The session's own timeline, which is the picture where there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTimeline {
    /// The session's first media position, the value every track's session
    /// position is measured from.
    pub origin: MediaTime,
    /// How long the session runs.
    pub length: MediaTime,
    /// The session axis against itself, carrying the picture's holes. A position
    /// inside one answers `OutsideClockMap`: the session really has nothing
    /// there.
    pub clock: ClockMap,
    /// The picture's holes, with their bounds.
    pub gaps: Vec<MediaGap>,
}

/// What `align` measured, so the caller need not re-derive it.
pub struct Aligned {
    /// The track's placement.
    pub track: AlignedTrack,
    /// The session position where the stream's content ends.
    pub last: MediaTime,
}

/// Place one measured stream on the session timeline whose origin is `origin`.
///
/// The measurements must already be validated: `gaps` in stream-position order,
/// inside `[0, coverage]`, none overlapping. [`super::multitrack`] is what
/// refuses the ones that are not, so an unvalidated gap here is a caller bug
/// rather than a runtime discovery.
///
/// A stream that starts at 300 ms has a 300 ms **head gap** and a zero drift
/// bound: the shift is constant, and a constant is not drift. A measured hole
/// produces one segment either side, and the session positions between them
/// belong to no segment — so `media_at` refuses them instead of clamping.
pub fn align(
    start: MediaTime,
    coverage: MediaTime,
    gaps: &[MeasuredGap],
    origin: MediaTime,
) -> Result<Aligned, TimeError> {
    let offset = MediaTime::from_micros(start.micros() - origin.micros())?;
    let last = offset.checked_add(coverage)?;

    // Each hole splits the content span, in stream positions. The session
    // position of a stream position is `offset` plus its distance from the
    // stream's own start, because one container is one timeline.
    let mut runs: Vec<(MediaTime, MediaTime)> = Vec::with_capacity(gaps.len() + 1);
    let mut reported: Vec<MediaGap> = Vec::with_capacity(gaps.len() + 1);
    let mut cursor = MediaTime::ZERO;
    let mut lost: i64 = 0;
    for gap in gaps {
        let at = MediaTime::from_micros(gap.at.micros() - start.micros())?;
        if at > cursor {
            runs.push((cursor, Interval::new(cursor, at)?.duration()?));
        }
        lost = lost.saturating_add(gap.len.micros());
        reported.push(MediaGap {
            at: offset.checked_add(at)?,
            len: gap.len,
            keyframe_distance: gap.keyframe_distance,
            drift_after_micros: lost,
        });
        cursor = at.checked_add(gap.len)?;
    }
    if cursor < coverage {
        runs.push((cursor, Interval::new(cursor, coverage)?.duration()?));
    }

    // The head a late start leaves, in the same terms as a measured hole.
    if offset > MediaTime::ZERO {
        reported.insert(
            0,
            MediaGap {
                at: MediaTime::ZERO,
                len: offset,
                keyframe_distance: MediaTime::ZERO,
                drift_after_micros: 0,
            },
        );
    }

    // One segment per run: `media_start` walks the track's own content, which is
    // contiguous across its holes, and `wall_start_ms` is the session position
    // the run begins at. Rate 1/1, so the uncertainty `ClockSegment` derives
    // from that rate is a measured zero rather than an assurance.
    let mut aligned = ClockMap::new();
    let mut content = MediaTime::ZERO;
    for (at, len) in runs {
        aligned.push(ClockSegment::exact(
            (offset.micros() + at.micros()) / 1_000,
            content,
            len,
        ))?;
        content = content.checked_add(len)?;
    }

    Ok(Aligned {
        track: AlignedTrack {
            offset,
            aligned,
            gaps: reported,
        },
        last,
    })
}

/// The largest bound any answer in these tracks carries, in microseconds.
///
/// The maximum over the hole bounds and each clock's own uncertainty, because a
/// caller that plans against the worst of them is right, and one that reads the
/// drift off the first track while another has more of it is not.
#[must_use]
pub fn worst_bound<'a>(tracks: impl IntoIterator<Item = (&'a ClockMap, &'a [MediaGap])>) -> i64 {
    tracks
        .into_iter()
        .flat_map(|(clock, gaps)| {
            gaps.iter()
                .map(|g| g.drift_after_micros)
                .chain(far_edge_uncertainty(clock))
        })
        .max()
        .unwrap_or(0)
}

/// The uncertainty the map reports at the end of a track, where a drifting one
/// has accumulated the most.
fn far_edge_uncertainty(clock: &ClockMap) -> Option<i64> {
    let last = clock.segments().last()?;
    let edge = last.media_start.checked_add(last.media_len).ok()?;
    Some(clock.wall_of(edge).ok()?.uncertainty_micros)
}

/// The session's own timeline.
///
/// The picture where there is one, and the longest track where there is not —
/// the identity, because reporting a session gap the session does not have
/// would be a second, invented answer.
pub fn timeline(
    origin: MediaTime,
    reference: &Aligned,
    picture: Option<&Aligned>,
) -> SessionTimeline {
    let source = picture.unwrap_or(reference);
    SessionTimeline {
        origin,
        length: source.last,
        clock: source.track.aligned.clone(),
        gaps: source.track.gaps.clone(),
    }
}
