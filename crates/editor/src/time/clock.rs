//! Wall-clock to media-time conversion, with a stated uncertainty.
//!
//! A recording's wall clock and its media clock are two different clocks.
//! They agree to within the capture pipeline's tolerance, they diverge
//! across a reconnect, and they are not interchangeable: a chat message
//! stamped `2026-09-07T19:04:11Z` is a *wall-clock* fact until something says
//! how that instant sits on the media timeline, and
//! `docs/CREATOR-EDITION-REPORT-2026-09-07.md:196` is explicit that the two
//! are not the same thing.
//!
//! So a [`ClockMap`] is a list of [`ClockSegment`]s — one per capture part,
//! which is what `src/recording/segments.rs` leaves behind when a stream
//! drops and the recording resumes into a new file — and each answer carries
//! an uncertainty bound rather than a bare number:
//!
//! * a segment running at 1 wall microsecond per media microsecond — real
//!   time — is **exact** and reports zero uncertainty. That is what makes a
//!   clean capture provably mappable rather than approximately so;
//! * a segment that drifted reports the drift accumulated since its anchor,
//!   growing with distance from it, because a VFR or reconnecting capture
//!   does not drift uniformly over its length;
//! * an instant that falls in a gap — a wall-clock moment when nothing was
//!   being recorded, or a media position no part covers — is
//!   [`TimeError::OutsideClockMap`]. Media time was not advancing then, and
//!   the honest answer is that there is no coordinate, not the nearest edge.
//!
//! Each segment therefore records the media length it covers. Without it a
//! media-side gap is undetectable and the map would silently extend a segment
//! across it, which is the "clamp to the nearest edge" this module refuses.

use serde::{Deserialize, Serialize};

use super::interval_map::MappedTime;
use super::{Interval, MediaTime, TimeError};

/// Wall microseconds per media microsecond: the rate a clean capture runs at,
/// which is 1:1, because one microsecond of wall time passes in one
/// microsecond of real time.
const EXACT_NUM: i64 = 1;
const EXACT_DEN: i64 = 1;

/// One continuous stretch of capture, relating the two clocks across it.
///
/// `rate_num / rate_den` is **wall microseconds per media microsecond**, so
/// `1000/1` is real time and `1001/1000` is a capture that gains a
/// millisecond per second. The uncertainty is derived from the rate rather
/// than stored, so a caller cannot record a bound that contradicts the rate
/// it recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockSegment {
    /// Wall-clock milliseconds at which this segment's first media
    /// microsecond was captured.
    pub wall_start_ms: i64,
    /// Media time at `wall_start_ms`.
    pub media_start: MediaTime,
    /// How many microseconds of media time this segment covers.
    pub media_len: MediaTime,
    /// Wall microseconds per media microsecond.
    pub rate_num: i64,
    pub rate_den: i64,
}

impl ClockSegment {
    /// A segment running at a declared rate.
    #[must_use]
    pub fn new(
        wall_start_ms: i64,
        media_start: MediaTime,
        media_len: MediaTime,
        rate_num: i64,
        rate_den: i64,
    ) -> Self {
        Self {
            wall_start_ms,
            media_start,
            media_len,
            rate_num,
            rate_den,
        }
    }

    /// A segment captured in real time, so every conversion across it is
    /// exact.
    #[must_use]
    pub fn exact(wall_start_ms: i64, media_start: MediaTime, media_len: MediaTime) -> Self {
        Self::new(wall_start_ms, media_start, media_len, EXACT_NUM, EXACT_DEN)
    }

    /// The media range this segment covers.
    pub fn covered_media(&self) -> Result<Interval, TimeError> {
        Interval::new(
            self.media_start,
            self.media_start.checked_add(self.media_len)?,
        )
    }

    /// The wall-clock range this segment covers, in milliseconds. Derived,
    /// so a stored rate and a stored range cannot disagree.
    pub fn covered_wall_ms(&self) -> Result<(i64, i64), TimeError> {
        Ok((
            self.wall_start_ms,
            self.wall_of(self.covered_media()?.end())?,
        ))
    }

    /// The media coordinate of a wall-clock instant inside this segment, and
    /// the bound on how far it may be from the true one.
    pub fn media_at(&self, wall_ms: i64) -> Result<MappedTime, TimeError> {
        let wall_micros = wall_ms
            .checked_sub(self.wall_start_ms)
            .and_then(|ms| ms.checked_mul(1_000))
            .ok_or(TimeError::Overflow)?;
        // media = wall_micros * den / num
        let scaled = i128::from(wall_micros) * i128::from(self.rate_den);
        let media_micros =
            i64::try_from(scaled / i128::from(self.rate_num)).map_err(|_| TimeError::Overflow)?;
        Ok(MappedTime {
            at: self
                .media_start
                .checked_add(MediaTime::from_micros(media_micros)?)?,
            uncertainty_micros: self.uncertainty(media_micros, wall_micros),
        })
    }

    /// The wall-clock instant, in milliseconds, of a media coordinate inside
    /// this segment.
    pub fn wall_of(&self, at: MediaTime) -> Result<i64, TimeError> {
        self.wall_start_ms
            .checked_add(self.wall_offset_micros(at)?.div_euclid(1_000))
            .ok_or(TimeError::Overflow)
    }

    /// How many wall microseconds after this segment's anchor a media
    /// coordinate falls. Kept in microseconds rather than rounded to
    /// milliseconds, because the uncertainty is derived from it and a
    /// millisecond of rounding would be a millisecond of invisible drift.
    fn wall_offset_micros(&self, at: MediaTime) -> Result<i64, TimeError> {
        let media_micros = at.micros() - self.media_start.micros();
        // wall_micros = media_micros * num / den
        let scaled = i128::from(media_micros) * i128::from(self.rate_num);
        i64::try_from(scaled / i128::from(self.rate_den)).map_err(|_| TimeError::Overflow)
    }

    /// How far the two clocks have diverged since this segment's anchor, in
    /// microseconds: the difference between the wall time elapsed and the
    /// media time elapsed across it. Zero at an exact rate, and growing with
    /// distance for a drifting one, because a capture that gains a
    /// millisecond per second gains another millisecond per second after it.
    ///
    /// Both inputs are already in microseconds, so the difference is the
    /// divergence in the same unit — no rate algebra, and no way for the
    /// bound to disagree with the rate that produced it.
    fn uncertainty(&self, media_micros: i64, wall_micros: i64) -> i64 {
        if self.rate_num == EXACT_NUM && self.rate_den == EXACT_DEN {
            return 0;
        }
        (wall_micros - media_micros).abs()
    }

    fn validate(&self) -> Result<(), TimeError> {
        if self.rate_num <= 0 || self.rate_den <= 0 {
            return Err(TimeError::BadRational {
                num: self.rate_num,
                den: self.rate_den,
            });
        }
        if self.wall_start_ms < 0 {
            return Err(TimeError::Negative {
                micros: self.wall_start_ms,
            });
        }
        Ok(())
    }
}

/// A wall-clock answer: milliseconds since the Unix epoch, plus how far that
/// instant may be from the true one.
///
/// A distinct type from [`MappedTime`] on purpose. A wall-clock instant is
/// not a media coordinate, and a caller that could read one as the other
/// would silently place a chat message on the wrong frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WallTime {
    pub wall_ms: i64,
    pub uncertainty_micros: i64,
}

/// A capture's wall-clock to media-time relationship, as a list of segments.
///
/// Segments are appended in order and must not overlap on either clock.
/// Media time is contiguous across a reconnect even though the wall clock is
/// not, which is exactly why this is a list and not a single rate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockMap {
    segments: Vec<ClockSegment>,
}

impl ClockMap {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a segment. Refuses one that overlaps the previous segment on the
    /// media timeline or on the wall clock, because two segments claiming the
    /// same instant have no single answer and clamping to the first would
    /// invent one.
    pub fn push(&mut self, segment: ClockSegment) -> Result<(), TimeError> {
        segment.validate()?;
        if let Some(last) = self.segments.last() {
            if segment.media_start < last.covered_media()?.end() {
                return Err(TimeError::ClockDiscontinuity {
                    wall_start_ms: segment.wall_start_ms,
                });
            }
            if segment.wall_start_ms < last.covered_wall_ms()?.1 {
                return Err(TimeError::ClockOverlap {
                    wall_start_ms: segment.wall_start_ms,
                });
            }
        }
        self.segments.push(segment);
        Ok(())
    }

    #[must_use]
    pub fn segments(&self) -> &[ClockSegment] {
        &self.segments
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// The media coordinate of a wall-clock instant, with its uncertainty.
    /// An instant in a gap, or before the capture started, is an error.
    pub fn media_at(&self, wall_ms: i64) -> Result<MappedTime, TimeError> {
        let segment = self
            .segments
            .iter()
            .find(|s| {
                s.covered_wall_ms()
                    .is_ok_and(|(from, to)| from <= wall_ms && wall_ms <= to)
            })
            .ok_or(TimeError::OutsideClockMap { wall_ms })?;
        segment.media_at(wall_ms)
    }

    /// The wall-clock instant of a media coordinate, with its uncertainty. A
    /// media position no segment covers is an error, for the same reason.
    pub fn wall_of(&self, at: MediaTime) -> Result<WallTime, TimeError> {
        let segment = self
            .segments
            .iter()
            .find(|s| s.covered_media().is_ok_and(|i| i.contains(at)))
            .ok_or(TimeError::OutsideClockMap {
                wall_ms: at.micros(),
            })?;
        Ok(WallTime {
            wall_ms: segment.wall_of(at)?,
            uncertainty_micros: segment.uncertainty(
                at.micros() - segment.media_start.micros(),
                segment.wall_offset_micros(at)?,
            ),
        })
    }
}
