//! Soak driver: capture interruption, segment recovery, disk pressure.
//!
//! Run through `scripts/soak_capture_pressure.sh`, which is what sets up the
//! size-capped volume and the private target directory. Requires `ffmpeg`,
//! `ffprobe` and `mkvmerge` on `PATH` and a writable `STRIVO_SOAK_VOLUME`;
//! it exits non-zero rather than skipping a phase, because a rehearsal that
//! silently did not run is worse than one that failed.
//!
//! Phases:
//!
//! 1. **Capture + recovery.** Six hours of real stream time, split into
//!    sessions, each interrupted per [`SoakPlan`]. Every segment is encoded
//!    by ffmpeg, merged by `mkvmerge` through
//!    `recording::segments::merge_segments`, and measured by `ffprobe`. Each
//!    part is fully decoded, not size-checked.
//! 2. **Concurrent edit and render.** An edit workload runs against the
//!    merged media while renders are admitted through the real
//!    `ResourceRegistry`, i.e. the same `ResourceLock::Disk` acquisition the
//!    daemon performs.
//! 3. **Disk pressure.** `STRIVO_SOAK_VOLUME` is filled until free space
//!    reaches capture's reserve, and the assertion is that capture is still
//!    admitted while the render is refused *with a reason*.
//!
//! # Non-vacuity switch
//!
//! `STRIVO_SOAK_MUTATE` deliberately breaks the ledger so the "zero
//! unreported gaps" claim can be shown to be falsifiable rather than
//! asserted:
//!
//! - `drop-ledger-entry` — one injected outage is never recorded.
//! - `phantom-ledger-entry` — an outage the media does not support is
//!   recorded anyway.
//!
//! Both must turn this run red. A clean run under either is a defect in the
//! rehearsal.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use strivo_core::pipeline::executor::{LockAcquireError, ResourceRegistry};
use strivo_core::pipeline::stage::ResourceLock;
use strivo_core::recording::capacity::{CaptureCapacity, CaptureReserve, RenderRefused};
use strivo_core::recording::gaps::{CaptureTimeline, GapLedger, GapReason};
use strivo_core::recording::segments;
use strivo_core::recording::soak::{
    CaptureSession, SessionReport, SoakPlan, SoakReport, MAX_RESUMES_PER_SESSION, SOAK_CAPTURE_MS,
    SOAK_TOLERANCE_MS,
};

/// Default interruption-schedule seed. `scripts/soak_capture_pressure.sh`
/// overrides it with `STRIVO_SOAK_SEED`.
const DEFAULT_SEED: u64 = 0x5EED;
const DEFAULT_SESSIONS: usize = 12;
const FIXTURE_WIDTH: u32 = 128;
const FIXTURE_HEIGHT: u32 = 32;
const FIXTURE_FPS: u32 = 10;

/// Bytes capture holds back from rendering in the pressure phase. Chosen to
/// sit inside the volume so free space crosses it mid-run.
const PRESSURE_RESERVE_BYTES: u64 = 8 * 1024 * 1024;
/// Bytes the filler writes before the pressure phase starts.
const PRESSURE_FILL_BYTES: u64 = 4 * 1024 * 1024;
/// Per-part slack between the scheduled window and what ffprobe measures.
/// An outage is 5-45 s; this is three orders of magnitude tighter.
const PART_TOLERANCE_MS: u64 = 500;
/// How far past the reserve the ballast run aims, so the refusal boundary is
/// crossed rather than approached.
const PRESSURE_OVERSHOOT_BYTES: u64 = 4 * 1024 * 1024;

struct Failure {
    check: String,
    detail: String,
}

#[derive(Default)]
struct Failures(Vec<Failure>);

impl Failures {
    fn require(&mut self, check: &str, ok: bool, detail: impl Into<String>) {
        if !ok {
            self.0.push(Failure {
                check: check.to_string(),
                detail: detail.into(),
            });
        }
    }
}

/// First line of the tool's version banner.
///
/// ffmpeg/ffprobe take `-version` and mkvmerge takes `--version`, so both
/// spellings are tried rather than branching on the tool name.
fn tool_version(tool: &str) -> Result<String, String> {
    let mut last = String::new();
    for flag in ["-version", "--version"] {
        match Command::new(tool).arg(flag).output() {
            Err(e) => return Err(format!("{tool} is not on PATH: {e}")),
            Ok(out) if out.status.success() => {
                return Ok(String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_string())
            }
            Ok(out) => last = format!("{tool} {flag} exited {}", out.status),
        }
    }
    Err(last)
}

/// Encode `seconds` of synthetic stream into `dest`, matching the fixture
/// corpus's shape (Matroska, H.264, no audio) so `merge_segments` and
/// `ffprobe` see the same container the PVR produces.
fn encode_segment(dest: &Path, seconds: u64) -> Result<(), String> {
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc2=size={FIXTURE_WIDTH}x{FIXTURE_HEIGHT}:rate={FIXTURE_FPS}"),
            "-t",
            &format!("{seconds}.{:.03}", 0),
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "51",
            "-g",
            "30",
        ])
        .arg(dest)
        .status()
        .map_err(|e| format!("spawn ffmpeg: {e}"))?;
    if !status.success() {
        return Err(format!("ffmpeg exited {status} for {}", dest.display()));
    }
    Ok(())
}

/// Container duration in milliseconds, from `ffprobe`.
fn probe_duration_ms(path: &Path) -> Result<u64, String> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()
        .map_err(|e| format!("spawn ffprobe: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "ffprobe failed on {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let secs: f64 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .map_err(|_| {
            format!(
                "ffprobe returned a non-numeric duration for {}",
                path.display()
            )
        })?;
    Ok((secs * 1000.0).round() as u64)
}

/// Fully decode `path`, discarding output. A parseable header is not
/// playability; this decodes every frame and fails on any decode error.
fn decodes_fully(path: &Path) -> Result<(), String> {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args(["-f", "null", "-"])
        .output()
        .map_err(|e| format!("spawn ffmpeg for decode: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "decode of {} failed: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !stderr.trim().is_empty() {
        return Err(format!(
            "decode of {} reported errors: {}",
            path.display(),
            stderr.trim()
        ));
    }
    Ok(())
}

fn free_bytes(dir: &Path) -> Option<u64> {
    let out = Command::new("df")
        .args(["-B1", "--output=avail", &dir.to_string_lossy()])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .last()
        .and_then(|l| l.trim().parse().ok())
}

/// Build a report from a caller-supplied ledger per session.
///
/// `SoakPlan::report` builds its ledgers internally, which is right for the
/// library and wrong for a mutation probe: a mutation has to reach the
/// ledger the acceptance assertion is graded on, not sit beside it. This
/// assembles the same public `GapLedger`/`CaptureTimeline`/`SessionReport`
/// the library does, so the graded path and the honest path differ only in
/// which entries the ledger holds.
fn build_report(
    plan: &SoakPlan,
    measured: &[u64],
    ledger_for: impl Fn(&CaptureSession) -> GapLedger,
) -> SoakReport {
    let sessions = plan
        .sessions
        .iter()
        .zip(measured)
        .map(|(session, &media_ms)| {
            let timeline = CaptureTimeline {
                wall_clock_ms: session.wall_clock_ms(),
                media_ms,
                tolerance_ms: SOAK_TOLERANCE_MS,
            };
            let ledger = ledger_for(session);
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

struct PartOutcome {
    path: PathBuf,
    /// The window this part was scheduled to hold, checked against ffprobe.
    nominal_ms: u64,
    decoded: bool,
}

fn main() {
    let started = Instant::now();
    let mut failures = Failures::default();

    let volume = std::env::var_os("STRIVO_SOAK_VOLUME").map(PathBuf::from);
    let mutation = std::env::var("STRIVO_SOAK_MUTATE").unwrap_or_else(|_| "none".into());
    let workdir = std::env::var_os("STRIVO_SOAK_WORKDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);

    // A missing phase is a failure, not a skip.
    let Some(volume) = volume else {
        eprintln!(
            "STRIVO_SOAK_VOLUME is not set. scripts/soak_capture_pressure.sh sets it to a \
             size-capped tmpfs; run the rehearsal through that script."
        );
        std::process::exit(2);
    };

    for tool in ["ffmpeg", "ffprobe", "mkvmerge"] {
        if let Err(e) = tool_version(tool) {
            eprintln!("{e}");
            std::process::exit(2);
        }
    }
    if !volume.is_dir() {
        eprintln!("STRIVO_SOAK_VOLUME {} is not a directory", volume.display());
        std::process::exit(2);
    }

    let seed: u64 = std::env::var("STRIVO_SOAK_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_SEED);
    let sessions: usize = std::env::var("STRIVO_SOAK_SESSIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_SESSIONS);

    let plan = SoakPlan::build(seed, sessions, SOAK_CAPTURE_MS);
    let scratch = workdir.join(format!("soak-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("create scratch");

    println!(
        "soak: seed={seed:#x} sessions={sessions} window={}ms interruptions={} segments={} mutation={mutation}",
        SOAK_CAPTURE_MS,
        plan.interruption_count(),
        plan.segment_count()
    );

    // ---- Phase 1 + 2: capture, recovery, concurrent edit and render ----
    let mut measured_merged_ms = Vec::with_capacity(sessions);
    let mut parts_total = 0usize;
    let mut parts_decoded = 0usize;
    let mut media_bytes = 0u64;
    let mut render_admitted = 0usize;
    let mut render_refused = 0usize;
    let dropped_entry: Option<(usize, u64)> = match mutation.as_str() {
        "drop-ledger-entry" => plan
            .sessions
            .iter()
            .enumerate()
            .flat_map(|(i, s)| s.interruptions.iter().map(move |x| (i, x.at_ms)))
            .next(),
        _ => None,
    };

    // A real filesystem with no space headroom is what makes the render
    // refusal reachable; on the scratch volume the reserve is 0 so renders
    // are admitted, which is the point of comparing the two.
    let scratch_reserve = CaptureReserve::disabled(scratch.clone());

    for (index, session) in plan.sessions.iter().enumerate() {
        let mut parts: Vec<PartOutcome> = Vec::new();
        for (part, (start_ms, end_ms)) in session.segment_windows().iter().enumerate() {
            let name = match part {
                0 => format!("{}.mkv", session.name),
                _ => format!("{}_part{}.mkv", session.name, part + 1),
            };
            let path = scratch.join(&name);
            let seconds = ((end_ms - start_ms) as f64 / 1000.0).round().max(2.0) as u64;
            if let Err(e) = encode_segment(&path, seconds) {
                failures.require("encode", false, e);
                continue;
            }
            parts.push(PartOutcome {
                path,
                nominal_ms: end_ms - start_ms,
                decoded: false,
            });
        }
        if parts.len() != session.segment_count() {
            failures.require(
                "segment_count",
                false,
                format!(
                    "{}: encoded {} of {} segments",
                    session.name,
                    parts.len(),
                    session.segment_count()
                ),
            );
            continue;
        }

        // Every completed part must be playable, proven by decoding it, and
        // must hold the media its window promised, proven by ffprobe.
        for part in parts.iter_mut() {
            parts_total += 1;
            match decodes_fully(&part.path) {
                Ok(()) => {
                    part.decoded = true;
                    parts_decoded += 1;
                }
                Err(e) => failures.require("part_playable", false, e),
            }
            match probe_duration_ms(&part.path) {
                Ok(measured) => {
                    let drift = measured.abs_diff(part.nominal_ms);
                    if drift > PART_TOLERANCE_MS {
                        failures.require(
                            "part_duration",
                            false,
                            format!(
                                "{}: nominal {} ms, measured {measured} ms, drift {drift} ms",
                                part.path.display(),
                                part.nominal_ms
                            ),
                        );
                    }
                }
                Err(e) => failures.require("probe_part", false, e),
            }
            media_bytes += std::fs::metadata(&part.path).map(|m| m.len()).unwrap_or(0);
        }

        // Concurrent edit: derive a sidecar from the merged media while a
        // render is admitted. The editor crate belongs to another lane, so
        // the edit is modelled as a real derived-artifact write rather than
        // driven through it; see the disclosure in the evidence file.
        let sources: Vec<PathBuf> = parts.iter().map(|p| p.path.clone()).collect();
        let merged = scratch.join(format!("{}-merged.mkv", session.name));
        if let Err(e) = segments::merge_segments(&sources, &merged) {
            failures.require("merge", false, format!("{}: {e}", session.name));
            continue;
        }
        match probe_duration_ms(&merged) {
            Ok(duration_ms) => measured_merged_ms.push(duration_ms),
            Err(e) => {
                failures.require("probe_merged", false, e);
                measured_merged_ms.push(session.expected_media_ms());
            }
        }

        let edit_path = scratch.join(format!("{}-edit.json", session.name));
        std::fs::write(
            &edit_path,
            format!(
                "{{\"session\":\"{}\",\"segments\":{},\"bytes\":{}}}",
                session.name,
                parts.len(),
                media_bytes
            ),
        )
        .expect("write edit sidecar");

        // Render admission through the real pipeline registry.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let admitted = runtime.block_on(async {
            let registry = ResourceRegistry::new().with_capture_reserve(scratch_reserve.clone());
            registry
                .acquire(&ResourceLock::Disk { cap: 2 })
                .await
                .is_ok()
        });
        if admitted {
            render_admitted += 1;
        } else {
            render_refused += 1;
        }

        if index == 0 {
            println!(
                "  {}: {} segments, edit sidecar {} bytes",
                session.name,
                parts.len(),
                std::fs::metadata(&edit_path).map(|m| m.len()).unwrap_or(0)
            );
        }
    }

    // ---- The audit ----
    //
    // Honest runs go through the library's own `SoakPlan::report`. A
    // mutation run grades a ledger the mutation actually altered, so the
    // acceptance assertion below is the thing that turns red.
    let dropped = dropped_entry;
    let phantom = mutation == "phantom-ledger-entry";
    let report = if dropped.is_none() && !phantom {
        plan.report(&measured_merged_ms)
    } else {
        build_report(&plan, &measured_merged_ms, |session| {
            let mut ledger = session.ledger();
            if phantom && session.name == "session-00" {
                // An outage the measured media does not support.
                let _ = ledger.note_gap(
                    session.started_ms + 60_000,
                    session.started_ms + 90_000,
                    GapReason::Stall,
                );
            }
            if let Some((index, at_ms)) = dropped {
                if plan.sessions[index].name == session.name {
                    let mut kept = GapLedger::new();
                    for entry in ledger.entries() {
                        if entry.start_ms == session.started_ms + at_ms {
                            continue;
                        }
                        let _ = kept.note_gap(
                            entry.start_ms,
                            entry.end_ms,
                            entry.reason.expect("a recorded entry carries a reason"),
                        );
                    }
                    return kept;
                }
            }
            ledger
        })
    };
    failures.require(
        "session_coverage",
        report.session_count() == sessions,
        format!("{} of {sessions} sessions reported", report.session_count()),
    );
    failures.require(
        "zero_unreported_gaps",
        report.unreported_ms() == 0,
        format!(
            "{} ms unrecovered across {} sessions",
            report.unreported_ms(),
            report.session_count()
        ),
    );
    failures.require(
        "zero_spurious_gaps",
        report.spurious_ms() == 0,
        format!(
            "{} ms of claimed outages the media does not support",
            report.spurious_ms()
        ),
    );
    failures.require(
        "every_part_decoded",
        parts_total > 0 && parts_decoded == parts_total,
        format!("{parts_decoded} of {parts_total} parts decoded"),
    );
    failures.require(
        "parts_match_plan",
        parts_total == plan.segment_count(),
        format!("{parts_total} parts for a plan of {}", plan.segment_count()),
    );

    // The mutation switch has to bite. It is graded on the same assertion as
    // the honest run, so a mutation that did not corrupt anything would show
    // up as a clean run and this check is what prevents that reading.
    if dropped.is_some() || phantom {
        let detail = format!(
            "unreported={} ms spurious={} ms",
            report.unreported_ms(),
            report.spurious_ms()
        );
        println!("  MUTATION {mutation} graded on the mutated ledger -> {detail}");
        let went_red = if phantom {
            report.spurious_ms() > 0
        } else {
            report.unreported_ms() > 0
        };
        failures.require(
            "mutation_detected",
            went_red,
            format!(
                "the {mutation} mutation left the graded report clean ({detail}), so the \
                 acceptance assertion cannot fail and proves nothing"
            ),
        );
    }

    println!(
        "  audit: {} ms media of {} ms wall clock, {} ms reported, {} ms unreported, {} ms spurious",
        report.media_ms(),
        report.wall_clock_ms(),
        report.reported_ms(),
        report.unreported_ms(),
        report.spurious_ms()
    );
    println!(
        "  parts: {parts_decoded}/{parts_total} decoded, {media_bytes} bytes, \
         renders admitted {render_admitted} refused {render_refused}"
    );

    // ---- Phase 3: disk pressure on the size-capped volume ----
    let pressure = disk_pressure_phase(&volume, &mut failures);

    let _ = std::fs::remove_dir_all(&scratch);

    println!("\nsoak summary");
    println!("  mutation            {mutation}");
    println!("  capture_window_ms   {SOAK_CAPTURE_MS}");
    println!("  sessions            {sessions}");
    println!("  interruptions       {}", plan.interruption_count());
    println!("  max_resumes         {MAX_RESUMES_PER_SESSION}");
    println!("  segments_encoded    {parts_total}");
    println!("  segments_decoded    {parts_decoded}");
    println!("  media_bytes         {media_bytes}");
    println!("  unreported_ms       {}", report.unreported_ms());
    println!("  spurious_ms         {}", report.spurious_ms());
    println!("  pressure_free_bytes {}", pressure.free_before);
    println!("  pressure_capture    {}", pressure.capture_verdict);
    println!("  pressure_render     {}", pressure.render_verdict);
    println!(
        "  pressure_reason     {}",
        pressure.reason.as_deref().unwrap_or("(none)")
    );
    println!(
        "  elapsed_secs        {:.1}",
        started.elapsed().as_secs_f64()
    );

    if failures.0.is_empty() {
        println!("\nRESULT ok");
        std::process::exit(0);
    }
    println!("\nRESULT fail");
    for failure in &failures.0 {
        println!("  [{}] {}", failure.check, failure.detail);
    }
    std::process::exit(1);
}

struct PressureOutcome {
    free_before: u64,
    capture_verdict: String,
    render_verdict: String,
    reason: Option<String>,
}

/// Fill `volume` until free space reaches `PRESSURE_RESERVE_BYTES`, then
/// assert the ordering C06 requires: capture is still admitted, the render
/// is refused, and the refusal names the numbers.
fn disk_pressure_phase(volume: &Path, failures: &mut Failures) -> PressureOutcome {
    let mut out = PressureOutcome {
        free_before: 0,
        capture_verdict: "not-run".into(),
        render_verdict: "not-run".into(),
        reason: None,
    };

    let free_before = match free_bytes(volume) {
        Some(free) => free,
        None => {
            failures.require("pressure_measure", false, "df could not read the volume");
            return out;
        }
    };
    out.free_before = free_before;
    println!(
        "\npressure phase: volume {} has {free_before} bytes free, reserve is {PRESSURE_RESERVE_BYTES}",
        volume.display()
    );

    // Fill with real bytes. A sparse file would not consume space, so this
    // writes actual data.
    let filler = volume.join("soak-filler.bin");
    let chunk = vec![0xA5u8; 64 * 1024];
    let target_fill = PRESSURE_FILL_BYTES;
    let mut written = 0u64;
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&filler).expect("create filler");
        while written < target_fill {
            let n = file.write(&chunk).expect("write filler");
            if n == 0 {
                break;
            }
            written += n as u64;
        }
        file.flush().expect("flush filler");
    }

    // Fill past the reserve, not up to it. `render_may_start` admits while
    // free is strictly ABOVE the reserve, so a ballast run that stopped at
    // reserve + slack would leave the render legitimately admitted and prove
    // nothing. Overshoot by PRESSURE_OVERSHOOT_BYTES so the boundary is
    // crossed unambiguously.
    let reserve_dir = volume.join("reserve-probe");
    std::fs::create_dir_all(&reserve_dir).expect("create probe dir");
    while let Some(free) = free_bytes(volume) {
        if free <= PRESSURE_RESERVE_BYTES {
            break;
        }
        let want = free
            .saturating_sub(PRESSURE_RESERVE_BYTES)
            .saturating_add(PRESSURE_OVERSHOOT_BYTES);
        let path = volume.join(format!("ballast-{free}"));
        let mut file = match std::fs::File::create(&path) {
            Ok(f) => f,
            Err(_) => break,
        };
        use std::io::Write;
        let mut left = want;
        let block = vec![0x5Au8; 64 * 1024];
        while left > 0 {
            let take = left.min(block.len() as u64) as usize;
            match file.write_all(&block[..take]) {
                Ok(()) => left -= take as u64,
                // The volume is full: exactly the state being rehearsed.
                Err(_) => break,
            }
        }
        drop(file);
    }

    let free_after = free_bytes(volume).unwrap_or(0);
    println!(
        "  filled {written} bytes of ballast; {free_after} bytes free, \
         {} above the {PRESSURE_RESERVE_BYTES}-byte reserve",
        free_after as i64 - PRESSURE_RESERVE_BYTES as i64
    );

    // The measured capacity is the real free space on the real volume.
    let capacity = CaptureCapacity::measure(&CaptureReserve {
        dir: volume.to_path_buf(),
        bytes: PRESSURE_RESERVE_BYTES,
    });
    let Some(capacity) = capacity else {
        failures.require(
            "pressure_measure",
            false,
            "measure() returned None for the volume",
        );
        return out;
    };

    // Capture first: the write it wants is the whole remaining free space,
    // which is exactly what capture is allowed to use.
    let capture_need = capacity.free_bytes();
    out.capture_verdict = match capacity.admit_capture(capture_need) {
        Ok(()) => "admitted".into(),
        Err(e) => format!("refused: {e}"),
    };
    failures.require(
        "pressure_capture_admitted",
        out.capture_verdict == "admitted",
        out.capture_verdict.clone(),
    );

    // An admission is a decision; survival is a write. Do the write, into
    // the same pressured volume, and keep what it produced.
    let capture_path = volume.join("soak-capture-part.mkv");
    let capture_written = encode_segment(&capture_path, 2);
    let capture_bytes = std::fs::metadata(&capture_path)
        .map(|m| m.len())
        .unwrap_or(0);
    match &capture_written {
        Ok(()) => {
            out.capture_verdict = format!("admitted and wrote {capture_bytes} bytes");
        }
        Err(e) => {
            out.capture_verdict = format!("admitted but the write failed: {e}");
            failures.require("pressure_capture_survives", false, e.clone());
        }
    }
    failures.require(
        "pressure_capture_survives",
        capture_written.is_ok() && capture_bytes > 0,
        format!("capture wrote {capture_bytes} bytes"),
    );
    if capture_written.is_ok() {
        // And what it wrote has to be playable, like any other part.
        if let Err(e) = decodes_fully(&capture_path) {
            failures.require("pressure_capture_playable", false, e);
        }
    }
    let _ = std::fs::remove_file(&capture_path);

    // Then render, through the real pipeline registry and the real disk
    // lock — the same acquisition the daemon performs.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let render = runtime.block_on(async {
        let registry = ResourceRegistry::new().with_capture_reserve(CaptureReserve {
            dir: volume.to_path_buf(),
            bytes: PRESSURE_RESERVE_BYTES,
        });
        registry.acquire(&ResourceLock::Disk { cap: 2 }).await
    });
    out.render_verdict = match &render {
        Ok(_) => "admitted".into(),
        Err(LockAcquireError::CaptureReserve {
            free_bytes,
            reserve_bytes,
        }) => format!(
            "refused: {free_bytes} bytes free is not above the {reserve_bytes}-byte reserve"
        ),
        Err(e) => format!("refused: {e}"),
    };
    if let Err(e) = &render {
        out.reason = Some(e.to_string());
    }

    failures.require(
        "pressure_render_refused",
        matches!(render, Err(LockAcquireError::CaptureReserve { .. })),
        format!(
            "expected LockAcquireError::CaptureReserve, got {}",
            out.render_verdict
        ),
    );
    let refusal = out.reason.clone().unwrap_or_default();
    failures.require(
        "pressure_refusal_names_numbers",
        refusal.contains(&PRESSURE_RESERVE_BYTES.to_string())
            && refusal.contains(&format!("{}", capacity.free_bytes())),
        format!("refusal {refusal:?} did not name the free and reserve byte counts"),
    );

    // The refusal is also what a caller would show, so it must render.
    let renderable = RenderRefused::BelowCaptureReserve {
        free_bytes: capacity.free_bytes(),
        reserve_bytes: PRESSURE_RESERVE_BYTES,
    };
    println!("  capture: {}", out.capture_verdict);
    println!("  render:  {}", out.render_verdict);
    println!("  reason:  {}", renderable.reason());

    // Leave the volume as found.
    let _ = std::fs::remove_file(&filler);
    if let Ok(entries) = std::fs::read_dir(volume) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("ballast-") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let _ = std::fs::remove_dir_all(&reserve_dir);

    out
}
