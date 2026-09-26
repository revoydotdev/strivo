//! Layered B-roll over continuing source audio — report item E08.
//!
//! The gap `docs/CREATOR-EDITION-REPORT-2026-09-07.md:96` records is that
//! `Edl::insert_broll` is a **sequential** insert: the B-roll becomes another
//! entry in an ordered cut list, so it lengthens the output and splits the
//! source around itself. E08 asks for the opposite — the B-roll drawn *over* a
//! source that keeps playing, including keeps playing *audibly*.
//!
//! ## What is measured where
//!
//! * The **model** claims (output length, refusals, the transform at three
//!   instants) are exact integer assertions on the compiled plan.
//! * The **render** claims are measured on a decoded artifact, never on an exit
//!   code or a file size.
//! * The **keyframe** claim is checked twice against the same three instants —
//!   once on `PlanLayer::transform_at` and once on the pixels — because the
//!   executor emits that model's answer as an ffmpeg expression and the two
//!   drifting apart is the failure that matters.
//!
//! Decode, not addressing: todo 43 measured that seeking lands on the nearest
//! keyframe. The keyframe fixture is therefore written all-intra (`-g 1`), so
//! the sampled frame is the frame at that instant *for that fixture*. On real
//! media a sample is keyframe-accurate, not frame-exact.
//!
//! ## Why the pixel assertions render losslessly
//!
//! Two things were measured on this box before any assertion was written, and
//! both change what a decoded-pixel comparison can mean:
//!
//! * **Encoded bytes are not reproducible.** Two `libx264` encodes of the same
//!   input on this 24-core host have different SHA-256 sums. Comparing file
//!   hashes across two renders would measure x264's thread scheduling.
//! * **Lossy re-encode is not spatially local.** A changed region perturbs
//!   every decoded pixel of the frame, because the quantiser decides per
//!   macroblock. Measured: compositing a 64x64 overlay onto a 128x72 frame
//!   changed all 128 columns, not 64.
//!
//! So the decoded pixels *are* reproducible (identical input gives identical
//! output), and the pixel assertions render through `ffv1`/`flac`, which makes
//! the difference between two renders exactly the set of composited pixels.
//! Under `libx264` the same assertions would pass or fail on compression noise.

use std::path::{Path, PathBuf};
use std::process::Command;

use strivo_editor::render::{
    compile, export, is_passthrough, CaptionMode, CaptionSpec, ClipRequest, Container, Coordinate,
    Frames, Interpolation, Keyframe, LayerKind, LayerSpec, OutputProfile, PlanLayer, RenderError,
    RenderRequest, TrackSpec, Transform, VideoIntent,
};
use strivo_editor::time::{Interval, MediaTime, Rate, SourceRef, SourceSpan, TimeBase};
use strivo_editor::{Cut, CutKind, Edl};

const BED_SECS: i64 = 3_000_000;
const BROLL_SECS: i64 = 2_000_000;
const OVERLAY_AT: i64 = 500_000;

fn us(value: i64) -> MediaTime {
    MediaTime::from_micros(value).expect("non-negative media time")
}

fn span(source: &str, start: i64, end: i64) -> SourceSpan {
    SourceSpan::new(
        SourceRef::from_locator(source),
        TimeBase::from_rate(1, 1).expect("identity timebase"),
        Interval::new(us(start), us(end)).expect("ordered"),
        Rate::ONE,
    )
    .expect("non-empty source span")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/media")
        .join(name)
}

/// The real corpus bed: two audio streams, aac stereo, 128x72 at 10 fps.
fn bed_path() -> PathBuf {
    let path = fixture("multi-audio.mkv");
    assert!(
        path.exists(),
        "the media corpus is committed; {} missing means the checkout is partial",
        path.display()
    );
    path
}

fn tool(name: &str) -> bool {
    Command::new(name)
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ── the gap, as it stood ────────────────────────────────────────────────

// ── the model ───────────────────────────────────────────────────────────

/// One source bed, one B-roll overlay.
fn layered(bed: &Path, broll: &Path) -> RenderRequest {
    let bed = bed.display().to_string();
    let broll = broll.display().to_string();
    RenderRequest {
        subject: "r1".into(),
        revision: "rev-1".into(),
        clips: vec![
            ClipRequest {
                cut: 0,
                span: span(&bed, 0, BED_SECS),
                fade_in: MediaTime::ZERO,
                fade_out: MediaTime::ZERO,
                overlay_at: None,
            },
            ClipRequest {
                cut: 1,
                span: span(&broll, 0, BROLL_SECS),
                fade_in: MediaTime::ZERO,
                fade_out: MediaTime::ZERO,
                overlay_at: Some(us(OVERLAY_AT)),
            },
        ],
        layers: vec![
            LayerSpec {
                id: "programme".into(),
                kind: LayerKind::Video,
                transform: Transform::default(),
                colour: Default::default(),
                clips: vec![0],
                visible: true,
                keyframes: Vec::new(),
            },
            LayerSpec {
                id: "broll".into(),
                kind: LayerKind::Overlay,
                transform: Transform::default(),
                colour: Default::default(),
                clips: vec![1],
                visible: true,
                keyframes: Vec::new(),
            },
        ],
        transitions: Vec::new(),
        tracks: vec![TrackSpec {
            label: "programme".into(),
            bus: "master".into(),
            gain_db: 0.0,
            pan: 0.0,
            muted: false,
            solo: false,
            insert: String::new(),
            clips: vec![0],
        }],
        effects: Vec::new(),
        loudness: None,
        captions: CaptionSpec {
            mode: CaptionMode::None,
            style_json: String::new(),
            coordinate: Coordinate::Source,
            safe_area: true,
            cues: Vec::new(),
        },
        video: VideoIntent {
            filter_complex: String::new(),
            colour: None,
            frames: None,
        },
        container: Container {
            extension: "mkv".into(),
            video_codec: "libx264".into(),
            audio_codec: "aac".into(),
            stream_copy: false,
        },
        profile: OutputProfile {
            name: "test".into(),
            renderer: "ffmpeg".into(),
            renderer_version: "n0".into(),
        },
    }
}

fn locators(bed: &Path, broll: &Path) -> Edl {
    Edl {
        recording_id: "r1".into(),
        cuts: vec![
            Cut {
                start_sec: 0.0,
                end_sec: BED_SECS as f32 / 1e6,
                kind: CutKind::Source {
                    source_path: bed.display().to_string(),
                },
                fade_in_sec: 0.0,
                fade_out_sec: 0.0,
            },
            Cut {
                start_sec: 0.0,
                end_sec: BROLL_SECS as f32 / 1e6,
                kind: CutKind::Broll {
                    broll_path: broll.display().to_string(),
                },
                fade_in_sec: 0.0,
                fade_out_sec: 0.0,
            },
        ],
    }
}

/// The same revision with the overlay layer removed.
fn without_overlay(bed: &Path, broll: &Path) -> RenderRequest {
    let mut req = layered(bed, broll);
    req.layers[1].clips.clear();
    req.clips.remove(1);
    req
}

#[test]
fn the_overlay_is_placed_and_does_not_extend_the_output() {
    let req = layered(Path::new("/bed.mkv"), Path::new("/broll.mkv"));
    let plan = compile(&req).expect("a layered revision compiles");

    assert_eq!(
        plan.output_duration,
        us(BED_SECS),
        "three seconds of source is three seconds of output, whatever is drawn over it"
    );
    assert_eq!(plan.clips.len(), 2, "both cuts are placed");
    assert!(
        !plan.clips[0].overlay,
        "the programme lane consumes the timeline"
    );
    assert!(plan.clips[1].overlay, "the B-roll is drawn over it");
    assert_eq!(
        plan.clips[1].output,
        Interval::new(us(OVERLAY_AT), us(OVERLAY_AT + BROLL_SECS)).expect("ordered"),
        "the overlay occupies the output span its placement names"
    );
    assert_eq!(
        plan.timeline.entries().len(),
        1,
        "one uninterrupted source segment — the bed is never cut to make room"
    );
}

#[test]
fn an_overlay_past_the_end_of_the_output_is_refused_not_clamped() {
    let mut req = layered(Path::new("/bed.mkv"), Path::new("/broll.mkv"));
    req.clips[1].overlay_at = Some(us(BED_SECS - 500_000));
    assert!(
        matches!(
            compile(&req),
            Err(RenderError::OverlayOutOfRange { cut: 1, .. })
        ),
        "a layer time the output cannot hold is refused, not pulled back inside"
    );
}

#[test]
fn an_unplaced_overlay_clip_is_refused() {
    let mut req = layered(Path::new("/bed.mkv"), Path::new("/broll.mkv"));
    req.clips[1].overlay_at = None;
    assert!(
        matches!(
            compile(&req),
            Err(RenderError::UnplacedLayerClip { cut: 1, .. })
        ),
        "an overlay layer's clip must be placed or it consumes the programme lane"
    );
}

#[test]
fn an_overlay_clip_on_an_audio_track_is_refused() {
    let mut req = layered(Path::new("/bed.mkv"), Path::new("/broll.mkv"));
    req.tracks[0].clips = vec![0, 1];
    assert!(
        matches!(
            compile(&req),
            Err(RenderError::OverlayOnAudioTrack { cut: 1 })
        ),
        "an overlay renders no audio, so routing it to a bus would be a false promise"
    );
}

#[test]
fn two_keyframes_at_one_instant_are_refused() {
    let mut req = layered(Path::new("/bed.mkv"), Path::new("/broll.mkv"));
    req.layers[1].keyframes = vec![
        Keyframe {
            at: us(1_000_000),
            transform: Transform::default(),
            interpolate: Interpolation::Linear,
        },
        Keyframe {
            at: us(1_000_000),
            transform: Transform::default(),
            interpolate: Interpolation::Stepped,
        },
    ];
    assert!(matches!(
        compile(&req),
        Err(RenderError::DuplicateKeyframe { at: 1_000_000, .. })
    ));
}

// ── keyframes ───────────────────────────────────────────────────────────

/// The three instants every keyframe claim is checked at.
const SAMPLES: [i64; 3] = [500_000, 1_500_000, 2_500_000];

fn offset(x: i64) -> Transform {
    Transform {
        scale: None,
        offset_px: (x, 0),
        z: 10,
    }
}

/// A layer that slides from x=0 to x=64 across the output, linearly.
fn sliding_keyframes() -> Vec<Keyframe> {
    vec![
        Keyframe {
            at: us(0),
            transform: offset(0),
            interpolate: Interpolation::Linear,
        },
        Keyframe {
            at: us(1_500_000),
            transform: offset(32),
            interpolate: Interpolation::Linear,
        },
        Keyframe {
            at: us(3_000_000),
            transform: offset(64),
            interpolate: Interpolation::Linear,
        },
    ]
}

fn sliding_layer() -> PlanLayer {
    let mut req = layered(Path::new("/bed.mkv"), Path::new("/broll.mkv"));
    req.layers[1].keyframes = sliding_keyframes();
    compile(&req)
        .expect("a keyframed overlay compiles")
        .layers
        .into_iter()
        .find(|l| l.id == "broll")
        .expect("the overlay layer is in the plan")
}

#[test]
fn the_transform_is_resolved_at_three_sample_times() {
    let layer = sliding_layer();
    // 500 ms into a 0..1.5 s ramp to 32 is 10.67, truncated to 10 — the same
    // integer ffmpeg's `x` option takes.
    for (at, want) in SAMPLES.iter().zip([10, 32, 53]) {
        assert_eq!(
            layer.transform_at(us(*at)).offset_px.0,
            want,
            "the model must resolve the slide at {at} us"
        );
    }
}

/// The same slide, with the second keyframe reached as a step.
fn stepped_layer() -> PlanLayer {
    let mut req = layered(Path::new("/bed.mkv"), Path::new("/broll.mkv"));
    req.layers[1].keyframes = vec![
        Keyframe {
            at: us(0),
            transform: offset(0),
            interpolate: Interpolation::Linear,
        },
        Keyframe {
            at: us(1_500_000),
            transform: offset(64),
            interpolate: Interpolation::Stepped,
        },
    ];
    compile(&req)
        .expect("a stepped overlay compiles")
        .layers
        .into_iter()
        .find(|l| l.id == "broll")
        .expect("the overlay layer is in the plan")
}

#[test]
fn a_stepped_keyframe_holds_rather_than_ramping() {
    let layer = stepped_layer();
    assert_eq!(
        layer.transform_at(us(1_499_999)).offset_px.0,
        0,
        "a stepped keyframe holds the previous value right up to it"
    );
    assert_eq!(layer.transform_at(us(1_500_000)).offset_px.0, 64);
    assert_eq!(
        layer.transform_at(us(2_900_000)).offset_px.0,
        64,
        "and keeps holding after it"
    );
    let (x, _) = strivo_editor::render::layer_offset_exprs(&layer);
    assert!(
        !x.contains("*(t-"),
        "a stepped track must emit no ramp at all: {x}"
    );
}

#[test]
fn a_static_transform_is_the_answer_when_there_are_no_keyframes() {
    let layer = compile(&layered(Path::new("/bed.mkv"), Path::new("/broll.mkv")))
        .expect("compiles")
        .layers
        .into_iter()
        .find(|l| l.id == "broll")
        .expect("the overlay layer is in the plan");
    assert_eq!(layer.transform_at(us(0)).offset_px.0, 0);
    assert_eq!(layer.transform_at(us(2_900_000)).offset_px.0, 0);
}

#[test]
fn the_executor_emits_the_expression_the_model_resolves() {
    let layer = sliding_layer();
    let (x, y) = strivo_editor::render::layer_offset_exprs(&layer);
    assert!(
        x.contains("lt(t\\,1.500000)") && x.contains("32.0000") && x.contains("64.0000"),
        "the expression must carry both keyframe boundaries and values: {x}"
    );
    assert!(
        !y.contains("*(t-"),
        "an unanimated axis must emit no ramp: {y}"
    );
}

// ── the decoded artifact ────────────────────────────────────────────────

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("strivo-e08-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

/// The same request rendered through `ffv1`/`flac`, so a decoded-pixel
/// comparison measures the composite rather than the encoder.
fn lossless(mut req: RenderRequest) -> RenderRequest {
    req.container.video_codec = "ffv1".into();
    req.container.audio_codec = "flac".into();
    req
}

/// A solid-colour bed with a tone, so a pixel's identity is decisive.
fn red_bed(dir: &Path) -> PathBuf {
    let path = dir.join("red.mkv");
    let status = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=128x72:d=3:r=10",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=3",
        ])
        .args([
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(&path)
        .status()
        .expect("ffmpeg must be on PATH");
    assert!(status.success(), "failed to synthesize the red bed");
    path
}

/// `at` as ffmpeg's `-ss` spelling, in seconds.
fn at_secs(micros: i64) -> String {
    format!("{:.6}", micros as f64 / 1e6)
}

/// A solid 64x64 clip at 25 fps, all frames keyframes so a seek lands on the
/// frame asked for rather than on the nearest one.
fn overlay(dir: &Path, name: &str, size: &str, rate: &str, colour: &str, secs: u32) -> PathBuf {
    let path = dir.join(name);
    let source = format!("color=c={colour}:s={size}:d={secs}:r={rate}");
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
        ])
        .arg(&source)
        .args(["-pix_fmt", "yuv420p", "-c:v", "libx264", "-g", "1", "-an"])
        .arg(&path)
        .status()
        .expect("ffmpeg must be on PATH");
    assert!(status.success(), "failed to synthesize {source}");
    path
}

/// The horizontal span of columns that differ between two renders at one
/// instant, or `None` when nothing differs.
fn differing_span(plain: &Path, other: &Path, at: &str, width: usize) -> Option<(u32, u32)> {
    let read = |path: &Path| -> Vec<u8> {
        let out = Command::new("ffmpeg")
            .args(["-v", "error", "-ss", at, "-i"])
            .arg(path)
            .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .output()
            .expect("ffmpeg decode");
        assert!(
            out.status.success(),
            "decoding {} at {at} failed",
            path.display()
        );
        out.stdout
    };
    let (a, b) = (read(plain), read(other));
    differing_columns(&a, &b, width)
}

/// The first and last column of two equally-sized RGB frames that differ.
fn differing_columns(a: &[u8], b: &[u8], width: usize) -> Option<(u32, u32)> {
    assert_eq!(a.len(), b.len(), "frames must decode to the same geometry");
    let mut first: Option<u32> = None;
    let mut last = 0u32;
    for index in 0..a.len() / 3 {
        if b[index * 3..index * 3 + 3] != a[index * 3..index * 3 + 3] {
            let column = (index % width) as u32;
            first.get_or_insert(column);
            last = column;
        }
    }
    first.map(|start| (start, last + 1))
}

/// One decoded frame and the presentation time it actually carries.
///
/// The seek is a *request*, not a promise: a 25 fps output asked for 2.500 s
/// hands back the frame at 2.480 s. Reading the frame's own `pts_time` is what
/// makes the keyframe assertion exact instead of approximately right, and it is
/// the difference todo 43 named — addressing is exact, decode is keyframe-
/// accurate.
fn frame_at(path: &Path, seek: &str) -> (f64, Vec<u8>) {
    let out = Command::new("ffmpeg")
        // `-copyts` or the seek rebases the output timestamps to zero, and a
        // frame that reports pts 0 cannot be asked what the model says about
        // it.
        .args(["-v", "info", "-copyts", "-ss", seek, "-i"])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "showinfo",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ])
        .output()
        .expect("ffmpeg decode");
    assert!(
        out.status.success(),
        "decoding {} at {seek} failed",
        path.display()
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let pts = stderr
        .lines()
        .find_map(|line| line.split("pts_time:").nth(1))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or_else(|| panic!("no pts_time in showinfo output: {stderr}"));
    (pts, out.stdout)
}

/// Decoded 16-bit PCM for a file's first audio stream.
fn pcm(path: &Path) -> Vec<u8> {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:a", "-f", "s16le", "-acodec", "pcm_s16le", "-"])
        .output()
        .expect("ffmpeg decode");
    assert!(
        out.status.success(),
        "decoding the audio of {} failed",
        path.display()
    );
    out.stdout
}

/// A short FNV-1a digest, so a sample-comparison failure reports a number.
fn digest(samples: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in samples {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x} over {} bytes", samples.len())
}

fn peak(samples: &[u8]) -> u16 {
    let (pairs, _) = samples.as_chunks::<2>();
    pairs
        .iter()
        .map(|p| i16::from_le_bytes(*p).unsigned_abs())
        .max()
        .unwrap_or(0)
}

fn probe_fields(path: &Path, entries: &str) -> String {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            entries,
            "-of",
            "default=nw=1",
        ])
        .arg(path)
        .output()
        .expect("ffprobe must be on PATH");
    assert!(out.status.success(), "probing {} failed", path.display());
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn render(req: &RenderRequest, edl: &Edl, out: &Path) -> bool {
    export(req, edl, out, None).is_ok()
}

/// The acceptance criterion, read off decoded media: the B-roll is visible and
/// the **original** bed is audible.
#[test]
fn broll_renders_over_the_bed_and_the_bed_audio_is_untouched() {
    if !tool("ffmpeg") || !tool("ffprobe") {
        eprintln!("no media tool; the model assertions in this file still ran");
        return;
    }
    let dir = scratch("bed");
    let bed = bed_path();
    let broll = overlay(&dir, "broll.mkv", "64x64", "25", "blue", 2);
    let edl = locators(&bed, &broll);

    // A one-millisecond video fade on the programme clip, and nothing else. It
    // is here to put *both* renders on the transcode path: without it the
    // overlay-free render stream-copies the aac source while the layered one
    // re-encodes, and comparing those measures the encoder, not the edit. The
    // assertion below is what stops the precondition drifting away.
    let mut plain = without_overlay(&bed, &broll);
    plain.clips[0].fade_out = us(1_000);
    let mut over_request = layered(&bed, &broll);
    over_request.clips[0].fade_out = us(1_000);
    for request in [&plain, &over_request] {
        let plan = compile(request).expect("compiles");
        assert!(
            !is_passthrough(&plan, None),
            "a stream-copy render would be compared against a re-encode"
        );
    }

    let only_bed = dir.join("only-bed.mkv");
    assert!(render(&lossless(plain), &edl, &only_bed));
    let over = dir.join("over.mkv");
    assert!(render(&lossless(over_request), &edl, &over));

    let (bed_pcm, over_pcm) = (pcm(&only_bed), pcm(&over));
    assert_eq!(
        bed_pcm.len(),
        over_pcm.len(),
        "an overlay must not change the programme's length"
    );
    // Digests, not the buffers: a `Vec<u8>` equality failure prints half a
    // megabyte of samples and buries the assertion that matters.
    assert_eq!(
        digest(&bed_pcm),
        digest(&over_pcm),
        "the decoded audio must be sample-identical with and without the overlay"
    );
    assert!(
        peak(&over_pcm) > 1_000,
        "the bed must carry signal, or the equality above is satisfied by silence"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The B-roll is composited at exactly its placement and nowhere else.
#[test]
fn the_overlay_is_painted_exactly_where_it_was_placed() {
    if !tool("ffmpeg") || !tool("ffprobe") {
        eprintln!("no media tool; the model assertions in this file still ran");
        return;
    }
    let dir = scratch("place");
    let bed = red_bed(&dir);
    let broll = overlay(&dir, "broll.mkv", "64x64", "25", "blue", 2);
    let edl = locators(&bed, &broll);

    let only_bed = dir.join("only-bed.mkv");
    assert!(render(
        &lossless(without_overlay(&bed, &broll)),
        &edl,
        &only_bed
    ));
    let over = dir.join("over.mkv");
    assert!(render(&lossless(layered(&bed, &broll)), &edl, &over));

    // Placed 0.5 s in and 2 s long, so it is up at 1.0 s and gone by 2.8 s.
    assert_eq!(
        differing_span(&only_bed, &over, "1.0", 128),
        Some((0, 64)),
        "the B-roll must occupy exactly its 64 columns"
    );
    assert_eq!(
        differing_span(&only_bed, &over, "0.2", 128),
        None,
        "and nothing before its placement"
    );
    assert_eq!(
        differing_span(&only_bed, &over, "2.8", 128),
        None,
        "and nothing after its length"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// **QA failure scenario.** Muting the bed must change the audio and nothing
/// else, which is what audio/video independence has to mean for an insert.
#[test]
fn muting_the_bed_leaves_the_broll_layer_untouched() {
    if !tool("ffmpeg") || !tool("ffprobe") {
        eprintln!("no media tool; the model assertions in this file still ran");
        return;
    }
    let dir = scratch("mute");
    let bed = red_bed(&dir);
    let broll = overlay(&dir, "broll.mkv", "64x64", "25", "blue", 2);
    let edl = locators(&bed, &broll);

    let audible = dir.join("audible.mkv");
    assert!(render(&lossless(layered(&bed, &broll)), &edl, &audible));
    let mut muted = layered(&bed, &broll);
    muted.tracks[0].muted = true;
    let silent = dir.join("silent.mkv");
    assert!(
        render(&lossless(muted), &edl, &silent),
        "a mute is a render, not a refusal"
    );

    assert!(
        peak(&pcm(&audible)) > 1_000,
        "the unmuted render must carry signal, or the equality below is vacuous"
    );
    assert_eq!(
        peak(&pcm(&silent)),
        0,
        "a muted track must decode to digital silence"
    );
    for at in ["0.2", "1.0", "2.8"] {
        assert_eq!(
            differing_span(&audible, &silent, at, 128),
            None,
            "muting the bed must not move a single pixel at {at} s"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// A mixed-size, mixed-rate composition must render at the **declared** output
/// spec, not at whatever the first source happened to be.
#[test]
fn a_mixed_size_and_rate_composition_renders_at_the_declared_output_spec() {
    if !tool("ffmpeg") || !tool("ffprobe") {
        eprintln!("no media tool; the model assertions in this file still ran");
        return;
    }
    let dir = scratch("spec");
    let bed = bed_path();
    // The bed is 128x72 at 10 fps; this is 256x144 at 29.97 fps.
    let big = overlay(&dir, "big.mkv", "256x144", "30000/1001", "lime", 2);
    let mut req = layered(&bed, &big);
    req.clips[1].span = span(&big.display().to_string(), 0, 1_500_000);
    req.layers[1].transform = Transform {
        scale: Some((96, 54)),
        offset_px: (0, 0),
        z: 10,
    };
    req.video.frames = Some(Frames {
        width: 320,
        height: 180,
        fps_num: 24_000,
        fps_den: 1_001,
        pixel_format: "yuv420p".into(),
        timebase_num: 1,
        timebase_den: 1_000,
    });
    let mut edl = locators(&bed, &big);
    edl.cuts[1].end_sec = 1.5;

    let out = dir.join("spec.mkv");
    assert!(
        render(&req, &edl, &out),
        "the conformed render must succeed"
    );

    let fields = probe_fields(
        &out,
        "stream=codec_type,width,height,r_frame_rate,pix_fmt,time_base",
    );
    for want in [
        "width=320",
        "height=180",
        "r_frame_rate=24000/1001",
        "pix_fmt=yuv420p",
        "time_base=1/1000",
    ] {
        assert!(
            fields.contains(want),
            "declared spec missing {want}: {fields}"
        );
    }
    assert_eq!(
        fields.matches("codec_type=video").count(),
        1,
        "one video stream out: {fields}"
    );
    assert_eq!(
        fields.matches("codec_type=audio").count(),
        1,
        "the bed's second audio stream must not be carried over: {fields}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The keyframe claim, read off decoded pixels at the same three instants the
/// model test reads.
#[test]
fn the_keyframed_overlay_is_where_the_model_says_at_three_sample_times() {
    if !tool("ffmpeg") || !tool("ffprobe") {
        eprintln!("no media tool; the model assertions in this file still ran");
        return;
    }
    let dir = scratch("keys");
    // A solid bed, not the corpus fixture: "which columns differ" is only the
    // overlay's extent when the bed's own colours cannot match the overlay's.
    let bed = red_bed(&dir);
    // Three seconds, so the overlay is up for every sample instant including
    // the last: the placement window is half-open and the third sample is
    // otherwise past the end of a two-second overlay.
    let broll = overlay(&dir, "broll.mkv", "64x64", "25", "blue", 3);
    let mut edl = locators(&bed, &broll);
    edl.cuts[1].end_sec = 3.0;
    let mut without = without_overlay(&bed, &broll);
    without.clips[0].span = span(&bed.display().to_string(), 0, BED_SECS);

    let only_bed = dir.join("only-bed.mkv");
    assert!(render(&lossless(without), &edl, &only_bed));

    let mut keyed = layered(&bed, &broll);
    keyed.clips[1].overlay_at = Some(MediaTime::ZERO);
    keyed.clips[1].span = span(&broll.display().to_string(), 0, BED_SECS);
    keyed.layers[1].keyframes = sliding_keyframes();
    let out = dir.join("keyed.mkv");
    assert!(
        render(&lossless(keyed), &edl, &out),
        "the keyframed render must succeed"
    );

    let layer = sliding_layer();
    let mut checked = 0;
    for at in SAMPLES {
        let (plain_pts, plain_frame) = frame_at(&only_bed, &at_secs(at));
        let (over_pts, over_frame) = frame_at(&out, &at_secs(at));
        // The two files do not carry the same frame: a 25 fps overlay over a
        // 10 fps source lifts the composite to the faster rate, which is
        // report §4.5's "does not normalize every mixed source" complaint and is
        // what `VideoIntent::frames` exists to fix. It is sound to compare these
        // two frames anyway because the bed is a solid colour, so both depict
        // the same picture; what the assertion reads is the overlay's position
        // in the frame, at the instant the overlay render's own pts names.
        assert!(
            (plain_pts - over_pts).abs() < 0.05,
            "the decoded frames should be within a frame of each other, \
             got {plain_pts} and {over_pts}"
        );
        let want = layer.transform_at(us((over_pts * 1e6) as i64)).offset_px.0;
        let span = differing_columns(&plain_frame, &over_frame, 128)
            .unwrap_or_else(|| panic!("no B-roll at {over_pts} s"));
        // Centroid, not the first column. Both sources are `yuv420p`, so the
        // overlay's edge column is chroma-blended with the bed and can read as
        // equal to it; the measured left edge is therefore ±1, and a first-edge
        // comparison would be asserting chroma placement rather than geometry.
        // The centre of a 64 px span is unaffected by losing either edge.
        let centre = f64::from(span.0 + span.1) / 2.0;
        let expected = want as f64 + 31.5;
        assert!(
            (centre - expected).abs() <= 1.5,
            "asked for {at} us, the frame decoded at {over_pts} s: the model \
             centres the B-roll at {expected}, the pixels at {centre} (span {span:?})"
        );
        checked += 1;
    }
    assert_eq!(checked, 3, "three sample times must be checked, not fewer");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The legacy cut list still concatenates its B-roll: the store and the SPA
/// still read it, and a sequential insert is that format's own shape.
#[test]
fn the_legacy_cut_list_still_concatenates_its_broll() {
    let edl = Edl {
        recording_id: "r1".into(),
        cuts: vec![
            Cut {
                start_sec: 0.0,
                end_sec: 1.0,
                kind: CutKind::Source {
                    source_path: "src.mkv".into(),
                },
                fade_in_sec: 0.0,
                fade_out_sec: 0.0,
            },
            Cut {
                start_sec: 0.0,
                end_sec: 1.0,
                kind: CutKind::Broll {
                    broll_path: "broll.mkv".into(),
                },
                fade_in_sec: 0.0,
                fade_out_sec: 0.0,
            },
        ],
    };
    assert_eq!(edl.total_duration(), 2.0);
    assert_eq!(edl.cuts.len(), 2);
}
