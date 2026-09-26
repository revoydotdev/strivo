//! Reading presentation times, and the holes in them.
//!
//! The arithmetic half of the OBS multitrack probe: pure functions over what
//! ffprobe printed, split out of [`super::multitrack_probe`] so the media-child
//! half has nothing to get wrong in it and this half spawns nothing.
//!
//! ## Integer in, integer out
//!
//! Positions are integer `pts` in a stream's own `time_base`, converted with
//! `i128` integer arithmetic and no division by a float. A reported position is
//! therefore a whole number of microseconds that is exact for the container's
//! clock, and the container's tick — [`tick_micros`] — is the floor on how
//! precisely any of it can be stated. A timebase of `1/1000` cannot express
//! 20 µs, and saying so is better than printing 20.
//!
//! ## The hole rule
//!
//! The **first** measured step is the nominal one. A step more than
//! [`STEP_TOLERANCE`] times nominal is a hole, and the missing span is that step
//! less the nominal step. Four is deliberately loose: an AAC frame is 21.33 ms
//! and steps of 21 and 22 ms alternate, so a tight threshold would report every
//! frame of every audio track as a hole. The rule is O(1) in memory beyond the
//! readings themselves, which is what lets the caller read a whole session.
//!
//! With a regular encoder the reported length is exact — for
//! `fixtures/media/discontinuities.mkv`, a nominal 40 ms and a 1040 ms step give
//! exactly the 1000 ms of picture that is missing. With an irregular one it is
//! accurate to within one frame interval, because the nominal step is one
//! sample of that interval rather than its average.

use strivo_editor::time::{Interval, MediaTime, TimeBase};
use strivo_research::creator::MeasuredGap;

/// How many times the nominal step a real step may be before it is a hole.
pub const STEP_TOLERANCE: u64 = 4;

/// The smallest whole number of microseconds in one `time_base` tick, or `None`
/// for a timebase that is not a positive rational.
#[must_use]
pub fn tick_micros(time_base: &str) -> Option<i64> {
    let (num, den) = parse(time_base)?;
    // A timebase is `num/den` **seconds per tick**, so a tick is
    // `num * 1_000_000 / den` microseconds — `1/1000` is 1000 us, not one. The
    // first draft had these the other way round and reported a 300 ms offset as
    // 300 000 ms, which the fixture test caught immediately.
    //
    // `div_ceil` on i128 is still unstable and both operands are positive, so the
    // ceiling is the same one line of arithmetic.
    let scaled = num * 1_000_000;
    i64::try_from((scaled + den - 1) / den).ok()
}

/// An integer timestamp in a stream's `time_base`, as whole microseconds. Exact,
/// and `None` for a negative position or a timebase that makes no sense —
/// [`MediaTime`] has no negative, and inventing one by clamping is the
/// approximation this model exists to refuse.
#[must_use]
pub fn at(pts: i64, time_base: &str) -> Option<MediaTime> {
    let (num, den) = parse(time_base)?;
    let micros = i128::from(pts) * num * 1_000_000 / den;
    MediaTime::from_micros(i64::try_from(micros).ok()?).ok()
}

fn parse(time_base: &str) -> Option<(i128, i128)> {
    let (num, den) = time_base.split_once('/')?;
    let num: i128 = num.parse().ok()?;
    let den: i128 = den.parse().ok()?;
    (num > 0 && den > 0).then_some((num, den))
}

/// The span between two positions, or `None` when the second precedes the first.
#[must_use]
pub fn span(from: MediaTime, to: MediaTime) -> Option<MediaTime> {
    Interval::new(from, to).ok()?.duration().ok()
}

/// Every `(stream, pts, is_keyframe)` reading in ffprobe's `compact` output, in
/// order.
///
/// **`compact`, not `csv`.** Two things the CSV writer does that would have made
/// a positional parser wrong rather than merely fragile, both found by reading
/// the real output of `fixtures/media/discontinuities.mkv`:
///
/// * it emits the requested fields in ffprobe's own order, not the order they
///   were asked for, so a positional read of `frame=key_frame,pts,stream_index`
///   gets `key_frame, ???, stream_index`;
/// * a frame carrying side data is printed as an **extra** line, so the first
///   frame of that fixture appears twice — once as a bare row and once with the
///   SEI message appended. Counting rows would have reported 31 frames for 30.
///
/// `compact` names every field, so the order cannot matter, and it folds side
/// data into the frame's own row. A field whose value contains `=` — the side
/// data blob does — is split at the first `=` and simply matches no name here.
#[must_use]
pub fn read_frames(compact: &str) -> Vec<(u32, i64, bool)> {
    compact
        .lines()
        .filter_map(|line| {
            let mut stream = None;
            let mut pts = None;
            let mut key = false;
            for field in line.trim().split('|') {
                let Some((name, value)) = field.split_once('=') else {
                    continue;
                };
                match name {
                    "stream_index" => stream = value.trim().parse().ok(),
                    "pts" => pts = value.trim().parse().ok(),
                    "key_frame" => key = value.trim() == "1",
                    _ => {}
                }
            }
            Some((stream?, pts?, key))
        })
        .collect()
}

/// One stream's presentation times, as a start, a coverage and its holes.
pub struct Reading {
    /// The first presentation position.
    pub start: MediaTime,
    /// The content span: first frame to last frame plus one nominal step, so the
    /// last frame's own duration is inside it.
    pub coverage: MediaTime,
    /// The holes, in order.
    pub gaps: Vec<MeasuredGap>,
    /// Whether the stream was read at all. `false` means no presentation time
    /// was readable, so nothing is claimed — not that the stream is clean.
    pub complete: bool,
}

/// Read one stream out of the frame table.
#[must_use]
pub fn walk(readings: &[(u32, i64, bool)], index: u32, time_base: &str) -> Reading {
    let pts: Vec<(i64, bool)> = readings
        .iter()
        .filter(|(s, _, _)| *s == index)
        .map(|(_, p, k)| (*p, *k))
        .collect();
    let Some(first) = pts.first().map(|(p, _)| *p) else {
        return Reading {
            start: MediaTime::ZERO,
            coverage: MediaTime::ZERO,
            gaps: Vec::new(),
            complete: false,
        };
    };
    let Some(start) = at(first, time_base) else {
        return Reading {
            start: MediaTime::ZERO,
            coverage: MediaTime::ZERO,
            gaps: Vec::new(),
            complete: false,
        };
    };
    let threshold = i64::try_from(STEP_TOLERANCE).unwrap_or(i64::MAX);
    let mut nominal = 0_i64;
    let mut gaps = Vec::new();
    let mut previous = first;
    for (i, (now, _)) in pts.iter().enumerate().skip(1) {
        let step = now - previous;
        if nominal == 0 {
            nominal = step.max(0);
        }
        if step > nominal * threshold {
            // The hole runs from the frame that *should* have been there to the
            // one that is, so it starts a nominal step after the previous frame
            // and is `step - nominal` long. The first draft anchored it at the
            // previous frame, which shifted both ends a step early: 560..1560
            // instead of 600..1600, and the segments either side did not meet the
            // container's own timestamps. Found by measuring
            // `fixtures/media/discontinuities.mkv` rather than by constructing
            // the values, which is the only reason it was found at all.
            let hole = at(previous, time_base)
                .zip(at(step, time_base))
                .zip(at(nominal, time_base))
                .and_then(|((from, step), nominal)| {
                    Some((from.checked_add(nominal).ok()?, span(nominal, step)?))
                });
            if let Some((at, len)) = hole {
                gaps.push(MeasuredGap {
                    at,
                    len,
                    keyframe_distance: keyframe_run(&pts, i, time_base),
                });
            }
        }
        previous = *now;
    }
    let last = pts.last().map_or(first, |(p, _)| *p);
    // The last frame's own duration belongs inside the coverage, so the span
    // runs to one nominal step past it. A step that will not add is a container
    // at the end of the clock, and there the measured span stands alone.
    let coverage = match (at(last, time_base), at(nominal, time_base)) {
        (Some(last), Some(step)) => {
            span(start, last.checked_add(step).unwrap_or(last)).unwrap_or(MediaTime::ZERO)
        }
        _ => MediaTime::ZERO,
    };
    Reading {
        start,
        coverage,
        gaps,
        complete: true,
    }
}

/// How far past the far side of the hole at `i` the next keyframe lies. Zero
/// when the frame right after the hole is one, and zero again when there is no
/// later keyframe — which reads as "no measured keyframe ahead", not as "none
/// needed", so a caller that needs the distinction should look at the stream.
fn keyframe_run(pts: &[(i64, bool)], i: usize, time_base: &str) -> MediaTime {
    let Some(from) = pts.get(i).and_then(|(p, _)| at(*p, time_base)) else {
        return MediaTime::ZERO;
    };
    pts.iter()
        .skip(i)
        .find(|(_, key)| *key)
        .and_then(|(p, _)| at(*p, time_base))
        .and_then(|key| span(from, key))
        .unwrap_or(MediaTime::ZERO)
}

/// A stream's own rate: frames per second for a picture, samples per second for
/// audio. `avg_frame_rate` of `0/0` is what ffprobe prints for a stream it
/// cannot average, so a `None` there means "no rate", not a zero one.
#[must_use]
pub fn declared_base(stream: &serde_json::Value, kind: &str) -> Option<TimeBase> {
    let rational = |key: &str| -> Option<TimeBase> {
        let text = stream.get(key).and_then(serde_json::Value::as_str)?;
        let (num, den) = text.split_once('/')?;
        TimeBase::from_rate(num.parse().ok()?, den.parse().ok()?).ok()
    };
    if kind == "audio" {
        stream
            .get("sample_rate")
            .and_then(serde_json::Value::as_str)
            .and_then(|hz| hz.parse().ok())
            .and_then(|hz: i64| TimeBase::from_rate(hz, 1).ok())
            .or_else(|| rational("avg_frame_rate"))
    } else {
        rational("avg_frame_rate").or_else(|| rational("r_frame_rate"))
    }
}
