//! What the plan asks ffmpeg to do.
//!
//! Asserted on the argument builders rather than on a rendered file, because
//! the claim under test is "a filter is never dropped onto the copy path" — a
//! statement about the argv. A file-size comparison would pass on a render
//! that dropped it.

use super::fixture::*;
use super::*;

#[test]
fn the_concat_step_stream_copies_only_when_there_is_nothing_to_do() {
    let mut bare = request();
    bare.effects.clear();
    bare.loudness = None;
    bare.video.filter_complex = "[0:v]copy[vout]".into();
    let plan = preview(&bare).expect("preview compiles");
    assert!(
        is_passthrough(&plan.plan, None),
        "no filters and no loudness means the copy path is legal"
    );
    let mut graphic = bare.clone();
    graphic.video.filter_complex = "[0:v]eq=brightness=0.0[vout]".into();
    assert!(
        !is_passthrough(&preview(&graphic).expect("preview compiles").plan, None),
        "a real video graph cannot stream-copy"
    );
    let mut filtered = bare.clone();
    filtered.effects.push(effect(
        "automation",
        EffectKind::VolumeAutomation,
        "volume=1.20dB",
    ));
    let shown = preview(&filtered).expect("preview compiles");
    assert!(
        !is_passthrough(&shown.plan, None),
        "a filter on the copy path would be silently dropped"
    );
}

#[test]
fn the_concat_step_transcodes_when_a_clip_has_a_track_gain() {
    let mut req = request();
    req.effects.clear();
    req.loudness = None;
    req.video.filter_complex = "[0:v]copy[vout]".into();
    req.tracks[0].gain_db = -3.0;
    let plan = preview(&req).expect("preview compiles");
    assert!(
        !is_passthrough(&plan.plan, None),
        "a per-track gain is applied on the sub-render, so the concat must transcode"
    );
    assert_eq!(track_filter(&plan.plan, 0), "volume=-3.00dB");
    assert_eq!(track_filter(&plan.plan, 1), "volume=-3.00dB");
}

#[test]
fn a_burned_in_caption_over_a_video_graph_is_refused_not_mis_wired() {
    let Some((mut req, revision, dir)) = on_fixture("burn-over") else {
        let mut req = request();
        req.captions.mode = CaptionMode::BurnedIn;
        req.captions.style_json = "{}".into();
        assert!(matches!(
            export(
                &req,
                &edl(),
                std::env::temp_dir().join("x.mkv").as_path(),
                None
            ),
            Err(RenderError::BurnOverVideoGraph) | Err(RenderError::BurnWithoutSubtitle)
        ));
        return;
    };
    req.captions.mode = CaptionMode::BurnedIn;
    req.captions.style_json = "{}".into();
    let subtitle = dir.join("captions.ass");
    std::fs::write(&subtitle, "[Script Info]\nScriptType: v4.00+\n").expect("write subtitle");
    let out = dir.join("out.mkv");
    let refusal = export(&req, &revision, &out, Some(&subtitle)).expect_err("no label to thread");
    assert!(
        matches!(refusal, RenderError::BurnOverVideoGraph),
        "expected the video-graph refusal, got {refusal:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_muted_track_is_a_stated_silence_not_a_skipped_filter() {
    let mut req = request();
    req.tracks[0].muted = true;
    let plan = preview(&req).expect("preview compiles");
    // A linear zero, not `-inf`: ffmpeg's volume filter turns `-inf` dB into a
    // non-finite multiplier and the AAC encoder refuses the stream, so the
    // `-inf` spelling made a mute *unrenderable*. Found by decoding the
    // artifact, not by reading this string.
    assert_eq!(track_filter(&plan.plan, 0), "volume=0");
}

#[test]
fn a_soloed_track_silences_the_rest_even_when_the_buttons_disagree() {
    // The executor's whole dependence on the compiler's resolved `audible`:
    // swap the two buttons' relationship and the filter follows the resolution,
    // not the buttons.
    let mut req = request();
    req.tracks[0].solo = true;
    let plan = preview(&req).expect("preview compiles");
    assert!(
        plan.plan.tracks[0].audible,
        "the only track is the soloed one, so it stays audible"
    );
    assert!(
        plan.plan.tracks.iter().all(|t| t.audible),
        "with one track there is nothing for the solo to silence"
    );
}

#[test]
fn export_leaves_no_scratch_directory_behind() {
    let Some((req, revision, dir)) = on_fixture("cleanup") else {
        return;
    };
    let out = dir.join("nested").join("out.mkv");
    let _ = export(&req, &revision, &out, None);
    let leftovers: Vec<String> = std::fs::read_dir(out.parent().expect("output parent"))
        .expect("output parent exists")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".plan-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "scratch directories left behind: {leftovers:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
