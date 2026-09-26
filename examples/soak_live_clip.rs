//! Soak driver: live clipping during capture, without starving acquisition.
//!
//! Run through `scripts/soak_live_clip.sh`, which sets up the size-capped
//! volume and the private target directory. Requires `ffmpeg`, `ffprobe` and
//! `mkvmerge` on `PATH` and a writable `STRIVO_SOAK_VOLUME`; it exits
//! non-zero rather than skipping a phase, because a rehearsal that silently
//! did not run is worse than one that failed.
//!
//! Phases:
//!
//! 1. **Capture with a clip taken out of the middle of it.** Every part is
//!    encoded by ffmpeg and fed to the real `LiveCapture` tail exactly as
//!    `src/recording/mod.rs` feeds it. Partway through each session — while
//!    the recorder is still going to write more parts — the harness asks
//!    for a clip, and the harness's own claims are checked per session:
//!
//!    * a request that reaches into the part currently being written is
//!      refused with `LiveClipError::PartStillOpen`;
//!    * a request inside the finalized prefix renders, through the real
//!      `render_live_clip` — real `mkvmerge` join, real `ffmpeg -c copy` —
//!      and the clip is fully decoded and its duration probed.
//!
//! 2. **The audit.** Six hours of stream time, every part merged by
//!    `recording::segments::merge_segments` and measured by `ffprobe`, then
//!    differenced against the interruption ledger by the library's own
//!    `SoakPlan::report`.
//!
//! 3. **Render saturation.** `STRIVO_SOAK_VOLUME` is filled until free space
//!    reaches capture's reserve, and the assertion is that capture is still
//!    admitted and still writes a part that decodes, while a live clip is
//!    refused *with a reason*.
//!
//! # The six hours is stream time, not wall clock
//!
//! Every number in the audit is real ffmpeg-encoded media measured by
//! ffprobe. The run takes tens of seconds. Nothing here spawns a recorder,
//! waits on a socket or survives a real disconnect, so real-time endurance —
//! a wedged child, a slow volume, a permit leak at hour five — is **not**
//! proven by a green run here. The compression factor is printed in the
//! summary rather than left to be inferred.
//!
//! # Non-vacuity switch
//!
//! `STRIVO_SOAK_MUTATE=drop-ledger-entry` deliberately breaks the ledger the
//! zero-unreported-gaps assertion is graded on. It must turn this run red: a
//! gap report that cannot fail proves nothing, and todo 39 recorded that
//! lesson the hard way.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use strivo_core::pipeline::executor::{LockAcquireError, ResourceRegistry};
use strivo_core::pipeline::stage::ResourceLock;
use strivo_core::recording::capacity::{CaptureCapacity, CaptureReserve};
use strivo_core::recording::gaps::{CaptureTimeline, GapLedger, GapReason};
use strivo_core::recording::live_clip::{
    render_live_clip, LiveCapture, LiveClipError, LiveClipRequest, RenderedClip, MIN_CLIP_MS,
};
use strivo_core::recording::segments;
use strivo_core::recording::soak::{
    CaptureSession, SessionReport, SoakPlan, SoakReport, MAX_RESUMES_PER_SESSION, SOAK_CAPTURE_MS,
    SOAK_TOLERANCE_MS,
};

const DEFAULT_SEED: u64 = 0x5EED;
const DEFAULT_SESSIONS: usize = 6;
const FIXTURE_WIDTH: u32 = 128;
const FIXTURE_HEIGHT: u32 = 32;
const FIXTURE_FPS: u32 = 10;

/// Bytes capture holds back from rendering in the saturation phase.
const SATURATION_RESERVE_BYTES: u64 = 8 * 1024 * 1024;
/// How far past the reserve the ballast aims, so the boundary is crossed.
const SATURATION_OVERSHOOT_BYTES: u64 = 4 * 1024 * 1024;
/// How much LONGER than the requested range a stream-copied clip may be.
///
/// A stream copy cannot cut inside a group of pictures: ffmpeg has to start
/// at the keyframe at or before the requested instant and cannot discard the
/// frames before it without re-encoding. With the fixture's `-g 30` at
/// 10 fps a GOP is 3 000 ms, and the container timestamp rounding at a part
/// join adds a little more, so a 3 000 ms request comes back around 5 500 ms.
/// That is a **stated ceiling on the quantisation**, not a tolerance for a
/// wrong range: a clip SHORTER than it asked for is a failure, and so is one
/// longer, because both mean the range was misread.
///
/// ponytail: stream copy rather than a re-encode, because re-encoding a
/// clip mid-capture spends capture's byte and CPU budget on media it already
/// has. Add an exact cut (or `accurate_seek` with a re-encode) when a caller
/// needs frame-accurate in-points and can say where the budget comes from.
const CLIP_QUANTISATION_MS: u64 = 4_000;

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
                    .to_string());
            }
            Ok(out) => last = format!("{tool} {flag} exited {}", out.status),
        }
    }
    Err(last)
}

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
            &format!("{seconds}"),
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
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() || !stderr.trim().is_empty() {
        return Err(format!(
            "decode of {} failed: {}",
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
/// Todo 39's helper, reused verbatim rather than re-derived: a mutation has
/// to reach the ledger the acceptance assertion is graded on, not sit beside
/// it, and `SoakPlan::report` builds its ledgers internally.
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

fn main() {
    let started = Instant::now();
    let mut failures = Failures::default();

    let volume = std::env::var_os("STRIVO_SOAK_VOLUME").map(PathBuf::from);
    let mutation = std::env::var("STRIVO_SOAK_MUTATE").unwrap_or_else(|_| "none".into());
    let workdir = std::env::var_os("STRIVO_SOAK_WORKDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);

    let Some(volume) = volume else {
        eprintln!(
            "STRIVO_SOAK_VOLUME is not set. scripts/soak_live_clip.sh sets it to a size-capped \
             tmpfs; run the rehearsal through that script."
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
    let scratch = workdir.join(format!("soak-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("create scratch");

    println!(
        "soak-live-clip: seed={seed:#x} sessions={sessions} window={SOAK_CAPTURE_MS}ms \
         interruptions={} segments={} mutation={mutation}",
        plan.interruption_count(),
        plan.segment_count()
    );
    println!(
        "  time base: the window is REAL stream time encoded by ffmpeg; this run is not six \
         hours of wall clock and does not claim to be"
    );

    let scratch_reserve = CaptureReserve::disabled(scratch.clone());
    let mut measured_merged_ms = Vec::with_capacity(sessions);
    let mut parts_total = 0usize;
    let mut parts_decoded = 0usize;
    let mut parts_written_mid_capture = 0usize;
    let mut clips_rendered = 0usize;
    let mut clips_decoded = 0usize;
    let mut open_part_refusals = 0usize;
    let mut clip_quantisation_ms = 0u64;
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
    let phantom = mutation == "phantom-ledger-entry";

    for session in &plan.sessions {
        let base = scratch.join(format!("{}.mkv", session.name));
        let windows = session.segment_windows();
        // The real tail, fed the way src/recording/mod.rs feeds it: the
        // base path at construction, one part opened per resume, and a part
        // finalized only once its writer has exited.
        let mut tail = LiveCapture::new(base.clone());
        let mut part_paths: Vec<PathBuf> = Vec::new();

        for (part, (start_ms, end_ms)) in windows.iter().enumerate() {
            let index = (part + 1) as u32;
            if part > 0 {
                tail.open_part(index).expect("open the resume part");
            }
            let path = tail
                .part(index)
                .expect("the tail knows the part")
                .path
                .clone();
            let seconds = ((end_ms - start_ms) as f64 / 1000.0).round().max(2.0) as u64;
            if let Err(e) = encode_segment(&path, seconds) {
                failures.require("encode", false, e);
                continue;
            }
            let measured = probe_duration_ms(&path).unwrap_or(0);
            tail.finalize_part(index, measured)
                .expect("publish the finalization");
            part_paths.push(path);
            parts_total += 1;
            if let Err(e) = decodes_fully(part_paths.last().expect("just pushed")) {
                failures.require("part_playable", false, e);
            } else {
                parts_decoded += 1;
            }

            // Mid-capture: the recorder is going to write more parts, and
            // the next one is already open. A request that reaches into it
            // must be refused.
            let still_going = part + 1 < windows.len();
            if still_going {
                parts_written_mid_capture += 1;
                tail.open_part(index + 1).expect("open the next part");
                let open = tail
                    .part(index + 1)
                    .expect("the open part is in the tail")
                    .path
                    .clone();
                let reached = tail.finalized_end_ms().saturating_sub(2_000);
                match tail.clip_inputs(&LiveClipRequest::new(reached, reached + 30_000)) {
                    Ok(inputs) => failures.require(
                        "open_part_is_not_an_input",
                        false,
                        format!(
                            "{}: a request reaching the open part {open:?} was answered with {} \
                             inputs",
                            session.name,
                            inputs.parts.len()
                        ),
                    ),
                    Err(LiveClipError::PartStillOpen { index: refused, .. }) => {
                        // It has to name the part that is *actually* open: a
                        // refusal naming the wrong part would still leave the
                        // open file one read away.
                        if refused == index + 1 {
                            open_part_refusals += 1;
                        } else {
                            failures.require(
                                "open_part_named",
                                false,
                                format!(
                                    "{}: refused naming part {refused} rather than the open part {}",
                                    session.name,
                                    index + 1
                                ),
                            );
                        }
                    }
                    Err(other) => failures.require(
                        "open_part_refusal_named",
                        false,
                        format!("{}: refused with {other}, not PartStillOpen", session.name),
                    ),
                }

                // And a request inside the finalized prefix really renders.
                let available = tail.finalized_end_ms();
                if available >= MIN_CLIP_MS * 3 {
                    let end = (available - 500).min(available);
                    let start = end.saturating_sub(3_000);
                    if end - start >= MIN_CLIP_MS {
                        let inputs = match tail.clip_inputs(&LiveClipRequest::new(start, end)) {
                            Ok(inputs) => inputs,
                            Err(e) => {
                                failures.require(
                                    "finalized_clip_admitted",
                                    false,
                                    format!("{}: a finalized range was refused: {e}", session.name),
                                );
                                continue;
                            }
                        };
                        let dest = scratch.join(format!("{}-liveclip-{part}.mkv", session.name));
                        let capacity = CaptureCapacity::measure(&scratch_reserve)
                            .unwrap_or_else(|| CaptureCapacity::new(u64::MAX / 2, 0));
                        match render_live_clip(&tail, &inputs, &dest, &capacity) {
                            Ok(clip) => {
                                clips_rendered += 1;
                                match decodes_fully(&clip.path) {
                                    Ok(()) => clips_decoded += 1,
                                    Err(e) => failures.require("clip_playable", false, e),
                                }
                                match probe_duration_ms(&clip.path) {
                                    Ok(clip_ms) => {
                                        let wanted = clip.end_ms - clip.start_ms;
                                        // Two-sided: a clip that came back
                                        // short is a range misread, and one
                                        // that came back long is the stated
                                        // keyframe quantisation or worse.
                                        if clip_ms < wanted
                                            || clip_ms - wanted > CLIP_QUANTISATION_MS
                                        {
                                            failures.require(
                                                "clip_range",
                                                false,
                                                format!(
                                                    "{}: asked for {wanted} ms, the clip holds \
                                                     {clip_ms} ms (keyframe quantisation \
                                                     ceiling {CLIP_QUANTISATION_MS} ms)",
                                                    session.name
                                                ),
                                            );
                                        }
                                        clip_quantisation_ms = clip_quantisation_ms
                                            .max(clip_ms.saturating_sub(wanted));
                                    }
                                    Err(e) => failures.require("probe_clip", false, e),
                                }
                            }
                            Err(e) => failures.require(
                                "clip_rendered",
                                false,
                                format!("{}: a live clip was refused with {e}", session.name),
                            ),
                        }
                    }
                }
            } else {
                // The capture is over for this session: the tail must say so,
                // or a post-capture render would keep taking the live path.
                tail.stop();
                failures.require(
                    "stopped_is_not_live",
                    tail.clip_inputs(&LiveClipRequest::new(0, MIN_CLIP_MS))
                        == Err(LiveClipError::CaptureStopped),
                    format!(
                        "{}: a stopped capture still answered a live clip",
                        session.name
                    ),
                );
            }

            // The render admission the daemon performs, for the same disk.
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            let admitted = runtime.block_on(async {
                let registry =
                    ResourceRegistry::new().with_capture_reserve(scratch_reserve.clone());
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
        }

        if part_paths.len() != session.segment_count() {
            failures.require(
                "segment_count",
                false,
                format!(
                    "{}: encoded {} of {} segments",
                    session.name,
                    part_paths.len(),
                    session.segment_count()
                ),
            );
            continue;
        }
        let merged = scratch.join(format!("{}-merged.mkv", session.name));
        if let Err(e) = segments::merge_segments(&part_paths, &merged) {
            failures.require("merge", false, format!("{}: {e}", session.name));
            continue;
        }
        match probe_duration_ms(&merged) {
            Ok(ms) => measured_merged_ms.push(ms),
            Err(e) => {
                failures.require("probe_merged", false, e);
                measured_merged_ms.push(session.expected_media_ms());
            }
        }
    }

    // ---- The audit ----
    let dropped = dropped_entry;
    let report = if dropped.is_none() && !phantom {
        plan.report(&measured_merged_ms)
    } else {
        build_report(&plan, &measured_merged_ms, |session| {
            let mut ledger = session.ledger();
            if phantom && session.name == "session-00" {
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

    // Every count is asserted, because a run that produced fewer clips
    // would read as a clean subset.
    failures.require(
        "session_coverage",
        report.session_count() == sessions,
        format!("{} of {sessions} sessions reported", report.session_count()),
    );
    failures.require(
        "parts_match_plan",
        parts_total == plan.segment_count(),
        format!("{parts_total} parts for a plan of {}", plan.segment_count()),
    );
    failures.require(
        "every_part_decoded",
        parts_total > 0 && parts_decoded == parts_total,
        format!("{parts_decoded} of {parts_total} parts decoded"),
    );
    // One clip per mid-capture point, not one per session: a session with
    // three resumes gives the recorder three chances to still be running.
    failures.require(
        "clips_were_taken_mid_capture",
        clips_rendered == parts_written_mid_capture && clips_rendered >= sessions,
        format!(
            "{clips_rendered} live clips rendered for {parts_written_mid_capture} mid-capture \
             points across {sessions} sessions"
        ),
    );
    failures.require(
        "clips_decoded",
        clips_rendered > 0 && clips_decoded == clips_rendered,
        format!("{clips_decoded} of {clips_rendered} clips decoded"),
    );
    failures.require(
        "open_part_refused_every_time",
        open_part_refusals == parts_written_mid_capture,
        format!(
            "{open_part_refusals} PartStillOpen refusals for {} mid-capture points",
            parts_written_mid_capture
        ),
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
        "  audit: {} ms media of {} ms wall clock, {} ms reported, {} ms unreported, \
         {} ms spurious",
        report.media_ms(),
        report.wall_clock_ms(),
        report.reported_ms(),
        report.unreported_ms(),
        report.spurious_ms()
    );
    println!(
        "  live clips: {clips_rendered} rendered, {clips_decoded} decoded, \
         {open_part_refusals} open-part refusals, renders admitted {render_admitted} refused \
         {render_refused}"
    );
    println!(
        "  worst keyframe quantisation: {clip_quantisation_ms} ms over the requested range \
         (ceiling {CLIP_QUANTISATION_MS} ms)"
    );

    // ---- Phase 3: render saturation on the size-capped volume ----
    let saturation = saturate_render(&volume, &mut failures);

    let _ = std::fs::remove_dir_all(&scratch);

    let wall = started.elapsed().as_secs_f64();
    println!("\nsoak-live-clip summary");
    println!("  mutation                {mutation}");
    println!("  capture_window_ms       {SOAK_CAPTURE_MS}");
    println!("  sessions                {sessions}");
    println!("  interruptions           {}", plan.interruption_count());
    println!("  max_resumes             {MAX_RESUMES_PER_SESSION}");
    println!("  segments_encoded        {parts_total}");
    println!("  segments_decoded        {parts_decoded}");
    println!("  live_clips_rendered     {clips_rendered}");
    println!("  live_clips_decoded      {clips_decoded}");
    println!("  open_part_refusals      {open_part_refusals}");
    println!("  clip_quantisation_ms    {clip_quantisation_ms}");
    println!("  unreported_ms           {}", report.unreported_ms());
    println!("  spurious_ms             {}", report.spurious_ms());
    println!("  saturation_free_bytes   {}", saturation.free_before);
    println!("  saturation_capture      {}", saturation.capture_verdict);
    println!("  saturation_clip         {}", saturation.clip_verdict);
    println!(
        "  saturation_reason       {}",
        saturation.reason.as_deref().unwrap_or("(none)")
    );
    println!(
        "  time_compression        {SOAK_CAPTURE_MS} ms of stream time in {wall:.1} s of wall \
         clock ({:.0}x)",
        SOAK_CAPTURE_MS as f64 / 1000.0 / wall.max(0.001)
    );
    println!("  elapsed_secs            {wall:.1}");

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

struct Saturation {
    free_before: u64,
    capture_verdict: String,
    clip_verdict: String,
    reason: Option<String>,
}

/// Fill `volume` until free space reaches `SATURATION_RESERVE_BYTES`, then
/// assert the ordering CE-P8 requires: capture is still admitted and still
/// writes a part that decodes, while a **live clip** is refused with a
/// reason.
///
/// The clip is attempted through the real `render_live_clip`, so the refusal
/// is the one a caller would see rather than a call to the capacity type.
fn saturate_render(volume: &Path, failures: &mut Failures) -> Saturation {
    let mut out = Saturation {
        free_before: 0,
        capture_verdict: "not-run".into(),
        clip_verdict: "not-run".into(),
        reason: None,
    };
    let Some(free_before) = free_bytes(volume) else {
        failures.require("saturation_measure", false, "df could not read the volume");
        return out;
    };
    out.free_before = free_before;
    println!(
        "\nsaturation phase: volume {} has {free_before} bytes free, reserve is \
         {SATURATION_RESERVE_BYTES}",
        volume.display()
    );

    while let Some(free) = free_bytes(volume) {
        if free <= SATURATION_RESERVE_BYTES {
            break;
        }
        let want = free
            .saturating_sub(SATURATION_RESERVE_BYTES)
            .saturating_add(SATURATION_OVERSHOOT_BYTES);
        let path = volume.join(format!("ballast-{free}"));
        let Ok(mut file) = std::fs::File::create(&path) else {
            break;
        };
        {
            use std::io::Write;
            let mut left = want;
            let block = vec![0x5Au8; 64 * 1024];
            while left > 0 {
                let take = left.min(block.len() as u64) as usize;
                match file.write_all(&block[..take]) {
                    Ok(()) => left -= take as u64,
                    Err(_) => break,
                }
            }
        }
        drop(file);
    }

    let reserve = CaptureReserve {
        dir: volume.to_path_buf(),
        bytes: SATURATION_RESERVE_BYTES,
    };
    let Some(capacity) = CaptureCapacity::measure(&reserve) else {
        failures.require(
            "saturation_measure",
            false,
            "measure() returned None for the volume",
        );
        return out;
    };

    // Capture first, and prove survival with a real write.
    out.capture_verdict = match capacity.admit_capture(capacity.free_bytes()) {
        Ok(()) => "admitted".into(),
        Err(e) => format!("refused: {e}"),
    };
    failures.require(
        "saturation_capture_admitted",
        out.capture_verdict == "admitted",
        out.capture_verdict.clone(),
    );
    let part = volume.join("soak-live-capture-part.mkv");
    let written = encode_segment(&part, 2);
    let bytes = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if written.is_ok() && bytes > 0 {
        out.capture_verdict = format!("admitted and wrote {bytes} bytes");
        if let Err(e) = decodes_fully(&part) {
            failures.require("saturation_capture_playable", false, e);
        }
    } else {
        failures.require(
            "saturation_capture_survives",
            false,
            format!("capture was admitted but the write failed: {written:?}"),
        );
    }
    failures.require(
        "saturation_capture_survives",
        written.is_ok() && bytes > 0,
        format!("capture wrote {bytes} bytes"),
    );
    let _ = std::fs::remove_file(&part);

    // A real capture, with a real finalized part, on the saturated volume.
    let base = volume.join("soak-live-base.mkv");
    let mut tail = LiveCapture::new(base.clone());
    let ok = encode_segment(&base, 4).and_then(|()| {
        let ms = probe_duration_ms(&base)?;
        tail.finalize_part(1, ms).map_err(|e| e.to_string())?;
        tail.open_part(2).map_err(|e| e.to_string())
    });
    if let Err(e) = ok {
        failures.require("saturation_tail", false, e);
        out.clip_verdict = "not-run".into();
        return out;
    }

    let inputs = tail.clip_inputs(&LiveClipRequest::new(0, 3_000));
    let dest = volume.join("soak-live-clip.mkv");
    let attempt: Result<RenderedClip, LiveClipError> = match inputs {
        Ok(inputs) => render_live_clip(&tail, &inputs, &dest, &capacity),
        Err(e) => Err(e),
    };
    out.clip_verdict = match &attempt {
        Ok(clip) => format!("rendered {} bytes", clip.bytes),
        Err(e) => format!("refused: {e}"),
    };
    if let Err(e) = &attempt {
        out.reason = Some(e.to_string());
    }
    failures.require(
        "saturation_clip_refused",
        matches!(attempt, Err(LiveClipError::RenderRefused { .. })),
        format!(
            "expected a live clip to be refused with LiveClipError::RenderRefused, got {}",
            out.clip_verdict
        ),
    );
    let refusal = out.reason.clone().unwrap_or_default();
    failures.require(
        "saturation_refusal_names_numbers",
        refusal.contains(&SATURATION_RESERVE_BYTES.to_string())
            && refusal.contains(&format!("{}", capacity.free_bytes())),
        format!("refusal {refusal:?} did not name the free and reserve byte counts"),
    );
    failures.require(
        "saturation_clip_wrote_nothing",
        !dest.exists(),
        "a refused live clip must not have written an output".to_string(),
    );

    // And the daemon's own render admission agrees.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let registry_verdict = runtime.block_on(async {
        ResourceRegistry::new()
            .with_capture_reserve(reserve)
            .acquire(&ResourceLock::Disk { cap: 2 })
            .await
    });
    failures.require(
        "saturation_registry_refused",
        matches!(
            registry_verdict,
            Err(LockAcquireError::CaptureReserve { .. })
        ),
        format!("the registry said {registry_verdict:?}"),
    );

    // Leave the volume as found.
    let _ = std::fs::remove_file(&base);
    if let Ok(entries) = std::fs::read_dir(volume) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with("ballast-") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    println!("  capture: {}", out.capture_verdict);
    println!("  clip:    {}", out.clip_verdict);
    out
}
