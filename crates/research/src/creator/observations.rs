//! The observation series: viewer samples and stream events, placed on the
//! source clock with a stated uncertainty, and a coverage figure that
//! names what it missed.
//!
//! Report item C04 (`docs/CREATOR-EDITION-REPORT-2026-09-07.md:226-229`).
//!
//! **A viewer sample is a wall-clock observation, and the two clocks are not
//! the same thing.** The value comes from a provider poll that happened at
//! some wall-clock instant; where that instant sits on the media timeline is
//! a question only the [`ClockMap`] can answer, and the answer carries an
//! uncertainty bound. So every sample in this module is built by
//! [`ViewerSample::observe`], which takes the map and cannot be handed a
//! position by a caller. There is no constructor that accepts a
//! [`MediaTime`] directly, because that is how a fabricated precision gets
//! in.
//!
//! **The bound is derived, never stored as an opinion.** A poll that ran
//! every 5 s is a statement about the preceding 5 s, so its position is
//! uncertain by at least 5 s *even on a perfectly-running clock* — the
//! provider never told us where inside its own window the value applied. On
//! top of that sits the clock's own drift across the window. An instantaneous
//! [`StreamEvent`] on an exact segment therefore has a bound of zero, and
//! zero there is a measurement, not a default.
//!
//! **A gap is a refusal, not the nearest edge.** An instant no clock segment
//! covers — a reconnect hole, or the far side of a session whose recording
//! stopped — is [`ObservationError::OutsideClockMap`], todo 22's own answer
//! arriving through this seam unchanged. Nothing is clamped, and nothing is
//! defaulted to the first frame: a viewer count placed at the wrong instant
//! is a fabricated measurement, and a series built from one is worse than an
//! empty series because it looks complete.
//!
//! **Coverage is a fraction with a denominator and a list of reasons, and the
//! two are not separable.** [`Coverage::validated`] refuses any figure below
//! full that carries no gaps, so "87% covered" cannot exist without saying
//! which intervals are the missing 13%. [`GapReason`] distinguishes the two
//! causes that a bare fraction would merge: [`GapReason::ReconnectGap`] is an
//! interval no capture covered, and [`GapReason::DroppedSample`] is an
//! interval the capture covered but the polls stopped in. Those are
//! different operational facts and a report that adds them together is
//! lying about its own cause.
//!
//! **Live status is not an outcome, and the schema is what says so.** The
//! only viewer field anywhere in the daemon is `concurrent_viewers` at
//! `src/platform/youtube.rs:118,520-522`, a badge value read from a live
//! status endpoint. It is not a measured outcome of published work, so it
//! must never reach `creator_metric_observations` — that table's subject is
//! a provider's measurement over a closed interval. The refusal is a
//! `BEFORE INSERT` **trigger** in the schema rather than a Rust `if`: a
//! constraint a caller can route around is not a constraint, and the
//! observation table is written by import adapters this crate does not own.

use strivo_editor::time::{ClockMap, MediaTime};

use super::observation_error::ObservationError;

/// The largest batch one append will take. The write is O(batch) and one
/// transaction, so the batch is the whole cost; a caller polling for a year
/// in one call is a missing pager, not a bigger batch.
///
/// ponytail: a fixed cap rather than a deadline, because this is a
/// synchronous SQLite write with no I/O to block on. Add a deadline if a
/// network-backed observation source ever appears.
pub const MAX_BATCH: usize = 4_096;

/// The largest event label or category, in bytes. Bounded because a label
/// is untrusted text and an unbounded one is a way to fill a disk. Over the
/// limit is a [`ObservationError::LabelTooLong`], never a truncation: a
/// shortened label reads exactly like a whole one.
pub const MAX_LABEL_BYTES: usize = 4_096;

/// The largest metric name, in bytes. Same reasoning as [`MAX_LABEL_BYTES`].
pub const MAX_METRIC_BYTES: usize = 256;

/// A named metric, of which a viewer sample is one. Bounded like a label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricName(String);

impl MetricName {
    /// The metric whose only source is the platform's live-status endpoint.
    /// Named here so a reader does not have to go and find
    /// `src/platform/youtube.rs:118` to learn that this one is a badge
    /// reading rather than a measurement.
    pub const CONCURRENT_VIEWERS: &'static str = "concurrent_viewers";

    /// Adopt a name, refusing an empty or oversized one.
    pub fn parse(raw: &str) -> Result<Self, ObservationError> {
        if raw.len() > MAX_METRIC_BYTES {
            return Err(ObservationError::MetricNameTooLong {
                bytes: raw.len(),
                limit: MAX_METRIC_BYTES,
            });
        }
        if raw.is_empty() {
            return Err(ObservationError::EmptyMetricName);
        }
        Ok(Self(raw.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The interval a capture run is expected to cover.
///
/// It is both the session's own bounds — a sample outside it is refused — and
/// the **denominator** of every coverage figure taken from it. There is no
/// way to ask for a coverage figure without naming the interval it is a
/// fraction of, which is the property that makes a coverage number mean
/// something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureWindow {
    pub start_wall_ms: i64,
    pub end_wall_ms: i64,
}

impl CaptureWindow {
    /// The expected interval. Inverted bounds are refused rather than read
    /// as a negative span.
    pub fn new(start_wall_ms: i64, end_wall_ms: i64) -> Result<Self, ObservationError> {
        if end_wall_ms <= start_wall_ms {
            return Err(ObservationError::InvertedWindow {
                start_wall_ms,
                end_wall_ms,
            });
        }
        Ok(Self {
            start_wall_ms,
            end_wall_ms,
        })
    }

    /// The denominator, in whole milliseconds.
    #[must_use]
    pub fn span_wall_ms(&self) -> i64 {
        self.end_wall_ms - self.start_wall_ms
    }

    #[must_use]
    pub fn contains(&self, wall_ms: i64) -> bool {
        wall_ms >= self.start_wall_ms && wall_ms <= self.end_wall_ms
    }
}

/// Where an observation sits on the media timeline, and how far from there
/// it may really be.
///
/// The two fields are one value because a position without a bound is the
/// defect this type exists to prevent: a consumer that reads `media` and
/// never learns `uncertainty_micros` has been handed a fabricated precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SamplePosition {
    pub media: MediaTime,
    /// Whole microseconds. Never negative, and never absent.
    pub uncertainty_micros: i64,
}

/// Why a metric has no value.
///
/// A rate limit and a missing grant are the same shape to a consumer — no
/// number — and completely different things to whoever has to fix it, so
/// they are not one variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnavailableReason {
    /// The credential in hand does not carry the scope this metric needs.
    /// The QA case: a request for an unauthorised metric.
    PermissionNotHeld,
    /// No credential at all for the provider.
    NotConfigured,
    /// The provider has no such metric, so no permission would produce one.
    ProviderUnsupported,
    RateLimited {
        retry_after_ms: i64,
    },
}

impl UnavailableReason {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PermissionNotHeld => "permission_not_held",
            Self::NotConfigured => "not_configured",
            Self::ProviderUnsupported => "provider_unsupported",
            Self::RateLimited { .. } => "rate_limited",
        }
    }

    pub fn parse(raw: &str, detail: &str) -> Option<Self> {
        match raw {
            "permission_not_held" => Some(Self::PermissionNotHeld),
            "not_configured" => Some(Self::NotConfigured),
            "provider_unsupported" => Some(Self::ProviderUnsupported),
            "rate_limited" => {
                // The retry hint travels in the detail rather than a second
                // column, so there is one place a rate limit's particulars
                // live instead of two that can disagree.
                let retry_after_ms = detail
                    .rsplit_once("retry_after_ms=")
                    .and_then(|(_, rest)| rest.trim().parse().ok())
                    .unwrap_or(0);
                Some(Self::RateLimited { retry_after_ms })
            }
            _ => None,
        }
    }
}

/// Whether a metric yielded a value, and why not when it did not.
///
/// The distinction this type exists to keep: [`Self::Observed`] of `0` is a
/// measurement — nobody was watching — and [`Self::Unavailable`] is the
/// absence of a measurement. A series that conflated them would report an
/// ungranted permission as a stream that peaked at zero viewers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetricAvailability {
    Observed(u32),
    Unavailable {
        reason: UnavailableReason,
        detail: String,
    },
}

impl MetricAvailability {
    /// The value, or `None` when the metric was never measured. `None` here
    /// means "no measurement", which is not the same as `Some(0)`.
    #[must_use]
    pub fn observed(&self) -> Option<u32> {
        match self {
            Self::Observed(value) => Some(*value),
            Self::Unavailable { .. } => None,
        }
    }

    #[must_use]
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable { .. })
    }

    #[must_use]
    pub fn unavailable_reason(&self) -> Option<UnavailableReason> {
        match self {
            Self::Unavailable { reason, .. } => Some(*reason),
            Self::Observed(_) => None,
        }
    }
}

/// Place an instant on the media timeline, refusing the ones the map does
/// not cover and refusing the ones outside the expected window.
pub(super) fn place(
    window: &CaptureWindow,
    clock: &ClockMap,
    wall_ms: i64,
    poll_interval_ms: i64,
) -> Result<SamplePosition, ObservationError> {
    if wall_ms < window.start_wall_ms {
        return Err(ObservationError::BeforeSession { wall_ms });
    }
    if wall_ms > window.end_wall_ms {
        return Err(ObservationError::AfterSession {
            wall_ms,
            end_wall_ms: window.end_wall_ms,
        });
    }
    let at = clock.media_at(wall_ms)?;
    // The reading is a statement about the preceding `poll_interval_ms`, so
    // the window's own length is an irreducible part of the bound. The clock's
    // contribution is the larger of the drift at the reading and at the start
    // of its window; taking the max rather than the difference is what keeps
    // the bound conservative when the window starts before the segment does.
    let window_start = wall_ms - poll_interval_ms;
    let drift_at_window_start = clock
        .media_at(window_start)
        .map_or(0, |m| m.uncertainty_micros);
    Ok(SamplePosition {
        media: at.at,
        uncertainty_micros: poll_interval_ms
            .checked_mul(1_000)
            .and_then(|window_us| {
                window_us.checked_add(at.uncertainty_micros.max(drift_at_window_start))
            })
            .ok_or(ObservationError::BadPollInterval {
                millis: poll_interval_ms,
            })?,
    })
}
