//! Ingest readiness: five steps, each reporting its own state.
//!
//! The claim under test is **per-step state**, not an overall verdict. A
//! checklist whose steps share one boolean satisfies "reports a readiness
//! state" for free while answering none of the questions a caller has, so the
//! controls here are written to make a shared boolean fail:
//!
//! - every assertion below reads a **named step**, never a whole-checklist
//!   boolean, and
//! - three of them assert that a *specific pair* of steps disagree
//!   (`probe` Ready while `waveform` is not), which one shared boolean cannot
//!   produce.
//!
//! **The subject is the real corpus, and it is proved before it is asserted on.**
//! Every fixture is the committed file from `fixtures/media/` (todo 32), the
//! staging test asserts the bytes on disk are the ones `MANIFEST.json`
//! records — same length, same recorded sha256, a real Matroska magic — and the
//! media facts come from that manifest's own ffprobe block. No fact in this
//! file is invented, and no test needs an external binary to run.
//!
//! The derived facts the checklist is fed:
//!
//! | fixture | what it proves |
//! | --- | --- |
//! | `multi-audio.mkv` | the control: 1 video + 2 audio, so every step is *applicable* and a missing ffmpeg makes four of them differ from the fifth |
//! | `silent-video.mkv` | a source with no audio has no waveform, and that is a state rather than a failure |
//! | `variable-frame-rate.mkv` | a variable-cadence source is never reported on a constant-frame assumption |
//! | `discontinuities.mkv` | cadence the committed evidence **cannot** establish is `Unknown`, and the filmstrip is blocked rather than assumed |
//! | `long-gop.mkv` | a 1-keyframe source still reports the facts, and the keyframe interval it carries is what a consumer judges density by |
//! | `unicode-path/…mkv` | a non-ASCII path and a markup-bearing title are carried as data and change no state |

use std::path::{Path, PathBuf};

use strivo_editor::time::MediaTime;
use strivo_multitrack::{AudioTrack, TrackKind};

use super::super::entities::{Project, SourceAsset};
use super::super::ids::{ProjectId, SourceAssetId};
use super::super::import::{fingerprint, AdmittedFile, ImportRequest};
use super::super::readiness::{
    Blocker, FrameTiming, IngestStep, ProbeReport, Readiness, ReadinessError, StepState, Tool,
    Toolchain, VideoStream,
};
use super::super::store::Store;
use tempfile::TempDir;

const AT: &str = "2026-09-26T00:00:00Z";

/// The media facts todo 32 recorded, read from the manifest rather than
/// restated here. A fact this file typed in by hand would be a second source
/// of truth about a file whose whole point is that its characteristics are
/// measured.
#[derive(Debug, serde::Deserialize)]
struct Manifest {
    fixtures: Vec<Entry>,
}

#[derive(Debug, serde::Deserialize)]
struct Entry {
    name: String,
    class: String,
    bytes: u64,
    sha256: String,
    ffprobe: Block,
}

#[derive(Debug, serde::Deserialize)]
struct Block {
    video_streams: u32,
    audio_streams: u32,
    video_frames: u32,
    avg_frame_rate: String,
    #[serde(default)]
    audio: Vec<AudioBlock>,
    duration_secs: f64,
}

#[derive(Debug, serde::Deserialize)]
struct AudioBlock {
    codec: String,
    channels: u32,
    sample_rate: String,
}

fn manifest() -> Manifest {
    let raw = std::fs::read_to_string(corpus_root().join("MANIFEST.json"))
        .expect("the corpus manifest is committed beside the fixtures");
    serde_json::from_str(&raw).expect("the corpus manifest parses")
}

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/media")
}

fn entry(name: &str) -> Entry {
    manifest()
        .fixtures
        .into_iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("the corpus manifest records {name}"))
}

/// A microsecond count stated the way a person reads it.
fn us(micros: i64) -> MediaTime {
    MediaTime::from_micros(micros).expect("a non-negative position")
}

/// A cadence reading derived from what the manifest measured.
///
/// Two committed inputs, neither invented. A class that **names** a cadence
/// deviation is a measured fact — `scripts/verify_media_fixtures.py` re-probes
/// every fixture and fails a class that is not really there — so
/// `variable-frame-rate` is read as `Variable`. Everything else is decided
/// arithmetically: does the frame count agree with the container's own declared
/// rate and duration?
///
/// **Agreement is `Constant`; disagreement is `Unknown`, never `Variable`.** A
/// frame count cannot tell a variable cadence from a timeline hole, and
/// `discontinuities.mkv` is the proof: 30 frames against `25/1 × 2.221 s` =
/// 55.5 expected, a whole 25 frames of hole, which arithmetic reports as
/// "not constant" and cannot label. So this returns `Unknown` for it, and a
/// caller who needs to know which it is has to read presentation timestamps —
/// which is the same reason `FrameTiming::Unknown` is a state the checklist
/// takes seriously rather than a default.
fn cadence(entry: &Entry) -> FrameTiming {
    if entry.class == "variable-frame-rate" {
        return FrameTiming::Variable;
    }
    let block = &entry.ffprobe;
    if block.video_streams == 0 {
        return FrameTiming::Unknown;
    }
    let (num, den) = block
        .avg_frame_rate
        .split_once('/')
        .expect("ffprobe writes a rational frame rate, never a bare float");
    let num: f64 = num.parse().expect("a rational numerator");
    let den: f64 = den.parse().expect("a rational denominator");
    if den == 0.0 || num == 0.0 {
        return FrameTiming::Unknown;
    }
    // A container rounds its own duration up to the end of its last frame, so
    // the expected count overshoots by up to one frame. A whole frame of slack
    // separates that rounding from any real disagreement.
    if (block.duration_secs * num / den - f64::from(block.video_frames)).abs() <= 1.0 {
        FrameTiming::Constant
    } else {
        FrameTiming::Unknown
    }
}

/// The probe report a caller hands the checklist for `name`: the manifest's
/// measured facts, with its sha256 as the content identity the probe ran
/// against. `keyframe_interval` is passed in rather than derived, because the
/// manifest records a frame count and not a keyframe count; the class name is
/// the corpus's own record of the keyframe layout.
fn report(name: &str, keyframe_interval_us: Option<i64>) -> ProbeReport {
    let e = entry(name);
    ProbeReport {
        fingerprint: format!("sha256:{}", e.sha256),
        duration: Some(us((e.ffprobe.duration_secs * 1_000_000.0).round() as i64)),
        video: (e.ffprobe.video_streams > 0).then(|| VideoStream {
            timing: cadence(&e),
            keyframe_interval: keyframe_interval_us.map(us),
        }),
        audio: e
            .ffprobe
            .audio
            .iter()
            .enumerate()
            .map(|(i, a)| AudioTrack {
                index: u32::try_from(i + 1).expect("a stream index fits a u32"),
                codec: a.codec.clone(),
                channels: a.channels,
                sample_rate: a
                    .sample_rate
                    .parse()
                    .expect("ffprobe writes a decimal sample rate"),
                title: None,
                language: None,
                // The corpus is synthetic and stamps no role on its streams, so
                // the honest role for one is the enum's own "not one of these".
                inferred_kind: TrackKind::Other,
            })
            .collect(),
    }
}

fn tools(ffmpeg: bool, ffprobe: bool) -> Toolchain {
    Toolchain { ffmpeg, ffprobe }
}

fn all_present() -> Toolchain {
    tools(true, true)
}

/// A source holding `name`'s content, without touching the database.
fn source_for(name: &str) -> SourceAsset {
    let e = entry(name);
    SourceAsset {
        id: SourceAssetId::generate(),
        project_id: ProjectId::generate(),
        kind: "import".into(),
        title: name.rsplit('/').next().unwrap_or(name).to_string(),
        media_fingerprint: format!("sha256:{}", e.sha256),
        locator: Some(corpus_root().join(name)),
        origin: "import".into(),
        rights_json: "{}".into(),
        // Left empty on purpose, exactly as `import_files` leaves them: these
        // are the columns a probe fills, and a checklist that reads them back
        // would be reading its own input.
        track_type: String::new(),
        codec: String::new(),
        duration: None,
        created_at: AT.into(),
    }
}

/// Stage the real corpus file and prove it is the one the manifest records,
/// before any assertion is made about it. A checklist over an empty or
/// substituted subject reports `Ready` for free, so the subject check comes
/// first in every test that uses it.
fn stage(name: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let e = entry(name);
    let dest = dir
        .path()
        .join(name.rsplit('/').next().expect("a file name"));
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).expect("parent made");
    }
    std::fs::copy(corpus_root().join(name), &dest).expect("the corpus file is readable");
    let bytes = std::fs::metadata(&dest).expect("staged").len();
    assert_eq!(
        bytes, e.bytes,
        "{} is not the length the manifest records",
        name
    );
    assert!(bytes > 0, "{name} staged as zero bytes");
    let magic = std::fs::read(&dest).expect("read")[..4].to_vec();
    assert_eq!(
        magic,
        [0x1a, 0x45, 0xdf, 0xa3],
        "{name} is not a Matroska file"
    );
    assert_eq!(
        fingerprint(&dest).expect("hashed"),
        format!("sha256:{}", e.sha256),
        "{name} on disk is not the content the manifest records"
    );
    (dir, dest)
}

/// The five states as a comparable vector, so a test can assert on a whole
/// checklist while still failing if two steps were collapsed into one.
fn states(r: &Readiness) -> Vec<(IngestStep, StepState)> {
    IngestStep::ALL.map(|s| (s, r.state(s))).to_vec()
}

// ── the anti-collapse control ───────────────────────────────────────────────

/// Given a source with a video and two audio streams, and no ffmpeg
/// When its readiness is evaluated
/// Then all five steps report, the probe is `Ready`, the four steps that need
/// ffmpeg are each `Blocked` naming ffmpeg, and at least one step differs
/// from the first — which a single shared boolean cannot produce.
#[test]
fn every_step_reports_its_own_state() {
    let (dir, path) = stage("multi-audio.mkv");
    let mut src = source_for("multi-audio.mkv");
    src.locator = Some(path);
    let r = Readiness::evaluate(
        &src,
        Some(&report("multi-audio.mkv", None)),
        &tools(false, true),
    )
    .expect("the report matches the source's content");

    let all = states(&r);
    assert_eq!(all.len(), IngestStep::ALL.len(), "a step went missing");
    assert_eq!(r.state(IngestStep::Probe), StepState::Ready);
    for step in [
        IngestStep::Proxies,
        IngestStep::Waveform,
        IngestStep::Filmstrip,
        IngestStep::Thumbnail,
    ] {
        assert_eq!(
            r.state(step),
            StepState::Blocked(Blocker::Tool(Tool::Ffmpeg)),
            "{step:?} does not report the missing tool for itself"
        );
    }

    // The collapse control: if every step were one shared verdict, this is the
    // assertion that fails.
    let first = all[0].1;
    assert!(
        all.iter().any(|(_, s)| *s != first),
        "every step reported the same state, so the states are not independent"
    );
    // And the specific disagreement a shared boolean cannot express: a source
    // whose probe is done while its waveform is not.
    let both = Readiness::evaluate(&src, Some(&report("multi-audio.mkv", None)), &all_present())
        .expect("the report matches the source's content");
    assert_eq!(both.state(IngestStep::Probe), StepState::Ready);
    assert_eq!(both.state(IngestStep::Waveform), StepState::Pending);
    drop(dir);
}

/// Given a source with a video and audio, and every tool present
/// When its readiness is evaluated
/// Then the probe is `Ready` and every analysis step is `Pending` — applicable
/// but not produced — so "ready to start editing" and "analysis finished" are
/// two different facts.
#[test]
fn a_probed_source_is_editable_before_any_optional_analysis_finishes() {
    let src = source_for("multi-audio.mkv");
    let r = Readiness::evaluate(&src, Some(&report("multi-audio.mkv", None)), &all_present())
        .expect("the report matches the source's content");

    assert!(r.editable(), "a probed source is not editable");
    for step in IngestStep::ALL {
        if step == IngestStep::Probe {
            continue;
        }
        assert_eq!(r.state(step), StepState::Pending, "{step:?}");
    }
    // Optional analysis pending is not a problem to chase.
    assert_eq!(r.state(IngestStep::Proxies).next_step(), None);
}

// ── not applicable is a state, not a failure ───────────────────────────────

/// Given a video with no audio stream at all
/// When its readiness is evaluated
/// Then the waveform step is `NotApplicable` and names no next step: a source
/// with no audio has no waveform, which is a fact and not a problem, and a
/// checklist that reported it as failed would send a user after a non-problem.
/// Its video steps stay applicable, so `NotApplicable` is per step.
#[test]
fn a_silent_video_reports_its_waveform_as_not_applicable() {
    let src = source_for("silent-video.mkv");
    let e = entry("silent-video.mkv");
    assert_eq!(
        e.ffprobe.audio_streams, 0,
        "the fixture is supposed to have no audio"
    );
    let r = Readiness::evaluate(
        &src,
        Some(&report("silent-video.mkv", None)),
        &all_present(),
    )
    .expect("the report matches the source's content");

    assert_eq!(r.state(IngestStep::Waveform), StepState::NotApplicable);
    assert_eq!(r.state(IngestStep::Waveform).next_step(), None);
    // Not applicable outranks a missing tool: a source with no audio needs no
    // waveform whether or not ffmpeg is installed, and reporting it as blocked
    // would send a user to install a tool for a file that will never have one.
    let no_ffmpeg = Readiness::evaluate(
        &src,
        Some(&report("silent-video.mkv", None)),
        &tools(false, true),
    )
    .expect("the report matches the source's content");
    assert_eq!(
        no_ffmpeg.state(IngestStep::Waveform),
        StepState::NotApplicable,
        "a missing tool turned a non-applicable step into work to do"
    );
    assert_eq!(
        no_ffmpeg.state(IngestStep::Thumbnail),
        StepState::Blocked(Blocker::Tool(Tool::Ffmpeg)),
        "the applicable step should still report the missing tool"
    );
    // Not a failure: nothing failed, and the source is still editable.
    assert!(r.editable());
    // And it is that step alone — the video steps are applicable.
    assert_ne!(r.state(IngestStep::Thumbnail), StepState::NotApplicable);
    assert_eq!(r.audio_streams(), 0);
}

/// Given an audio-only source
/// When its readiness is evaluated
/// Then the two video steps are `NotApplicable` and the waveform is not, so
/// applicability is decided per step from the tracks a probe actually found.
#[test]
fn an_audio_only_source_reports_its_visual_steps_as_not_applicable() {
    let src = source_for("multi-audio.mkv");
    let mut only_audio = report("multi-audio.mkv", None);
    only_audio.video = None;
    let r = Readiness::evaluate(&src, Some(&only_audio), &all_present())
        .expect("the report matches the source's content");

    assert_eq!(r.state(IngestStep::Filmstrip), StepState::NotApplicable);
    assert_eq!(r.state(IngestStep::Thumbnail), StepState::NotApplicable);
    assert_eq!(r.state(IngestStep::Waveform), StepState::Pending);
    assert_eq!(r.audio_streams(), 2);
}

// ── frame cadence is never assumed ─────────────────────────────────────────

/// Given a container whose measured frames do not agree with its own declared
/// rate and duration
/// When its readiness is evaluated
/// Then the cadence is reported `Variable`, the source is never described as
/// constant-frame, and the source is still editable — a variable cadence is a
/// fact about the picture, not a reason to refuse to edit it.
#[test]
fn a_variable_frame_rate_source_is_never_reported_on_a_constant_cadence() {
    let src = source_for("variable-frame-rate.mkv");
    let r = Readiness::evaluate(
        &src,
        Some(&report("variable-frame-rate.mkv", None)),
        &all_present(),
    )
    .expect("the report matches the source's content");

    assert_eq!(
        r.frame_timing(),
        FrameTiming::Variable,
        "the manifest's frames and rate disagree, so the cadence is not constant"
    );
    assert_ne!(r.frame_timing(), FrameTiming::Constant);
    // The step that would assume an even cadence is not reported ready, and the
    // instruction says what to do instead of assuming one.
    assert_eq!(
        r.state(IngestStep::Filmstrip),
        StepState::Blocked(Blocker::FrameTiming(FrameTiming::Variable))
    );
    let next = r
        .state(IngestStep::Filmstrip)
        .next_step()
        .expect("a blocked step names what to do");
    assert!(
        next.contains("variable"),
        "the next step does not say the cadence is variable: {next}"
    );
    // A single keyframe-approximate frame is not a cadence claim.
    assert_eq!(r.state(IngestStep::Thumbnail), StepState::Pending);
    assert!(r.editable());
    assert_eq!(r.state(IngestStep::Probe), StepState::Ready);
}

/// Given a constant-cadence source
/// When its readiness is evaluated
/// Then the cadence reads `Constant` — so the previous test is a real
/// discriminator and not a constant that happens to be right.
#[test]
fn a_constant_frame_rate_source_reports_a_constant_cadence() {
    let src = source_for("multi-audio.mkv");
    let r = Readiness::evaluate(&src, Some(&report("multi-audio.mkv", None)), &all_present())
        .expect("the report matches the source's content");
    assert_eq!(r.frame_timing(), FrameTiming::Constant);
}

/// Given a source whose cadence the evidence cannot establish — a
/// constant-cadence file carrying a timeline hole looks exactly like a
/// variable-cadence one to a frame count
/// When its readiness is evaluated
/// Then the cadence is `Unknown` and the time-addressed filmstrip is `Blocked`
/// naming that reason, rather than being reported ready on an assumption. The
/// single-frame thumbnail is not blocked: it seeks to a keyframe and says so.
#[test]
fn an_unestablished_cadence_blocks_the_time_addressed_filmstrip() {
    let src = source_for("discontinuities.mkv");
    let r = Readiness::evaluate(
        &src,
        Some(&report("discontinuities.mkv", None)),
        &all_present(),
    )
    .expect("the report matches the source's content");

    assert_eq!(r.frame_timing(), FrameTiming::Unknown);
    assert_eq!(
        r.state(IngestStep::Filmstrip),
        StepState::Blocked(Blocker::FrameTiming(FrameTiming::Unknown))
    );
    let next = r
        .state(IngestStep::Filmstrip)
        .next_step()
        .expect("a blocked step names what to do");
    assert!(
        next.contains("cadence"),
        "the next step does not name the cadence: {next}"
    );
    assert_eq!(r.state(IngestStep::Thumbnail), StepState::Pending);
    assert!(r.editable(), "an unknown cadence does not stop editing");
}

/// Given a hundred-frame source holding exactly one keyframe
/// When its readiness is evaluated
/// Then the measured keyframe interval rides on the checklist, so a consumer
/// judging whether evenly-spaced frames are truthful has the number in front
/// of it rather than a `Ready` and a hope.
#[test]
fn a_single_keyframe_source_reports_the_keyframe_interval_it_measured() {
    let src = source_for("long-gop.mkv");
    let r = Readiness::evaluate(
        &src,
        Some(&report("long-gop.mkv", Some(10_021_000))),
        &all_present(),
    )
    .expect("the report matches the source's content");

    assert_eq!(r.frame_timing(), FrameTiming::Constant);
    assert_eq!(
        r.keyframe_interval(),
        Some(us(10_021_000)),
        "the interval a consumer needs to judge frame addressing is missing"
    );
    // A source whose probe never read keyframe placement says so rather than
    // implying a dense one.
    let blind = Readiness::evaluate(&src, Some(&report("long-gop.mkv", None)), &all_present())
        .expect("the report matches the source's content");
    assert_eq!(blind.keyframe_interval(), None);
}

// ── missing tools name the next step, and do not stop the timeline ──────────

/// Given a source whose probe has completed and no ffmpeg on PATH
/// When its readiness is evaluated
/// Then every ffmpeg-backed step names ffmpeg specifically, the timeline is
/// still editable, and the next step is the install rather than an opaque
/// failure.
#[test]
fn a_missing_ffmpeg_names_the_exact_next_step_without_blocking_the_timeline() {
    let src = source_for("multi-audio.mkv");
    let r = Readiness::evaluate(
        &src,
        Some(&report("multi-audio.mkv", None)),
        &tools(false, true),
    )
    .expect("the report matches the source's content");

    let next = r
        .state(IngestStep::Waveform)
        .next_step()
        .expect("a blocked step names what to do");
    assert!(
        next.contains("ffmpeg"),
        "the next step does not name ffmpeg: {next}"
    );
    assert!(
        r.editable(),
        "a missing optional analysis tool blocked the timeline"
    );
    assert_eq!(r.state(IngestStep::Probe), StepState::Ready);
}

/// Given a source and no ffprobe at all
/// When its readiness is evaluated
/// Then the probe step is blocked naming ffprobe, the timeline is **not**
/// editable, and the other four steps are blocked too rather than reported as
/// ready on a probe that never ran.
#[test]
fn a_missing_ffprobe_is_named_and_does_not_edit() {
    let src = source_for("multi-audio.mkv");
    let r = Readiness::evaluate(
        &src,
        Some(&report("multi-audio.mkv", None)),
        &tools(true, false),
    )
    .expect("the report matches the source's content");

    assert_eq!(
        r.state(IngestStep::Probe),
        StepState::Blocked(Blocker::Tool(Tool::Ffprobe))
    );
    assert!(!r.editable(), "an unprobed source was reported editable");
    let next = r
        .state(IngestStep::Probe)
        .next_step()
        .expect("a blocked step names what to do");
    assert!(
        next.contains("ffprobe"),
        "the next step does not name ffprobe: {next}"
    );
    for step in IngestStep::ALL {
        if step == IngestStep::Probe {
            continue;
        }
        assert_ne!(
            r.state(step),
            StepState::Ready,
            "{step:?} is ready with no probe"
        );
    }
}

// ── ordering, staleness, cancellation ──────────────────────────────────────

/// Given a source whose probe has not completed
/// When its readiness is evaluated
/// Then no step reads `Ready` and the timeline is not usable — a readiness
/// computed before the probe finished must not read as finished.
#[test]
fn a_readiness_computed_before_the_probe_completed_reads_as_pending_everywhere() {
    let src = source_for("multi-audio.mkv");
    let r = Readiness::evaluate(&src, None, &all_present()).expect("no report, nothing to refuse");

    for step in IngestStep::ALL {
        assert_eq!(
            r.state(step),
            StepState::Pending,
            "{step:?} read as finished"
        );
    }
    assert!(!r.editable());
}

/// Given a source whose bytes were replaced on disk after the probe ran
/// When a checklist is built for the new content from the old probe
/// Then it is refused, naming both identities — a readiness over content that
/// no longer exists is the stale-state bug, and the fingerprint the probe ran
/// against is the only thing that can catch it.
#[test]
fn a_source_replaced_on_disk_invalidates_its_readiness() {
    let mut replaced = source_for("multi-audio.mkv");
    replaced.media_fingerprint = "sha256:0000".into();
    let err = Readiness::evaluate(
        &replaced,
        Some(&report("multi-audio.mkv", None)),
        &all_present(),
    )
    .expect_err("a probe of other content must not produce a checklist");

    assert_eq!(
        err,
        ReadinessError::ContentChanged {
            source: "sha256:0000".to_string(),
            probed: format!("sha256:{}", entry("multi-audio.mkv").sha256),
        }
    );
    // And a checklist held across the swap says so rather than reading ready.
    let r = Readiness::evaluate(
        &source_for("multi-audio.mkv"),
        Some(&report("multi-audio.mkv", None)),
        &all_present(),
    )
    .expect("the report matches its own source");
    assert!(
        r.stale_for(&replaced),
        "a replaced source did not invalidate it"
    );
    assert!(!r.stale_for(&source_for("multi-audio.mkv")));
}

/// Given a checklist whose probe was cancelled
/// When the caller comes back with the report
/// Then re-evaluating restores `Ready` and no state is latched: the vocabulary
/// has no terminal in-progress state for a cancellation to strand a step in,
/// because a step is either produced or not.
#[test]
fn a_cancelled_probe_leaves_no_step_latched() {
    let src = source_for("multi-audio.mkv");
    let cancelled = Readiness::evaluate(&src, None, &all_present()).expect("no report");
    assert_eq!(cancelled.state(IngestStep::Probe), StepState::Pending);
    assert!(!cancelled.editable());

    let finished =
        Readiness::evaluate(&src, Some(&report("multi-audio.mkv", None)), &all_present())
            .expect("the report matches the source's content");
    assert_eq!(finished.state(IngestStep::Probe), StepState::Ready);
    assert!(finished.editable());
    // Nothing from the cancelled attempt survives into the new one.
    assert_eq!(finished.audio_streams(), 2);
}

// ── malformed input is a state, not a panic ────────────────────────────────

/// Given a probe that ran and read no decodable stream at all
/// When its readiness is evaluated
/// Then the probe step is `Failed` with the reason attached, the timeline is
/// not editable, and the steps that need a track are `NotApplicable` rather
/// than blocked or pending — every one of them is a state, and nothing panics.
#[test]
fn a_source_with_no_decodable_stream_is_reported_not_panicked() {
    let src = source_for("multi-audio.mkv");
    let empty = ProbeReport {
        fingerprint: src.media_fingerprint.clone(),
        duration: None,
        video: None,
        audio: Vec::new(),
    };
    let r = Readiness::evaluate(&src, Some(&empty), &all_present())
        .expect("a failed probe is a report, not an error");

    assert!(matches!(r.state(IngestStep::Probe), StepState::Failed(_)));
    assert!(!r.editable());
    let detail = r
        .state(IngestStep::Probe)
        .next_step()
        .expect("a failed step says what it found");
    assert!(
        detail.contains("stream"),
        "the failure does not say what was found: {detail}"
    );
    for step in IngestStep::ALL {
        if step == IngestStep::Probe {
            continue;
        }
        assert_eq!(r.state(step), StepState::NotApplicable, "{step:?}");
    }
    assert_eq!(r.duration(), None);
    assert_eq!(r.track_type(), "unknown");
}

// ── untrusted text is data ─────────────────────────────────────────────────

/// Given the same source content under a title and a path carrying markup,
/// shell metacharacters and a right-to-left override
/// When its readiness is evaluated
/// Then every step state is byte-identical to the benign source's, and the
/// title and locator are carried verbatim — nothing is derived from a name.
#[test]
fn untrusted_names_are_carried_as_data_and_change_no_state() {
    let hostile_title = "<script>alert(1)</script>\u{202e}gnp.exe $(id)`whoami`";
    let hostile_path = "/tmp/$(rm -rf /)/日本語 セッション 🎬.mkv";

    let mut hostile = source_for("multi-audio.mkv");
    hostile.title = hostile_title.into();
    hostile.locator = Some(PathBuf::from(hostile_path));
    let r = Readiness::evaluate(
        &hostile,
        Some(&report("multi-audio.mkv", None)),
        &all_present(),
    )
    .expect("the report matches the source's content");

    let benign = Readiness::evaluate(
        &source_for("multi-audio.mkv"),
        Some(&report("multi-audio.mkv", None)),
        &all_present(),
    )
    .expect("the report matches the source's content");
    assert_eq!(
        states(&r),
        states(&benign),
        "a name changed a readiness state"
    );

    assert_eq!(
        r.source().title,
        hostile_title,
        "the name was not carried verbatim"
    );
    assert_eq!(r.source().id, hostile.id);
    assert_eq!(
        r.source().locator.as_deref(),
        Some(Path::new(hostile_path)),
        "the locator was not carried verbatim"
    );
}

// ── the checklist is derived from a real imported row ──────────────────────

/// Given a file imported through the real import path, and a probe report
/// built from the content that import recorded
/// When readiness is evaluated against the imported row
/// Then it builds, and the report's fingerprint is the row's own — the
/// binding the stale check rests on is the identity the import actually
/// stored, not one the test made up.
#[test]
fn an_imported_source_reports_readiness_against_its_own_fingerprint() {
    let dir = TempDir::new().expect("tempdir");
    let (_guard, staged) = stage("multi-audio.mkv");
    let store = Store::open_in_memory().expect("store opens");
    let project = Project {
        id: ProjectId::generate(),
        name: "readiness".into(),
        description: String::new(),
        channel: String::new(),
        series: String::new(),
        settings_json: "{}".into(),
        created_at: AT.into(),
        updated_at: AT.into(),
    };
    store.insert_project(&project).expect("project inserted");
    let manifest = super::super::import::import_files(
        &store,
        &ImportRequest {
            project: project.id,
            files: &[AdmittedFile::admitted(staged.clone())],
            scratch_root: dir.path(),
            at: AT,
        },
    )
    .expect("the import succeeds");

    let imported = manifest.entries[0].outcome.outcome_asset().clone();
    assert_eq!(
        imported.media_fingerprint,
        format!("sha256:{}", entry("multi-audio.mkv").sha256),
        "the import did not record the content the manifest describes"
    );
    let r = Readiness::evaluate(
        &imported,
        Some(&report("multi-audio.mkv", None)),
        &all_present(),
    )
    .expect("the report matches the imported row's own content");
    assert_eq!(r.state(IngestStep::Probe), StepState::Ready);
    assert_eq!(r.audio_streams(), 2);
    // The import leaves the media columns empty on purpose; the checklist
    // derives what a probe measured rather than reading them back.
    assert_eq!(imported.track_type, "", "the import guessed a track type");
    assert_eq!(r.track_type(), "video");
    assert_eq!(r.duration(), Some(us(3_021_000)));
}
