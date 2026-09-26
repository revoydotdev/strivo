//! Capture-first admission: rendering never eats capture's byte reserve.
//!
//! The `ResourceLock::Disk` semaphore in [`crate::pipeline`] bounds how many
//! render stages write *at once*. It bounds nothing about how many bytes are
//! left, so a render can fill a volume and the next capture write fails on a
//! full disk — the outcome C06 calls "rendering silently exhausting capture
//! capacity".
//!
//! The bound here is a **byte partition with a fixed priority order**, not a
//! semaphore: capture's reserve is subtracted from the free space a render is
//! allowed to touch, so the render is refused *before* it can consume the
//! space capture still needs. Nothing about the PVR concurrency gate in
//! `ROADMAP.md` changes — this is a second, independent bound on a different
//! resource (bytes rather than recorder slots).
//!
//! The reserve size is `monitor_limits.disk_budget_reserved_gb`, the same
//! value the monitor's own auto-record gate uses, so there is one number and
//! not two that can drift. Zero disables the gate, matching that gate.

use std::fmt;
use std::path::PathBuf;

/// Bytes held back from rendering so capture always has somewhere to write.
///
/// `dir` is the volume to measure. It is the recording directory, because
/// that is the volume capture actually consumes and the one the monitor's
/// disk-budget gate already measures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureReserve {
    pub dir: PathBuf,
    pub bytes: u64,
}

impl CaptureReserve {
    /// A reserve that holds nothing back, so the render gate never fires.
    pub fn disabled(dir: PathBuf) -> Self {
        Self { dir, bytes: 0 }
    }
}

/// One measured capacity decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureCapacity {
    free_bytes: u64,
    reserve_bytes: u64,
}

impl CaptureCapacity {
    /// Build a verdict from a measurement. Exposed so the decision table
    /// can be tested against a stated free/reserve pair rather than against
    /// whatever the test host's disk happened to report.
    pub fn new(free_bytes: u64, reserve_bytes: u64) -> Self {
        Self {
            free_bytes,
            reserve_bytes,
        }
    }

    /// Measure free space on the reserve's volume.
    ///
    /// `None` when the lookup fails, which the caller must treat as "no
    /// information" rather than "no space": the monitor's disk-budget gate
    /// falls through to allow on a stat failure for the same reason, and a
    /// statvfs hiccup must not stop work.
    pub fn measure(reserve: &CaptureReserve) -> Option<Self> {
        let free = crate::monitor::free_space_bytes(&reserve.dir)?;
        Some(Self::new(free, reserve.bytes))
    }

    pub fn free_bytes(&self) -> u64 {
        self.free_bytes
    }

    pub fn reserve_bytes(&self) -> u64 {
        self.reserve_bytes
    }

    /// Bytes a render may consume without touching capture's reserve.
    /// Saturating: free space below the reserve means no headroom at all,
    /// not a wrapped enormous one.
    pub fn render_headroom(&self) -> u64 {
        self.free_bytes.saturating_sub(self.reserve_bytes)
    }

    /// Admit a capture of `need_bytes`.
    ///
    /// Capture may use the reserve — the reserve exists for capture. It is
    /// still refused when the volume cannot hold the write at all, because a
    /// capture that cannot be written is a capture that must be reported
    /// lost rather than silently skipped.
    pub fn admit_capture(&self, need_bytes: u64) -> Result<(), RenderRefused> {
        if need_bytes <= self.free_bytes {
            Ok(())
        } else {
            Err(RenderRefused::VolumeFull {
                need_bytes,
                free_bytes: self.free_bytes,
            })
        }
    }

    /// Whether a render may start at all.
    ///
    /// Separate from [`Self::admit_render`] because a render's output size
    /// is not knowable before it runs, so the only condition the daemon can
    /// actually enforce is "free space is still above capture's reserve".
    /// Strictly greater: at `free == reserve` the headroom is zero bytes and
    /// the first byte a render writes comes out of capture's share.
    pub fn render_may_start(&self) -> Result<(), RenderRefused> {
        if self.free_bytes > self.reserve_bytes {
            Ok(())
        } else {
            Err(RenderRefused::BelowCaptureReserve {
                free_bytes: self.free_bytes,
                reserve_bytes: self.reserve_bytes,
            })
        }
    }

    /// Admit a render of `need_bytes`, refusing before the work starts.
    pub fn admit_render(&self, need_bytes: u64) -> Result<(), RenderRefused> {
        self.render_may_start()?;
        let headroom = self.render_headroom();
        if need_bytes <= headroom {
            Ok(())
        } else {
            Err(RenderRefused::InsufficientHeadroom {
                need_bytes,
                headroom_bytes: headroom,
            })
        }
    }
}

/// Why a render was not admitted. Every variant carries the numbers that
/// produced it, so the refusal a user reads names the shortfall rather than
/// saying "busy".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderRefused {
    /// Free space is already inside the reserve capture holds. Rendering
    /// stops here so capture keeps the remaining bytes.
    BelowCaptureReserve { free_bytes: u64, reserve_bytes: u64 },
    /// There is headroom, but not enough for this render.
    InsufficientHeadroom {
        need_bytes: u64,
        headroom_bytes: u64,
    },
    /// The volume cannot hold the write at all. Shared with capture, which
    /// is refused for the same reason — at this point capture's reserve is
    /// not what is stopping anything.
    VolumeFull { need_bytes: u64, free_bytes: u64 },
}

impl RenderRefused {
    /// Stable machine token for logs and evidence.
    pub fn code(&self) -> &'static str {
        match self {
            Self::BelowCaptureReserve { .. } => "below-capture-reserve",
            Self::InsufficientHeadroom { .. } => "insufficient-render-headroom",
            Self::VolumeFull { .. } => "volume-full",
        }
    }

    /// Human-readable reason, with the measured numbers in it.
    pub fn reason(&self) -> String {
        match self {
            Self::BelowCaptureReserve {
                free_bytes,
                reserve_bytes,
            } => format!(
                "render refused: {} bytes free is inside the {}-byte reserve held for capture",
                free_bytes, reserve_bytes
            ),
            Self::InsufficientHeadroom {
                need_bytes,
                headroom_bytes,
            } => format!(
                "render refused: needs {need_bytes} bytes but only {headroom_bytes} are available \
                 outside capture's reserve"
            ),
            Self::VolumeFull {
                need_bytes,
                free_bytes,
            } => format!("write refused: needs {need_bytes} bytes but only {free_bytes} are free"),
        }
    }
}

impl fmt::Display for RenderRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    #[test]
    fn a_zero_reserve_disables_the_render_gate() {
        // Same convention as the monitor's disk_budget_reserved_gb == 0.
        let cap = CaptureCapacity::new(10 * MIB, 0);
        assert_eq!(cap.render_headroom(), 10 * MIB);
        assert!(cap.admit_render(10 * MIB).is_ok());
    }

    #[test]
    fn the_verdict_table_is_decided_by_free_reserve_and_need() {
        // The table is the guard against an admit_render that always says
        // yes. Each row's expectation is derived here, independently of the
        // implementation: a render starts only while free space is strictly
        // above the reserve, and then only if it fits the headroom.
        let rows: &[(u64, u64, u64, bool)] = &[
            (100, 0, 100, true),
            (100, 0, 101, false),
            (100, 40, 60, true),
            (100, 40, 61, false),
            (100, 100, 0, false),
            (100, 99, 0, true),
            (40, 100, 1, false),
            (0, 0, 0, false),
            (0, 0, 1, false),
            (1, 2, 1, false),
        ];
        assert_eq!(rows.len(), 10, "the table lost rows");
        for (free, reserve, need, expected_ok) in rows {
            let cap = CaptureCapacity::new(*free, *reserve);
            let expected = *free > *reserve && *need <= free.saturating_sub(*reserve);
            assert_eq!(
                expected, *expected_ok,
                "row (free={free}, reserve={reserve}, need={need}) disagrees with free > reserve && need <= free - reserve"
            );
            assert_eq!(
                cap.admit_render(*need).is_ok(),
                expected,
                "admit_render(free={free}, reserve={reserve}, need={need}) disagreed with the table"
            );
            assert_eq!(
                cap.render_may_start().is_ok(),
                *free > *reserve,
                "render_may_start(free={free}, reserve={reserve}) disagreed with free > reserve"
            );
        }
    }

    #[test]
    fn a_render_stops_at_the_reserve_boundary_not_below_it() {
        // One byte of headroom admits; zero does not. The daemon enforces
        // only this condition, so its exact placement is the whole bound.
        assert!(CaptureCapacity::new(101, 100).render_may_start().is_ok());
        let at = CaptureCapacity::new(100, 100)
            .render_may_start()
            .expect_err("zero headroom must refuse");
        assert_eq!(at.code(), "below-capture-reserve");
        assert!(at.reason().contains("100"), "{:?}", at.reason());
    }

    #[test]
    fn capture_is_admitted_where_render_is_refused() {
        // The ordering C06 asks for, in one assertion pair: at the same
        // free space, the same write is fine for capture and refused for
        // render. If both were decided by the same rule this fails.
        //
        // The write must fit the *volume* (5 MiB of 10 MiB free) while
        // exceeding the *headroom* (2 MiB), or capture would be refused too
        // and the pair would prove nothing about priority. Free is still
        // above the reserve here, so the refusal is the headroom one; the
        // reserve case has free *below* the reserve and is covered below.
        let cap = CaptureCapacity::new(10 * MIB, 8 * MIB);
        let need = 5 * MIB;
        assert_eq!(cap.render_headroom(), 2 * MIB);
        assert!(
            cap.admit_capture(need).is_ok(),
            "capture must be admitted first"
        );
        let refusal = cap.admit_render(need).expect_err("render must be refused");
        assert_eq!(refusal.code(), "insufficient-render-headroom");
    }

    #[test]
    fn a_shortfall_inside_the_reserve_is_named_differently_from_a_large_render() {
        let small_render = CaptureCapacity::new(50 * MIB, 10 * MIB);
        assert_eq!(
            small_render.admit_render(45 * MIB).unwrap_err().code(),
            "insufficient-render-headroom"
        );

        let into_reserve = CaptureCapacity::new(5 * MIB, 10 * MIB);
        assert_eq!(
            into_reserve.admit_render(45 * MIB).unwrap_err().code(),
            "below-capture-reserve"
        );
    }

    #[test]
    fn every_refusal_names_the_numbers_that_caused_it() {
        let cases: &[(CaptureCapacity, u64, &str)] = &[
            (
                CaptureCapacity::new(5 * MIB, 10 * MIB),
                1,
                "10485760-byte reserve",
            ),
            (
                CaptureCapacity::new(50 * MIB, 10 * MIB),
                45 * MIB,
                "41943040",
            ),
        ];
        for (cap, need, must_contain) in cases {
            let reason = cap.admit_render(*need).unwrap_err().reason();
            assert!(
                reason.contains(must_contain),
                "refusal {reason:?} did not name {must_contain:?}"
            );
            assert!(reason.starts_with("render refused:"), "{reason:?}");
        }
        let full = CaptureCapacity::new(0, 0)
            .admit_capture(1)
            .unwrap_err()
            .reason();
        assert!(full.contains("write refused:"), "{full:?}");
        assert!(full.contains(" 0 "), "{full:?} omitted the free byte count");
    }

    #[test]
    fn headroom_saturates_rather_than_wrapping() {
        // Free below the reserve must read as "no headroom", not as a
        // wrapped near-u64 value that would admit everything.
        let cap = CaptureCapacity::new(1, 1_000_000_000);
        assert_eq!(cap.render_headroom(), 0);
        assert!(cap.admit_render(1).is_err());
    }

    #[test]
    fn measure_reads_the_real_volume_and_never_invents_a_number() {
        let reserve = CaptureReserve {
            dir: std::env::temp_dir(),
            bytes: 0,
        };
        let cap = CaptureCapacity::measure(&reserve).expect("temp dir must be statable");
        assert!(
            cap.free_bytes() > 0,
            "a real volume reported zero free bytes"
        );
        assert_eq!(cap.reserve_bytes(), 0);

        // A path that cannot exist cannot be measured, and that has to be a
        // `None` the caller can distinguish from "the disk is full".
        let missing = CaptureReserve {
            dir: PathBuf::from("/nonexistent-capacity-probe-dir/inner"),
            bytes: 0,
        };
        assert_eq!(CaptureCapacity::measure(&missing), None);
    }
}
