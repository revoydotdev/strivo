//! The coverage figure: a fraction of a named interval, carrying the reasons
//! for whatever is missing.
//!
//! **A coverage number is meaningless without its denominator and its
//! reasons, and this module makes that structural rather than advisory.**
//! [`Coverage::validated`] refuses any figure below full that names no gap,
//! so a report cannot print "87% covered" without also saying which
//! intervals are the missing 13%.
//!
//! The fraction is **whole per mille**, not a float. `f64` would let a
//! complete capture print as `0.9999999999`, and the rounding is the only
//! part of this anybody reads. `1_000` is 100% and means exactly that.
//!
//! [`GapReason`] separates the two causes a bare fraction would merge: a
//! [`GapReason::ReconnectGap`] is an interval no capture covered, and a
//! [`GapReason::DroppedSample`] is one the capture covered and the polls
//! stopped in. Those need different fixes, so they are not one gap.
//!
//! `widest_uncertainty_micros` is carried because coverage answers a
//! different question from knowledge: two 30-second polls tile a 60-second
//! window, so coverage is 1 000 — and the worst any instant in it is known
//! to is 30 seconds.

use strivo_editor::time::ClockMap;

use super::ids::CaptureSessionId;
use super::observation_error::ObservationError;
use super::observations::CaptureWindow;
use super::store::Store;

/// The store's own result type, so a refusal stays typed to the caller.
type Result<T> = std::result::Result<T, ObservationError>;

/// Why an interval of the expected window carries no observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapReason {
    /// Nothing was collected at all, so the whole window is uncovered.
    NoSamples,
    /// No clock segment covers this interval: the capture was not running.
    ReconnectGap,
    /// The capture covered it and no poll reported in it.
    DroppedSample,
}

impl GapReason {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoSamples => "no_samples",
            Self::ReconnectGap => "reconnect_gap",
            Self::DroppedSample => "dropped_sample",
        }
    }
}

/// One interval of the expected window with nothing observed in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoverageGap {
    pub start_wall_ms: i64,
    pub end_wall_ms: i64,
    pub reason: GapReason,
}

/// How much of the expected window was observed, and what the rest was.
///
/// The fraction is **whole per mille**, not a float. `f64` would let a
/// figure print as `0.9999999999` for a capture that is complete to the
/// microsecond, and the rounding is the only part of this that anybody would
/// read. `1_000` is 100% and means exactly that.
///
/// `widest_uncertainty_micros` is here because coverage answers a different
/// question from knowledge. Two 30-second polls tile a 60-second window, so
/// coverage is 1 000 — and the worst position any instant in it is known to
/// is 30 seconds. A report that carried only the fraction would read as
/// "everything observed, therefore everything known", which is false by a
/// factor of the poll interval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coverage {
    /// The denominator, in whole milliseconds.
    pub expected_wall_ms: i64,
    /// The union of every sample window that landed inside it.
    pub observed_wall_ms: i64,
    /// `observed / expected` in whole per mille, never above 1 000.
    pub per_mille: u32,
    /// Every uncovered interval, with its cause. A figure below 1 000
    /// without these is refused by [`Self::validated`].
    pub gaps: Vec<CoverageGap>,
    /// The largest per-sample bound in the series, in microseconds.
    pub widest_uncertainty_micros: i64,
}

impl Coverage {
    /// Whether this figure is one a report may print.
    ///
    /// Two refusals, both structural: a fraction above 1 000 is not a
    /// fraction, and a shortfall with no gaps is a figure that cannot say
    /// what it is short of. The second is the one this todo exists for — a
    /// coverage number with no denominator and no reasons is the failure.
    pub fn validated(&self) -> Result<&Self> {
        if self.per_mille > 1_000 {
            return Err(ObservationError::CoverageOutOfRange {
                per_mille: self.per_mille,
            });
        }
        if self.per_mille < 1_000 && self.gaps.is_empty() {
            return Err(ObservationError::UnexplainedShortfall {
                per_mille: self.per_mille,
            });
        }
        Ok(self)
    }
}

impl Store {
    /// How much of `window` the session's samples actually observed.
    ///
    /// Every figure here is derived from the rows. The gaps are the
    /// complement of the union of the sample windows inside the expected
    /// window, and each is classified by asking the *clock* rather than
    /// guessing: an interval no segment covers was never recorded, and an
    /// interval a segment does cover is one where the polls stopped. The two
    /// need different fixes, so they are not the same gap.
    pub fn coverage(
        &self,
        session: CaptureSessionId,
        window: &CaptureWindow,
        clock: &ClockMap,
    ) -> Result<Coverage> {
        let samples = self.viewer_samples(session)?;
        let start = window.start_wall_ms;
        let end = window.end_wall_ms;

        let mut covered: Vec<(i64, i64)> = samples
            .iter()
            .filter(|s| !s.dropped)
            .map(|s| {
                (
                    (s.wall_ms - s.poll_interval_ms).max(start),
                    s.wall_ms.min(end),
                )
            })
            .filter(|(lo, hi)| hi > lo)
            .collect();
        covered.sort_unstable();

        let mut merged: Vec<(i64, i64)> = Vec::with_capacity(covered.len());
        for span in covered {
            match merged.last_mut() {
                Some(last) if span.0 <= last.1 => last.1 = last.1.max(span.1),
                _ => merged.push(span),
            }
        }

        let mut gaps = Vec::new();
        let mut cursor = start;
        for (lo, hi) in &merged {
            if *lo > cursor {
                gaps.push(self.classify(cursor, *lo, clock));
            }
            cursor = cursor.max(*hi);
        }
        if cursor < end {
            gaps.push(self.classify(cursor, end, clock));
        }
        if samples.is_empty() {
            gaps.clear();
            gaps.push(CoverageGap {
                start_wall_ms: start,
                end_wall_ms: end,
                reason: GapReason::NoSamples,
            });
        }

        let observed_wall_ms: i64 = merged.iter().map(|(lo, hi)| hi - lo).sum();
        let expected_wall_ms = window.span_wall_ms();
        let per_mille = (observed_wall_ms * 1_000 / expected_wall_ms.max(1)).min(1_000) as u32;
        let coverage = Coverage {
            expected_wall_ms,
            observed_wall_ms,
            per_mille,
            gaps,
            widest_uncertainty_micros: samples
                .iter()
                .map(|s| s.position.uncertainty_micros)
                .max()
                .unwrap_or(0),
        };
        coverage.validated()?;
        Ok(coverage)
    }

    /// Name an uncovered interval's cause by asking the clock map, not by
    /// inferring it from the samples around it.
    ///
    /// The probe is the interval's **midpoint**, not its start, and that is
    /// not a detail. `ClockMap::covered_wall_ms` closes a segment's wall
    /// range *inclusively* — todo 34 measured and documented that a run's
    /// last millisecond overlaps the next one's first — so a reconnect hole
    /// beginning exactly on a segment boundary answers at its first
    /// millisecond and would be misreported as dropped polls. Probing the
    /// start of the gap is therefore wrong for every gap that begins where a
    /// capture part ends, which is every one of them.
    ///
    /// A gap narrower than the probe's own resolution — under a millisecond,
    /// at a boundary — is classified by its midpoint and may be reported as
    /// `DroppedSample`. Such a gap is a rounding error in a per-mille figure
    /// that cannot see it either, and the honest alternative is an O(1 ms)
    /// scan per gap to defend against a case that cannot move the number.
    fn classify(&self, from: i64, to: i64, clock: &ClockMap) -> CoverageGap {
        let midpoint = from + (to - from) / 2;
        let reason = match clock.media_at(midpoint) {
            Ok(_) => GapReason::DroppedSample,
            Err(_) => GapReason::ReconnectGap,
        };
        CoverageGap {
            start_wall_ms: from,
            end_wall_ms: to,
            reason,
        }
    }
}
