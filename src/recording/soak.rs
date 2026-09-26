//! Soak model for capture interruption, segment recovery and workload
//! isolation (report item C06).
//!
//! # What is real and what is modelled
//!
//! The *media* in this rehearsal is real. Every segment is encoded by
//! ffmpeg, merged by `mkvmerge` through
//! [`super::segments::merge_segments`], and measured by `ffprobe`; the
//! durations this module audits are those measurements, not its own
//! arithmetic. The six hours is six hours of real stream time.
//!
//! The *process and concurrency* dimension is modelled and time-compressed.
//! Nothing here spawns a recorder, waits on a socket, or runs for six hours
//! of wall clock, so real-time behaviour — a wedged child, a slow volume, a
//! descriptor leak that only shows at hour five — is **not** proven by a
//! green run here. The example driver states its own compression factor.
//!
//! # Why the audit is not circular
//!
//! A ledger built from the same schedule that produced the media would
//! always balance, and would prove nothing. So the two sides of
//! [`GapLedger::audit`] come from different places: the *reported* side is
//! the interruption schedule, and the *media* side is what `ffprobe`
//! measured on the merged file. If an encode comes up short, or `mkvmerge`
//! loses a join, the measured side moves and the audit reports an
//! unrecovered interval. `a_short_merged_file_is_reported_as_unrecovered_time`
//! is that check, and it fails when the measurement is off.

use super::gaps::{CaptureTimeline, GapAudit, GapLedger, GapReason};

/// The acceptance window: six hours of capture.
pub const SOAK_CAPTURE_MS: u64 = 6 * 60 * 60 * 1000;

/// Resumes the recorder will attempt before a session is failed.
///
/// `src/recording/mod.rs` admits a gap-resume retry only while
/// `retry_count < 3`, so a session can produce at most four segments. The
/// plan stays inside that; a soak that modelled a fifth resume would be
/// rehearsing a path the recorder does not take.
pub const MAX_RESUMES_PER_SESSION: u32 = 3;

/// Per-session slack for encode and merge rounding, in milliseconds.
///
/// Six hours of stream time is re-encoded and re-joined, and container
/// timestamps are millisecond-quantised at every boundary. This is the
/// stated ceiling on that disagreement — not a fudge factor, and not large
/// enough to hide an outage, which is orders of magnitude bigger.
pub const SOAK_TOLERANCE_MS: u64 = 5_000;

/// Shortest uninterrupted run a session may keep, so no interruption lands
/// on a boundary and every segment has a decodable body.
const MIN_SEGMENT_MS: u64 = 30_000;
/// Bounds on an injected outage: short enough to stay well inside the
/// session, long enough that no tolerance can absorb it.
const MIN_OUTAGE_MS: u64 = 5_000;
const MAX_OUTAGE_MS: u64 = 45_000;

/// One mid-stream stop and the hole it leaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interruption {
    /// Offset from the session start at which capture stops writing.
    pub at_ms: u64,
    /// How long nothing is captured before the next segment starts.
    pub outage_ms: u64,
    pub reason: GapReason,
}

/// One channel's contiguous capture, with the interruptions injected into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureSession {
    pub name: String,
    /// Offset of this session within the soak window.
    pub started_ms: u64,
    pub length_ms: u64,
    /// Sorted by `at_ms`, non-overlapping, each leaving `MIN_SEGMENT_MS` of
    /// media on both sides.
    pub interruptions: Vec<Interruption>,
}

impl CaptureSession {
    /// Number of segments this session produces: one per uninterrupted run.
    pub fn segment_count(&self) -> usize {
        self.interruptions.len() + 1
    }

    /// Nominal `(start_ms, end_ms)` of each segment, relative to the session
    /// start. Contiguous in media time; the holes between them are the
    /// outages.
    pub fn segment_windows(&self) -> Vec<(u64, u64)> {
        let mut windows = Vec::with_capacity(self.segment_count());
        let mut cursor = 0u64;
        for interruption in &self.interruptions {
            windows.push((cursor, interruption.at_ms));
            cursor = interruption.at_ms + interruption.outage_ms;
        }
        windows.push((cursor, self.length_ms));
        windows
    }

    /// Media the session should hold if every segment encodes to length.
    pub fn expected_media_ms(&self) -> u64 {
        self.length_ms - self.interruptions.iter().map(|i| i.outage_ms).sum::<u64>()
    }

    /// Wall clock from the first captured byte to the last.
    pub fn wall_clock_ms(&self) -> u64 {
        self.length_ms
    }

    /// The ledger the recorder would have written for this session.
    pub fn ledger(&self) -> GapLedger {
        let mut ledger = GapLedger::new();
        for interruption in &self.interruptions {
            let started = self.started_ms + interruption.at_ms;
            let _ = ledger.note_gap(
                started,
                started + interruption.outage_ms,
                interruption.reason,
            );
        }
        ledger
    }
}

/// A deterministic split of the soak window into sessions.
///
/// Deterministic on purpose: a soak that draws a different schedule on every
/// run cannot be compared against the run before it, and cannot be replayed
/// after a failure. The generator is a xorshift64 written out here rather
/// than a dependency — the workspace has no RNG crate and this needs none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoakPlan {
    pub seed: u64,
    pub window_ms: u64,
    pub sessions: Vec<CaptureSession>,
}

impl SoakPlan {
    /// Build `sessions` sessions filling `window_ms`, injecting up to
    /// [`MAX_RESUMES_PER_SESSION`] interruptions into each.
    pub fn build(seed: u64, sessions: usize, window_ms: u64) -> Self {
        assert!(sessions > 0, "a soak needs at least one session");
        let mut rng = Rng(seed);
        let length = window_ms / sessions as u64;
        let reasons = [
            GapReason::RecorderExit,
            GapReason::Stall,
            GapReason::Restart,
            GapReason::DiskPressure,
        ];
        let mut planned = Vec::with_capacity(sessions);
        for index in 0..sessions {
            let mut interruptions = Vec::new();
            let count = rng.below(MAX_RESUMES_PER_SESSION as u64 + 1) as usize;
            for slot in 0..count {
                let span = length - 2 * MIN_SEGMENT_MS;
                let at_ms = MIN_SEGMENT_MS
                    + rng.below(span / (count as u64 + 1))
                    + (slot as u64) * (span / (count as u64 + 1));
                let outage_ms = MIN_OUTAGE_MS + rng.below(MAX_OUTAGE_MS - MIN_OUTAGE_MS);
                interruptions.push(Interruption {
                    at_ms,
                    outage_ms,
                    reason: reasons[(index + slot) % reasons.len()],
                });
            }
            interruptions.sort_by_key(|i| i.at_ms);
            planned.push(CaptureSession {
                name: format!("session-{index:02}"),
                started_ms: index as u64 * length,
                length_ms: length,
                interruptions,
            });
        }
        Self {
            seed,
            window_ms,
            sessions: planned,
        }
    }

    /// Total interruptions the plan injects.
    pub fn interruption_count(&self) -> usize {
        self.sessions.iter().map(|s| s.interruptions.len()).sum()
    }

    /// Total segments the plan expects, and therefore the number of files
    /// every completed part check has to account for.
    pub fn segment_count(&self) -> usize {
        self.sessions
            .iter()
            .map(CaptureSession::segment_count)
            .sum()
    }

    /// Audit the plan against durations measured from real media.
    ///
    /// `measured_merged_ms[i]` is what `ffprobe` reported for session `i`'s
    /// merged file. It is the only input that did not come from the schedule,
    /// which is what keeps the audit from balancing by construction.
    pub fn report(&self, measured_merged_ms: &[u64]) -> SoakReport {
        assert_eq!(
            measured_merged_ms.len(),
            self.sessions.len(),
            "one measurement per session, got {} for {} sessions",
            measured_merged_ms.len(),
            self.sessions.len()
        );
        let sessions = self
            .sessions
            .iter()
            .zip(measured_merged_ms)
            .map(|(session, &media_ms)| {
                let ledger = session.ledger();
                let timeline = CaptureTimeline {
                    wall_clock_ms: session.wall_clock_ms(),
                    media_ms,
                    tolerance_ms: SOAK_TOLERANCE_MS,
                };
                SessionReport {
                    name: session.name.clone(),
                    segments: session.segment_count(),
                    wall_clock_ms: timeline.wall_clock_ms,
                    media_ms,
                    reported_ms: ledger.reported_ms(),
                    audit: ledger.audit(&timeline),
                }
            })
            .collect();
        SoakReport { sessions }
    }
}

/// What one session's rehearsal found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionReport {
    pub name: String,
    pub segments: usize,
    pub wall_clock_ms: u64,
    /// Measured, not scheduled.
    pub media_ms: u64,
    pub reported_ms: u64,
    pub audit: GapAudit,
}

impl SessionReport {
    /// How far the measured media fell short of the wall clock.
    pub fn lost_ms(&self) -> u64 {
        self.wall_clock_ms.saturating_sub(self.media_ms)
    }
}

/// The whole rehearsal's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoakReport {
    pub sessions: Vec<SessionReport>,
}

impl SoakReport {
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Total measured media across every session.
    pub fn media_ms(&self) -> u64 {
        self.sessions.iter().map(|s| s.media_ms).sum()
    }

    /// Total wall clock the sessions span.
    pub fn wall_clock_ms(&self) -> u64 {
        self.sessions.iter().map(|s| s.wall_clock_ms).sum()
    }

    /// Total reported interruptions.
    pub fn reported_ms(&self) -> u64 {
        self.sessions.iter().map(|s| s.reported_ms).sum()
    }

    /// Unrecovered time the audit found, across all sessions.
    pub fn unreported_ms(&self) -> u64 {
        self.sessions
            .iter()
            .flat_map(|s| s.audit.unreported.iter())
            .map(|i| i.len_ms())
            .sum()
    }

    /// Time the ledger claimed that the media does not support.
    pub fn spurious_ms(&self) -> u64 {
        self.sessions
            .iter()
            .flat_map(|s| s.audit.spurious.iter())
            .map(|i| i.len_ms())
            .sum()
    }

    /// True when every session's measured media is fully accounted for.
    pub fn is_clean(&self) -> bool {
        !self.sessions.is_empty() && self.sessions.iter().all(|s| s.audit.is_clean())
    }
}

struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measured_from_plan(plan: &SoakPlan) -> Vec<u64> {
        plan.sessions
            .iter()
            .map(CaptureSession::expected_media_ms)
            .collect()
    }

    #[test]
    fn the_plan_fills_exactly_the_six_hour_window() {
        let plan = SoakPlan::build(0x5EED, 12, SOAK_CAPTURE_MS);
        assert_eq!(plan.sessions.len(), 12);
        let total: u64 = plan.sessions.iter().map(|s| s.length_ms).sum();
        assert_eq!(total, SOAK_CAPTURE_MS, "the sessions do not cover 6 h");
        assert_eq!(plan.window_ms, SOAK_CAPTURE_MS);
    }

    #[test]
    fn the_plan_never_exceeds_the_recorder_three_resume_limit() {
        // Fidelity to the real retry path: `retry_count < 3` in
        // src/recording/mod.rs means a fourth segment is unreachable.
        let plan = SoakPlan::build(0x5EED, 12, SOAK_CAPTURE_MS);
        for session in &plan.sessions {
            assert!(
                session.interruptions.len() <= MAX_RESUMES_PER_SESSION as usize,
                "{} plans {} resumes, the recorder allows {}",
                session.name,
                session.interruptions.len(),
                MAX_RESUMES_PER_SESSION
            );
            assert_eq!(session.segment_count(), session.interruptions.len() + 1);
        }
        assert!(
            plan.segment_count() <= 12 * (MAX_RESUMES_PER_SESSION as usize + 1),
            "segment total {} exceeds the reachable maximum",
            plan.segment_count()
        );
    }

    #[test]
    fn every_segment_keeps_a_decodable_body() {
        let plan = SoakPlan::build(0x5EED, 12, SOAK_CAPTURE_MS);
        for session in &plan.sessions {
            for (start, end) in session.segment_windows() {
                assert!(
                    end - start >= MIN_SEGMENT_MS,
                    "{} has a {}-segment from {} to {}",
                    session.name,
                    end - start,
                    start,
                    end
                );
            }
            // Windows are ordered and non-overlapping.
            let windows = session.segment_windows();
            for pair in windows.windows(2) {
                assert!(pair[0].1 < pair[1].0, "{} windows overlap", session.name);
            }
        }
    }

    #[test]
    fn the_plan_is_reproducible_from_its_seed_and_varies_without_it() {
        let a = SoakPlan::build(0x5EED, 12, SOAK_CAPTURE_MS);
        let b = SoakPlan::build(0x5EED, 12, SOAK_CAPTURE_MS);
        let c = SoakPlan::build(0xC0FFEE, 12, SOAK_CAPTURE_MS);
        assert_eq!(a, b, "the same seed produced two different plans");
        assert_ne!(a, c, "two seeds produced an identical plan");
        assert!(
            a.interruption_count() > 0,
            "the plan injected no interruption at all"
        );
    }

    #[test]
    fn every_reason_appears_in_the_plan() {
        // A rehearsal that never produced a `Restart` has not rehearsed
        // restart recovery, whatever else it covers.
        let plan = SoakPlan::build(0x5EED, 12, SOAK_CAPTURE_MS);
        for reason in [
            GapReason::RecorderExit,
            GapReason::Stall,
            GapReason::Restart,
            GapReason::DiskPressure,
        ] {
            assert!(
                plan.sessions
                    .iter()
                    .flat_map(|s| s.interruptions.iter())
                    .any(|i| i.reason == reason),
                "no session injected {reason}"
            );
        }
    }

    #[test]
    fn a_faithfully_measured_run_audits_clean() {
        // The control. If this failed, the schedule and the arithmetic
        // disagree before any media exists.
        let plan = SoakPlan::build(0x5EED, 12, SOAK_CAPTURE_MS);
        let report = plan.report(&measured_from_plan(&plan));
        assert_eq!(report.session_count(), 12);
        assert!(report.is_clean(), "a faithful run reported {:?}", report);
        assert_eq!(report.unreported_ms(), 0);
        assert_eq!(report.spurious_ms(), 0);
    }

    #[test]
    fn a_short_merged_file_is_reported_as_unrecovered_time() {
        // The non-vacuity check. `mkvmerge` loses a join, or an encode comes
        // up short: the measured media drops below the wall clock by more
        // than the schedule explains, and the audit must say so.
        let plan = SoakPlan::build(0x5EED, 12, SOAK_CAPTURE_MS);
        let mut measured = measured_from_plan(&plan);
        measured[3] -= 30_000; // 30 s short on one session

        let report = plan.report(&measured);
        assert!(!report.is_clean(), "a 30 s shortfall went unreported");
        let offender = &report.sessions[3];
        assert_eq!(offender.audit.unreported.len(), 1);
        assert_eq!(offender.audit.unreported[0].reason, None);
        assert_eq!(report.unreported_ms(), 30_000);
        // Every other session is untouched, so the finding is located.
        assert!(report.sessions[4].audit.is_clean());
    }

    #[test]
    fn a_loss_inside_the_stated_tolerance_is_not_reported() {
        // The tolerance is a stated ceiling, and it has to be narrow enough
        // that the 30 s case above is caught. 5 s is not.
        let plan = SoakPlan::build(0x5EED, 12, SOAK_CAPTURE_MS);
        let mut measured = measured_from_plan(&plan);
        measured[0] -= SOAK_TOLERANCE_MS - 1;
        assert!(plan.report(&measured).is_clean());
    }

    #[test]
    fn an_over_claiming_ledger_is_reported_spurious() {
        // The other direction, on the same plan. An audit that can only
        // report missing time accepts a ledger that claims outages that
        // never happened.
        let plan = SoakPlan::build(0x5EED, 12, SOAK_CAPTURE_MS);
        let measured = measured_from_plan(&plan);
        let report = plan.report(&measured);
        assert!(report.is_clean());

        // Re-run one session with a phantom 20 s outage bolted on that the
        // measured media does not support.
        let mut tampered = plan.clone();
        let session = &mut tampered.sessions[2];
        session.length_ms -= 20_000;
        session.interruptions.push(Interruption {
            at_ms: session.length_ms - 60_000,
            outage_ms: 20_000,
            reason: GapReason::Stall,
        });
        let mut tampered_measured = measured.clone();
        tampered_measured[2] -= 20_000;
        let report = tampered.report(&tampered_measured);
        assert!(!report.is_clean(), "a phantom outage was accepted");
        // 20 000 claimed against a timeline that lost 0, less the same 5 s
        // tolerance the missing-time branch forgives — the audit flags the
        // excess, not the whole claim.
        assert_eq!(
            report.spurious_ms(),
            15_000,
            "{:?}",
            report.sessions[2].audit
        );
        assert_eq!(report.unreported_ms(), 0);
    }

    #[test]
    fn a_measurement_of_the_wrong_arity_is_refused() {
        let plan = SoakPlan::build(0x5EED, 12, SOAK_CAPTURE_MS);
        let mut measured = measured_from_plan(&plan);
        measured.pop();
        let result = std::panic::catch_unwind(move || plan.report(&measured));
        assert!(result.is_err(), "a short measurement list was accepted");
    }
}
