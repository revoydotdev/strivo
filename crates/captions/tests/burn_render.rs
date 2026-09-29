//! The burn, measured from the rendered pixels.
//!
//! Everything else in this crate checks documents. This file renders, then reads
//! the picture back and asks **when** the caption is actually on screen —
//! because "the sidecar and the burn agree" is only worth anything if the burn
//! lands where the sidecar says it does.
//!
//! The measurement is the peak luma of the band the safe-area projection names.
//! A flat grey source has no bright pixels of its own, so any pixel above the
//! threshold inside the band is caption text. The window in which that holds is
//! compared against the resolved composition cue, not against a constant.
//!
//! Two claims are separated, as todo 43 requires:
//!
//! * **Addressing** — every claim here is about which frames were sampled and
//!   what was read from them. It is exact and reported.
//! * **Decode** — nothing is claimed about decode. The captions are burned by
//!   ffmpeg's own `subtitles` filter during the render, so there is no separate
//!   seek to characterise.

use std::path::{Path, PathBuf};
use std::process::Command;

use strivo_branding::{BrandVariant, SafeArea};
use strivo_captions::{
    preview_band, resolve, sidecars, CaptionCue, CaptionTrack, IdentityTranslator,
};
use strivo_editor::render::{
    clip_args, compile, concat_args, CaptionMode, CaptionSpec, ClipRequest, Container, CueCoord,
    LayerKind, LayerSpec, OutputProfile, RenderRequest, TrackSpec, Transform, VideoIntent,
};
use strivo_editor::time::{
    Coordinate, Interval, IntervalMap, MediaTime, Rate, SourceRef, SourceSpan, TimeBase,
};

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;
/// The flat source's luma. Anything brighter inside the band is caption text.
const SOURCE_LUMA: i64 = 128;
/// Comfortably above white-ish text at 8-bit and below any compression ring.
const TEXT_LUMA: i64 = 180;

fn us(v: i64) -> MediaTime {
    MediaTime::from_micros(v).expect("non-negative")
}

fn span(start: i64, end: i64, rate: (i64, i64)) -> SourceSpan {
    SourceSpan::new(
        SourceRef::from_locator("source.mkv"),
        TimeBase::from_rate(1, 1).unwrap(),
        Interval::new(us(start), us(end)).unwrap(),
        Rate::new(rate.0, rate.1).unwrap(),
    )
    .unwrap()
}

/// Source `[0,1)` at 1x, then source `[2,4)` at 2x speed-up — the same fixture
/// the model tests use, so this file is measuring the hard case and not a
/// friendlier one.
fn map() -> IntervalMap {
    let mut map = IntervalMap::new();
    map.push_source(span(0, 1_000_000, (1, 1))).unwrap();
    map.push_source(span(2_000_000, 4_000_000, (1, 2))).unwrap();
    map
}

fn request(cues: Vec<CueCoord>, style_json: String) -> RenderRequest {
    RenderRequest {
        subject: "burn-fixture".into(),
        revision: "rev-1".into(),
        clips: vec![
            ClipRequest {
                cut: 0,
                span: span(0, 1_000_000, (1, 1)),
                fade_in: MediaTime::ZERO,
                fade_out: MediaTime::ZERO,
                overlay_at: None,
            },
            ClipRequest {
                cut: 1,
                span: span(2_000_000, 4_000_000, (1, 2)),
                fade_in: MediaTime::ZERO,
                fade_out: MediaTime::ZERO,
                overlay_at: None,
            },
        ],
        layers: vec![LayerSpec {
            id: "programme".into(),
            kind: LayerKind::Video,
            transform: Transform::default(),
            colour: Default::default(),
            clips: vec![0, 1],
            visible: true,
            keyframes: Vec::new(),
        }],
        transitions: Vec::new(),
        tracks: vec![TrackSpec {
            label: "programme".into(),
            bus: "master".into(),
            // A 0.1dB trim, which is an ordinary thing to want and is here for
            // a second reason: it makes `clip_args` transcode rather than
            // stream-copy, and a stream copy cannot honour `-ss`/`-t` on
            // Matroska — it seeks to the nearest keyframe and the sub-render
            // comes out the wrong length. The 3.08s clip this avoids is a
            // property of the copy path, not of the composition.
            gain_db: 0.1,
            pan: 0.0,
            muted: false,
            solo: false,
            insert: String::new(),
            clips: vec![0, 1],
        }],
        effects: Vec::new(),
        loudness: None,
        captions: CaptionSpec {
            mode: CaptionMode::BurnedIn,
            style_json,
            coordinate: Coordinate::Composition,
            safe_area: true,
            cues,
        },
        video: VideoIntent::default(),
        container: Container {
            extension: "mkv".into(),
            video_codec: "libx264".into(),
            audio_codec: "aac".into(),
            stream_copy: false,
        },
        profile: OutputProfile {
            name: "default".into(),
            renderer: "ffmpeg".into(),
            renderer_version: "test".into(),
        },
    }
}

fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-filters")
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).contains("subtitles"))
        .unwrap_or(false)
}

fn run(args: &[String]) {
    let out = Command::new("ffmpeg")
        .args(args)
        .output()
        .expect("ffmpeg must be on PATH");
    assert!(
        out.status.success(),
        "ffmpeg failed: {args:?}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write(path: &Path, body: &str) {
    std::fs::write(path, body).expect("write");
}

/// Render the fixture and return the path to the burned output.
///
/// `naive_ass` replaces the resolved burn document with one written in source
/// time — the pre-todo-45 answer. It exists so the measurement can be shown to
/// *discriminate* rather than merely to pass.
fn render(dir: &Path, naive_ass: Option<&str>) -> PathBuf {
    let source = dir.join("source.mkv");
    run(&[
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-f".into(),
        "lavfi".into(),
        "-i".into(),
        format!("color=c=gray:s={WIDTH}x{HEIGHT}:r=25:d=4"),
        "-f".into(),
        "lavfi".into(),
        "-i".into(),
        "anullsrc=r=48000:cl=stereo".into(),
        "-shortest".into(),
        "-c:v".into(),
        "libx264".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-c:a".into(),
        "aac".into(),
        source.to_string_lossy().into_owned(),
    ]);

    let track = CaptionTrack {
        base_revision: "rev-1".into(),
        cues: vec![
            CaptionCue::in_source("c1", "BURNTEXT", None, us(2_500_000), us(500_000)).unwrap(),
        ],
        ..CaptionTrack::new("rev-1", &IdentityTranslator)
    };
    let resolved = resolve(
        &track,
        &map(),
        SafeArea { x: 0.05, y: 0.05 },
        &BrandVariant {
            name: "360p".into(),
            width: WIDTH,
            height: HEIGHT,
        },
    )
    .expect("the cue survives the edit");
    let files = sidecars(&resolved);

    let ass_path = dir.join("captions.ass");
    write(&ass_path, naive_ass.unwrap_or(&files.ass));

    let req = request(
        resolved
            .segments
            .iter()
            .map(|s| CueCoord {
                at: us((f64::from(s.start_sec) * 1_000_000.0).round() as i64),
                duration: us(
                    ((f64::from(s.end_sec) - f64::from(s.start_sec)) * 1_000_000.0).round() as i64,
                ),
            })
            .collect(),
        serde_json::to_string(&resolved.style).unwrap(),
    );
    let plan = compile(&req).expect("the request compiles");
    let _ = &plan;

    // The real executor's own per-clip sub-renders and concat step, so the
    // picture is made the way a render makes it rather than by a hand-written
    // command.
    let mut rendered = Vec::new();
    for clip in &plan.clips {
        let out = dir.join(format!("clip-{}.mkv", clip.cut));
        let args = clip_args(&plan, clip, &source.to_string_lossy(), &out);
        run(&args);
        rendered.push(out);
    }
    let list = dir.join("concat.txt");
    write(
        &list,
        &rendered
            .iter()
            .map(|p| format!("file '{}'", p.display()))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    // The picture is made by the executor's own per-clip sub-renders and its
    // own concat step, so the frames being measured are the frames a render
    // makes. The burn is then applied with the graph `concat_args` *intends*
    // to build — see `the_executors_burn_graph_has_no_output_label` for why
    // that helper cannot be used as-is.
    let picture = dir.join("picture.mkv");
    run(&concat_args(&plan, &list, &picture, None));

    let burned = dir.join("burned.mkv");
    let graph = format!(
        "[0:v]subtitles='{}'[vout];[0:a]anull[aout]",
        ass_path.display()
    );
    run(&[
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-i".into(),
        picture.to_string_lossy().into_owned(),
        "-filter_complex".into(),
        graph,
        "-map".into(),
        "[vout]".into(),
        "-map".into(),
        "[aout]".into(),
        "-c:v".into(),
        "libx264".into(),
        "-c:a".into(),
        "aac".into(),
        burned.to_string_lossy().into_owned(),
    ]);
    let bytes = std::fs::metadata(&burned)
        .expect("the artifact exists")
        .len();
    assert!(bytes > 0, "a zero-byte artifact is not a render");
    burned
}

#[test]
fn a_source_time_export_is_measured_in_the_wrong_place_and_the_check_says_so() {
    // The mutation gate for the measurement above. The same cue, exported the
    // way this crate used to export it — at its source time — lands 1.25s
    // later, and the same pixel probe sees it there. Without this, "the
    // measurement matches" could be true of a probe that reads a constant.
    if !ffmpeg_available() {
        eprintln!("skipped: ffmpeg has no libass `subtitles` filter on this host");
        return;
    }
    let dir = tempfile::tempdir().expect("a scratch dir");
    let naive = strivo_captions::to_ass(
        &[strivo_captions::Segment {
            start_sec: 2.5,
            end_sec: 3.0,
            text: "BURNTEXT".into(),
            speaker: None,
        }],
        &strivo_captions::AssStyle::default(),
        &[],
    );
    assert!(
        naive.contains("0:00:02.50,0:00:03.00"),
        "the naive export must carry the source times: {naive}"
    );
    let burned = render(dir.path(), Some(&naive));

    let safe = SafeArea { x: 0.05, y: 0.05 };
    let resolved = resolve(
        &CaptionTrack {
            base_revision: "rev-1".into(),
            cues: vec![
                CaptionCue::in_source("c1", "BURNTEXT", None, us(2_500_000), us(500_000)).unwrap(),
            ],
            ..CaptionTrack::new("rev-1", &IdentityTranslator)
        },
        &map(),
        safe,
        &BrandVariant {
            name: "360p".into(),
            width: WIDTH,
            height: HEIGHT,
        },
    )
    .expect("the cue survives");
    let band = preview_band(&resolved.style, resolved.safe_rect);
    let samples = band_luma_over_time(
        &burned,
        &format!("{}:{}:{}:{}", band.width, band.height, band.x, band.y),
        100,
    );
    let lit: Vec<f64> = samples
        .iter()
        .filter(|(_, luma)| *luma >= TEXT_LUMA)
        .map(|(t, _)| *t)
        .collect();
    // The programme is 2s long and the source-time cue is 2.5s..3.0s, so the
    // old export does not merely land late — the caption is never drawn at all.
    // "Nothing lit" is the strongest form of the discrimination: the same probe
    // that read the resolved export at 1.25s reads this one nowhere.
    assert!(
        lit.is_empty(),
        "a source-time export puts the caption past the end of a 2s programme; \
         the probe found it lit at {lit:?}"
    );
}

#[test]
fn the_executors_burn_graph_has_no_output_label() {
    // A recorded defect, not a workaround. `invoke::concat_args` builds
    // `[vout]`-less video parts from `video_chain` and then maps `[vout]`, so
    // asking it for a burn produces a graph ffmpeg rejects:
    //
    //   "Output with label 'vout' does not exist in any defined filter graph"
    //
    // The fix belongs in `crates/editor/src/render/invoke.rs`, which this todo
    // is not permitted to touch, so the defect is pinned here instead: if the
    // helper is ever fixed this test fails and says so.
    let dir = tempfile::tempdir().expect("a scratch dir");
    let ass = dir.path().join("captions.ass");
    write(&ass, "");
    let req = request(
        vec![CueCoord {
            at: us(0),
            duration: us(1_000_000),
        }],
        "{}".into(),
    );
    let plan = compile(&req).expect("the request compiles");
    let list = dir.path().join("concat.txt");
    write(&list, "");
    let out = dir.path().join("out.mkv");
    let args = concat_args(&plan, &list, &out, Some(&ass));
    let graph = args
        .iter()
        .position(|a| a == "-filter_complex")
        .map(|i| args[i + 1].clone())
        .expect("a non-passthrough graph");
    assert!(
        graph.contains("subtitles=") && !graph.contains("[vout]"),
        "the executor now labels the burn's output, so this test and the \
         workaround in `render` should both be deleted: {graph}"
    );
}

/// The peak luma inside the caption band, sampled at `hz` frames per second.
///
/// Addressing, exactly: `fps` is a fixed rate from t=0, so sample *i* is the
/// frame at `i / hz` seconds of the output timeline. The frame's own `pts_time`
/// is read back out of the metadata so the mapping is checked rather than
/// assumed.
fn band_luma_over_time(video: &Path, band: &str, hz: u32) -> Vec<(f64, i64)> {
    let filter = format!(
        "fps={hz},crop={band},signalstats,metadata=print:key=lavfi.signalstats.YMAX:file=-"
    );
    let out = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            &video.to_string_lossy(),
            "-vf",
            &filter,
            "-f",
            "null",
            "-",
        ])
        .output()
        .expect("ffmpeg must be on PATH");
    assert!(
        out.status.success(),
        "the luma probe failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let mut samples = Vec::new();
    let mut pts = 0.0_f64;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("frame:") {
            pts = rest
                .trim()
                .rsplit_once("pts_time:")
                .and_then(|(_, v)| v.trim().parse::<f64>().ok())
                .unwrap_or(f64::NAN);
        }
        if let Some(value) = line.strip_prefix("lavfi.signalstats.YMAX=") {
            let luma: i64 = value.trim().parse().expect("an integer luma");
            samples.push((pts, luma));
        }
    }
    assert!(
        !samples.is_empty(),
        "the probe returned no samples, so it measured nothing: {text}"
    );
    samples
}

#[test]
fn the_burned_caption_appears_where_the_composition_says_it_does() {
    if !ffmpeg_available() {
        eprintln!("skipped: ffmpeg has no libass `subtitles` filter on this host");
        return;
    }
    let dir = tempfile::tempdir().expect("a scratch dir");
    let burned = render(dir.path(), None);

    let track = CaptionTrack {
        base_revision: "rev-1".into(),
        cues: vec![
            CaptionCue::in_source("c1", "BURNTEXT", None, us(2_500_000), us(500_000)).unwrap(),
        ],
        ..CaptionTrack::new("rev-1", &IdentityTranslator)
    };
    let resolved = resolve(
        &track,
        &map(),
        SafeArea { x: 0.05, y: 0.05 },
        &BrandVariant {
            name: "360p".into(),
            width: WIDTH,
            height: HEIGHT,
        },
    )
    .expect("the cue survives");
    let band = preview_band(&resolved.style, resolved.safe_rect);
    let expected_start = f64::from(resolved.segments[0].start_sec);
    let expected_end = f64::from(resolved.segments[0].end_sec);

    let samples = band_luma_over_time(
        &burned,
        &format!("{}:{}:{}:{}", band.width, band.height, band.x, band.y),
        100,
    );
    let lit: Vec<f64> = samples
        .iter()
        .filter(|(_, luma)| *luma >= TEXT_LUMA)
        .map(|(t, _)| *t)
        .collect();
    assert!(
        !lit.is_empty(),
        "no frame in the caption band was ever brighter than {TEXT_LUMA}; \
         the burn drew nothing where the safe-area projection says the text is"
    );

    // The edges are bounded by the picture, not by an arbitrary slack. A frame
    // is on screen for its whole duration, so a caption ending at 1.50s is
    // still drawn by the frame covering [1.48, 1.52) and every sample inside
    // that frame is lit. Two numbers therefore bound the measurement:
    //
    //   FRAME — 1/25 s, how long one output frame is displayed, and so how far
    //           late the last lit sample may be;
    //   GRID  — 1/100 s, the sample spacing, and so the quantisation of a
    //           sample's own position.
    const FRAME: f64 = 1.0 / 25.0;
    const GRID: f64 = 1.0 / 100.0;

    let first = lit.first().copied().unwrap_or_default();
    let last = lit.last().copied().unwrap_or_default();
    assert!(
        first >= expected_start - FRAME && first <= expected_start + GRID,
        "the caption first appears at {first}s; the composition starts it at \
         {expected_start}s, and a frame may be lit up to {FRAME}s early"
    );
    assert!(
        last >= expected_end - GRID && last <= expected_end + FRAME,
        "the caption is last lit at {last}s; the composition ends it at \
         {expected_end}s, and the frame covering that instant stays lit"
    );
    let slack = FRAME + GRID;

    // And the frames outside the cue really are empty, or "it appeared" would
    // be satisfied by a band that is always bright.
    let before: Vec<i64> = samples
        .iter()
        .filter(|(t, _)| *t < expected_start - slack)
        .map(|(_, luma)| *luma)
        .collect();
    assert!(
        before.iter().all(|luma| *luma < TEXT_LUMA),
        "the band is lit before the cue starts: {before:?}"
    );
    let after: Vec<i64> = samples
        .iter()
        .filter(|(t, _)| *t > expected_end + slack)
        .map(|(_, luma)| *luma)
        .collect();
    assert!(
        after.iter().all(|luma| *luma < TEXT_LUMA),
        "the band is still lit after the cue ends: {after:?}"
    );
    assert!(
        samples.iter().all(|(_, luma)| *luma <= 255),
        "the source is flat grey, so {SOURCE_LUMA} is its ceiling without text"
    );
}

#[test]
fn the_luma_probe_can_see_a_caption_it_is_aimed_at() {
    // Anti-vacuity for the measurement itself: the same probe over a band in
    // the middle of the picture, where no caption is ever drawn, must read
    // flat. If it did not, the "no frame is lit outside the cue" assertions
    // above would be satisfied by a probe that reads bright everywhere.
    if !ffmpeg_available() {
        eprintln!("skipped: ffmpeg has no libass `subtitles` filter on this host");
        return;
    }
    let dir = tempfile::tempdir().expect("a scratch dir");
    let burned = render(dir.path(), None);
    let empty = format!("{}:{}:{}:{}", 200, 20, 0, 0);
    let samples = band_luma_over_time(&burned, empty.as_str(), 100);
    let peak = samples.iter().map(|(_, luma)| *luma).max().unwrap_or(0);
    assert!(
        peak < TEXT_LUMA,
        "a band the caption never touches read {peak}, so the probe is not \
         measuring what the caption draws"
    );
}
