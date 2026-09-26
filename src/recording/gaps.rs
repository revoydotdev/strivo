//! Missing-interval ledger for interrupted capture (C06).
//!
//! The gap-resume substrate in [`super::segments`] knows *how* to keep
//! recording after a drop: one independent segment per FFmpeg invocation,
//! merged back with `mkvmerge`. It has never recorded *what was lost*.
//! A recording that silently dropped forty seconds and then merged its
//! parts looks identical on disk to one that never dropped, so a user has
//! no way to learn that a chunk of their stream is missing.
//!
//! This module closes that. Two independent accounts of the same capture:
//!
//! - the **ledger** — what the recorder wrote at the moment of each
//!   interruption, with the reason and the measured interval;
//! - the **timeline** — wall-clock elapsed, versus the stream duration
//!   `ffprobe` measures on the concatenated media.
//!
//! [`GapLedger::audit`] differences them. A hole the timeline proves and
//! the ledger does not explain is an *unreported* gap; a hole the ledger
//! claims that the timeline does not show is a *spurious* one. Both are
//! returned, because an audit that can only fail in one direction is an
//! audit that passes on a ledger which reports nothing at all.
//!
//! The cross-check determines the **length** of a hole, not where in the
//! capture it sits — see [`MissingInterval::start_ms`].

use std::fmt;

/// Why capture stopped mid-stream.
///
/// The stall variant is the one `STALL_TIMEOUT_SECS` in
/// [`super::mod`](crate::recording) produces: a frozen recorder writes no
/// bytes, so the capture timeline advances while the media does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapReason {
    /// The recorder process exited non-zero while the channel was still
    /// live — the ordinary disconnect, handled by the gap-resume retry.
    RecorderExit,
    /// No byte growth for the stall timeout; the recorder was stopped so
    /// the retry path could run.
    Stall,
    /// The daemon restarted and recovered the job as interrupted.
    Restart,
    /// Capture was stopped deliberately to keep the volume writable.
    DiskPressure,
}

impl GapReason {
    /// Stable machine token, for evidence files and log greps.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RecorderExit => "recorder-exit",
            Self::Stall => "stall",
            Self::Restart => "restart",
            Self::DiskPressure => "disk-pressure",
        }
    }
}

impl fmt::Display for GapReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A stretch of capture wall-clock that no media covers.
///
/// `reason` is what separates the two populations this type carries. A
/// ledger entry always has one. An interval the audit derived from the
/// timeline alone carries `None`, and that absence **is** the finding:
/// the hole is real and nothing said why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissingInterval {
    /// Start, in milliseconds from the first captured byte.
    ///
    /// For a ledger entry this is measured at the interruption. For an
    /// audit residual it is the *earliest* position the hole could occupy
    /// given what is already accounted for — the audit derives the
    /// length, not the position, and this field must not be read as a
    /// located offset.
    pub start_ms: u64,
    /// End, exclusive, on the same clock as [`Self::start_ms`].
    pub end_ms: u64,
    /// `None` for a hole nothing reported.
    pub reason: Option<GapReason>,
}

impl MissingInterval {
    /// Length of the interval. Saturating: a reversed pair is a
    /// programming error upstream, and reporting zero is safer than
    /// wrapping a `u64`.
    pub fn len_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

/// The measured shape of one capture, from the two independent accounts.
///
/// `media_ms` must come from `ffprobe` on the finished media, not from a
/// counter the recorder also wrote — a counter and the ledger are the
/// same witness, and a cross-check against one witness cannot fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureTimeline {
    /// Wall-clock from the first byte written to the last byte written,
    /// summed across every segment including the dead time between them.
    pub wall_clock_ms: u64,
    /// Stream duration of the concatenated media, measured with `ffprobe`.
    pub media_ms: u64,
    /// Slack allowed for measurement disagreement: the recorder's poll
    /// granularity, plus container timestamp rounding at segment joins.
    ///
    /// This is a stated ceiling, not a fudge factor. A residual larger
    /// than it is reported.
    pub tolerance_ms: u64,
}

/// The result of differencing a ledger against a timeline.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GapAudit {
    /// Holes the timeline proves and the ledger does not account for.
    pub unreported: Vec<MissingInterval>,
    /// Holes the ledger claims that the timeline does not support. An
    /// audit that cannot produce these accepts a ledger that reports
    /// every second of a six-hour capture as lost.
    pub spurious: Vec<MissingInterval>,
}

impl GapAudit {
    /// True when the ledger accounts for the timeline exactly, within the
    /// stated tolerance.
    pub fn is_clean(&self) -> bool {
        self.unreported.is_empty() && self.spurious.is_empty()
    }
}

/// Per-recording record of every interruption, and the cross-check that
/// holds it to account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GapLedger {
    entries: Vec<MissingInterval>,
}

impl GapLedger {
    /// An empty ledger. A capture that never dropped has one of these and
    /// its audit is clean, because the timeline shows no lost time.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an interruption the recorder observed. The interval is
    /// measured, not estimated: `start_ms` is the wall-clock of the last
    /// byte the segment received, `end_ms` the wall-clock of the first
    /// byte of its successor.
    ///
    /// An inverted or empty interval is rejected rather than stored, so a
    /// zero-length "gap" cannot pad a ledger into looking complete.
    pub fn note_gap(&mut self, start_ms: u64, end_ms: u64, reason: GapReason) -> bool {
        if end_ms <= start_ms {
            return false;
        }
        self.entries.push(MissingInterval {
            start_ms,
            end_ms,
            reason: Some(reason),
        });
        true
    }

    /// Every recorded interruption, in the order it was recorded.
    pub fn entries(&self) -> &[MissingInterval] {
        &self.entries
    }

    /// Total wall-clock the ledger claims was lost.
    pub fn reported_ms(&self) -> u64 {
        self.entries.iter().map(MissingInterval::len_ms).sum()
    }

    /// Wall-clock the measured media does not cover, before any ledger
    /// entry is subtracted. Zero when the media spans the capture.
    pub fn lost_ms(&self, timeline: &CaptureTimeline) -> u64 {
        timeline.wall_clock_ms.saturating_sub(timeline.media_ms)
    }

    /// Difference the two accounts.
    ///
    /// `unreported` is non-empty when the timeline lost more wall-clock
    /// than the ledger explains, beyond `tolerance_ms`. `spurious` is
    /// non-empty when the ledger claims more than the timeline lost,
    /// beyond the same tolerance — a ledger that reports a gap that never
    /// happened is as wrong as one that stays silent.
    pub fn audit(&self, timeline: &CaptureTimeline) -> GapAudit {
        let lost = self.lost_ms(timeline);
        let reported = self.reported_ms();
        let slack = timeline.tolerance_ms;
        let mut audit = GapAudit::default();

        if lost > reported.saturating_add(slack) {
            // The hole sits after everything already accounted for. The
            // length is exact; the position is the earliest one the
            // accounted-for prefix leaves room for.
            let accounted = timeline.media_ms.saturating_add(reported);
            audit.unreported.push(MissingInterval {
                start_ms: accounted,
                end_ms: timeline.wall_clock_ms,
                reason: None,
            });
        }

        if reported > lost.saturating_add(slack) {
            // Measured against `lost + slack`, the same baseline the branch
            // condition uses: subtracting `slack` twice would flag the
            // tolerance this branch just excused.
            let mut excess = reported - lost - slack;
            for entry in self.entries.iter().rev() {
                if excess == 0 {
                    break;
                }
                let len = entry.len_ms();
                if len <= excess {
                    excess -= len;
                    audit.spurious.push(*entry);
                } else {
                    audit.spurious.push(MissingInterval {
                        start_ms: entry.end_ms - excess,
                        end_ms: entry.end_ms,
                        reason: entry.reason,
                    });
                    excess = 0;
                }
            }
        }

        audit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Six hours in milliseconds — the acceptance window, stated once so
    /// no test re-derives it.
    const SIX_HOURS_MS: u64 = 6 * 60 * 60 * 1000;

    fn six_hour_timeline(lost_ms: u64) -> CaptureTimeline {
        CaptureTimeline {
            wall_clock_ms: SIX_HOURS_MS,
            media_ms: SIX_HOURS_MS - lost_ms,
            tolerance_ms: 1_000,
        }
    }

    #[test]
    fn gap_reason_tokens_are_distinct_and_stable() {
        let all = [
            GapReason::RecorderExit,
            GapReason::Stall,
            GapReason::Restart,
            GapReason::DiskPressure,
        ];
        let mut tokens: Vec<&str> = all.iter().map(|r| r.as_str()).collect();
        tokens.sort_unstable();
        let before = tokens.len();
        tokens.dedup();
        assert_eq!(
            before,
            tokens.len(),
            "gap reason tokens collided: {tokens:?}"
        );
        assert_eq!(GapReason::Stall.to_string(), "stall");
    }

    #[test]
    fn a_capture_with_no_interruption_audits_clean() {
        // The control. Nothing lost, nothing claimed: if THIS were the
        // only case, every other assertion below would be vacuous.
        let ledger = GapLedger::new();
        let timeline = six_hour_timeline(0);
        assert_eq!(ledger.entries().len(), 0);
        let audit = ledger.audit(&timeline);
        assert!(audit.is_clean(), "clean capture reported {audit:?}");
    }

    #[test]
    fn an_evenly_explained_loss_audits_clean() {
        // 6 h wall clock, 4 minutes of media missing, four recorded
        // interruptions of 30 s + 90 s + 30 s + 90 s = 240 s.
        let mut ledger = GapLedger::new();
        assert!(ledger.note_gap(600_000, 630_000, GapReason::RecorderExit));
        assert!(ledger.note_gap(3_600_000, 3_690_000, GapReason::Stall));
        assert!(ledger.note_gap(9_000_000, 9_030_000, GapReason::Restart));
        assert!(ledger.note_gap(15_000_000, 15_090_000, GapReason::DiskPressure));
        assert_eq!(ledger.reported_ms(), 240_000);

        let audit = ledger.audit(&six_hour_timeline(240_000));
        assert!(
            audit.is_clean(),
            "an explained loss reported {audit:?} — the audit is not differencing the two accounts"
        );
    }

    #[test]
    fn a_hole_the_ledger_never_mentions_is_unreported() {
        // 90 seconds of media missing; the ledger claims one 30-second
        // disconnect. The other 60 seconds is the finding.
        let mut ledger = GapLedger::new();
        assert!(ledger.note_gap(1_000_000, 1_030_000, GapReason::RecorderExit));

        let audit = ledger.audit(&six_hour_timeline(90_000));
        assert_eq!(audit.unreported.len(), 1, "expected one unreported hole");
        let hole = audit.unreported[0];
        assert_eq!(
            hole.reason, None,
            "an unreported hole has no reason by definition"
        );
        // 6 h - 90 s lost, minus the 30 s the ledger claims.
        assert_eq!(
            hole.len_ms(),
            60_000,
            "residual length must be derived, not guessed"
        );
        assert!(audit.spurious.is_empty());
    }

    #[test]
    fn a_ledger_that_claims_more_than_the_timeline_lost_is_spurious() {
        // Media is complete, but the ledger claims a 45-second drop. The
        // other direction has to fail too, or a ledger reporting every
        // second of a six-hour capture as lost would pass.
        let mut ledger = GapLedger::new();
        assert!(ledger.note_gap(2_000_000, 2_045_000, GapReason::Stall));

        let audit = ledger.audit(&six_hour_timeline(0));
        assert!(audit.unreported.is_empty());
        assert_eq!(
            audit.spurious.len(),
            1,
            "expected the over-claim to be flagged"
        );
        assert_eq!(
            audit.spurious[0].len_ms(),
            44_000,
            "only the excess is spurious"
        );
    }

    #[test]
    fn tolerance_absorbs_measurement_slack_but_not_a_real_hole() {
        let timeline = CaptureTimeline {
            wall_clock_ms: SIX_HOURS_MS,
            media_ms: SIX_HOURS_MS - 5_000,
            tolerance_ms: 1_000,
        };
        // 5 s lost, 1 s tolerance, nothing reported: still a hole.
        let audit = GapLedger::new().audit(&timeline);
        assert_eq!(audit.unreported.len(), 1);
        assert_eq!(audit.unreported[0].len_ms(), 5_000);

        // 900 ms lost is inside the stated slack.
        let within = CaptureTimeline {
            wall_clock_ms: SIX_HOURS_MS,
            media_ms: SIX_HOURS_MS - 900,
            tolerance_ms: 1_000,
        };
        assert!(GapLedger::new().audit(&within).is_clean());
    }

    #[test]
    fn a_zero_length_or_inverted_gap_is_refused_rather_than_stored() {
        let mut ledger = GapLedger::new();
        assert!(
            !ledger.note_gap(5_000, 5_000, GapReason::Stall),
            "empty interval"
        );
        assert!(
            !ledger.note_gap(9_000, 4_000, GapReason::Stall),
            "inverted interval"
        );
        assert_eq!(ledger.entries().len(), 0);
        assert_eq!(ledger.reported_ms(), 0);
    }

    #[test]
    fn lost_ms_saturates_instead_of_wrapping() {
        // A capture whose media outlasts its wall clock (a clock skew, or
        // a re-timestamped container) must not underflow into a gigantic
        // "loss" that reads as a real hole.
        let timeline = CaptureTimeline {
            wall_clock_ms: 1_000,
            media_ms: 5_000,
            tolerance_ms: 0,
        };
        assert_eq!(GapLedger::new().lost_ms(&timeline), 0);
    }

    #[test]
    fn every_recorded_reason_survives_into_the_audit() {
        // The refusal has to name *which* interruption it saw, so a user
        // can tell a network drop from a stall. Asserted per reason, not
        // as a set, so dropping one reason is caught.
        for (reason, expected) in [
            (GapReason::RecorderExit, "recorder-exit"),
            (GapReason::Stall, "stall"),
            (GapReason::Restart, "restart"),
            (GapReason::DiskPressure, "disk-pressure"),
        ] {
            let mut ledger = GapLedger::new();
            assert!(ledger.note_gap(0, 10_000, reason));
            let audit = ledger.audit(&six_hour_timeline(10_000));
            assert!(audit.is_clean(), "{expected} was not accepted: {audit:?}");
            assert_eq!(ledger.entries()[0].reason.unwrap().as_str(), expected);
        }
    }
}
