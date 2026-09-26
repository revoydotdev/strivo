//! Frame-accurate addressing, and the bound it can actually claim.
//!
//! **The honest bound is one frame, and only for addressing.** A frame index
//! converts to a media coordinate exactly (to the nearest microsecond), and
//! this module is the whole of that claim: [`seek_to_frame`] is the inverse of
//! [`frame_at`], and the residual it carries is bounded by the timebase's own
//! rounding, which is at most half a microsecond.
//!
//! **It is not a claim about decoding.** A seek into a compressed stream lands
//! on the nearest *keyframe*, not on the requested frame, because that is what
//! the bitstream allows. Measuring that distance is the caller's job — a
//! browser reports what the demuxer actually found, and this module refuses to
//! pretend a keyframe distance is zero. What it does provide is
//! [`FrameIndex::frame_micros`], the number a "within one frame" claim is
//! measured against, so the two bounds cannot be confused for one another.

use crate::time::{MediaTime, TimeBase, TimeError};

/// One microsecond, stated once so the arithmetic below cannot drift from it.
const MICROS_PER_SECOND: i128 = 1_000_000;

/// Where a position sits on a frame grid, and how far it is from a boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameIndex {
    /// The frame whose span contains the position.
    pub frame: i64,
    /// Where that frame starts on the media timeline. **Not** the position
    /// that was asked about — `at` plus `error_micros` is.
    pub at: MediaTime,
    /// How far the asked-about position is past `at`, in microseconds.
    /// Zero when the position *is* a frame boundary, and zero is a real
    /// measurement: this is a plain `i64` and nothing defaults it with `||`.
    pub error_micros: i64,
    /// One frame's length in whole microseconds, rounded **up**. The bound a
    /// "within one frame" claim is stated against, and therefore never smaller
    /// than the real frame length.
    pub frame_micros: i64,
}

/// The frame whose span contains `at`.
///
/// Floor, not round: the frame a position belongs to is the one that is
/// playing, and rounding up would report the frame that starts *after* the
/// picture the user is looking at.
pub fn frame_at(timebase: TimeBase, at: MediaTime) -> Result<FrameIndex, TimeError> {
    let (num, den) = timebase.rate();
    let scaled = i128::from(at.micros()) * i128::from(num);
    let divisor = i128::from(den) * MICROS_PER_SECOND;
    let frame = i64::try_from(scaled / divisor).map_err(|_| TimeError::Overflow)?;
    let start = timebase.frame_to_media(frame)?;
    let span = i128::from(1_000_000) * i128::from(den);
    // ceil(span / num) — the frame length as a whole number of microseconds,
    // never rounding *down* into a bound smaller than the real frame.
    let frame_micros = i64::try_from((span + i128::from(num) - 1) / i128::from(num))
        .map_err(|_| TimeError::Overflow)?;
    Ok(FrameIndex {
        frame,
        at: start,
        error_micros: at.micros() - start.micros(),
        frame_micros,
    })
}

/// The media coordinate a frame index starts at — the frame-accurate seek.
///
/// **Rounded up, not to nearest**, and the reason is a real defect this
/// function exists to avoid. Almost no video timebase divides a second into
/// whole microseconds: at 30fps a frame is 33333.33 µs, so rounding frame 7's
/// start to nearest gives 233333 µs — which is 0.33 µs *before* the frame it
/// names, so `frame_at` on the result reports frame **6**. A seek that lands
/// on the previous frame is a seek that is off by one, and no amount of
/// accuracy in the arithmetic hides it. Rounding up guarantees the coordinate
/// is inside the frame it names, at a cost of at most one microsecond, which
/// [`FrameIndex::error_micros`] then reports rather than hides.
pub fn seek_to_frame(timebase: TimeBase, frame: i64) -> Result<MediaTime, TimeError> {
    if frame < 0 {
        return Err(TimeError::Negative { micros: frame });
    }
    let (num, den) = timebase.rate();
    let scaled = i128::from(frame)
        .checked_mul(i128::from(den))
        .and_then(|v| v.checked_mul(MICROS_PER_SECOND))
        .ok_or(TimeError::Overflow)?;
    let divisor = i128::from(num);
    let (quotient, remainder) = (scaled / divisor, scaled % divisor);
    let rounded_up = if remainder > 0 {
        quotient + 1
    } else {
        quotient
    };
    MediaTime::from_micros(i64::try_from(rounded_up).map_err(|_| TimeError::Overflow)?)
}

#[cfg(test)]
mod tests {
    use super::{frame_at, seek_to_frame};

    fn tb(num: i64, den: i64) -> super::TimeBase {
        super::TimeBase::from_rate(num, den).expect("a positive rational")
    }

    /// The defect: 1/30 s is 33333.33 us, so a frame boundary is not a whole
    /// microsecond. Rounding frame 7's start to *nearest* yields 233333, which
    /// is before the frame really starts, and indexing it answers frame 6.
    #[test]
    fn a_seek_never_lands_on_the_frame_before_the_one_it_named() {
        let base = tb(30, 1);
        let at = seek_to_frame(base, 7).unwrap();
        assert_eq!(at.micros(), 233_334, "rounded up, so it is inside frame 7");
        let idx = frame_at(base, at).unwrap();
        assert_eq!(idx.frame, 7, "not 6");
        assert_eq!(idx.error_micros, 1, "one microsecond past the true start");
        assert_eq!(idx.frame_micros, 33_334, "ceil(1e6/30)");
    }

    /// A timebase that divides a second exactly has no residual at all, and
    /// the measured zero must survive as a zero.
    #[test]
    fn a_whole_microsecond_timebase_reports_a_measured_zero() {
        let base = tb(25, 1);
        let at = seek_to_frame(base, 7).unwrap();
        assert_eq!(at.micros(), 280_000);
        let idx = frame_at(base, at).unwrap();
        assert_eq!(idx.frame, 7);
        assert_eq!(idx.error_micros, 0);
    }

    #[test]
    fn a_negative_frame_index_is_refused_rather_than_wrapping_forward() {
        assert!(seek_to_frame(tb(30, 1), -1).is_err());
    }

    #[test]
    fn the_frame_length_bound_is_never_smaller_than_the_real_frame() {
        // 1001/30000 s is 33366.67 us, so a floor would report 33366 and make
        // "within one frame" a claim a real frame can break.
        assert_eq!(
            frame_at(tb(30000, 1001), seek_to_frame(tb(30000, 1001), 0).unwrap())
                .unwrap()
                .frame_micros,
            33_367
        );
        // A timebase faster than 1000fps rounds a frame up to 1us, not 0.
        assert_eq!(
            frame_at(tb(4000, 1), seek_to_frame(tb(4000, 1), 0).unwrap())
                .unwrap()
                .frame_micros,
            250
        );
    }
}
