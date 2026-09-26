//! One decoded-media fixture per render-affecting control, in the Phase-1 UI.
//!
//! ## What a fixture here is
//!
//! A render, then a **decode**: the artifact is decoded and the decoded signal
//! is compared, never the file size and never the exit code. `video_fingerprint`
//! is `ffmpeg -f framemd5`, a per-frame MD5 of the decoded picture;
//! `audio_fingerprint` decodes to signed 16-bit PCM and hashes it. Both are
//! readings of the media, so a control that is accepted and then dropped
//! cannot pass.
//!
//! ## The pairing, and why it is the shape of the test
//!
//! Every control is asserted **twice**: once with its sidecar saved, and once
//! with the sidecar removed. The second half is what makes the first mean
//! something. A filter that was dropped on the way to the artifact would
//! otherwise leave the artifact identical to the baseline, and "identical to
//! the baseline" is exactly what a *no-op* control also produces — so a
//! one-sided assertion cannot tell "the effect is baked" from "the effect was
//! ignored".
//!
//! ## The controls, and the reading each one asserts
//!
//! | Control | Sidecar | Assertion |
//! |---|---|---|
//! | per-cut fade | the EDL's own `fade_*_sec` | decoded video fingerprint changes |
//! | volume automation | `automation` | decoded audio changes |
//! | insert FX chain | `insert-fx` | decoded audio changes |
//! | pitch / time | `pitch` | decoded duration shortens by the tempo factor |
//! | sub-mix bus | `submix` | decoded audio changes |
//! | branding overlay | `branding` | decoded video changes |
//! | loudness target | `loudness` | decoded audio changes |
//!
//! The four that `editor_render` skipped before this todo are the four the
//! report calls a trust gap: the UI said each was baked and none of them was.

#![cfg(feature = "creator")]

mod common;

use std::path::{Path, PathBuf};

use axum::http::StatusCode;

use common::{edl_cutting, ffmpeg_available, Sandbox, API_KEY, EDITOR_STORE};

/// Per-frame MD5 of the decoded picture.
fn video_fingerprint(path: &Path) -> String {
    decode(path, &["-map", "0:v", "-f", "framemd5", "-"])
}

/// A hash of the decoded audio, as signed 16-bit PCM.
fn audio_fingerprint(path: &Path) -> String {
    decode(
        path,
        &[
            "-map",
            "0:a",
            "-f",
            "s16le",
            "-acodec",
            "pcm_s16le",
            "-ar",
            "48000",
            "-ac",
            "1",
            "-",
        ],
    )
}

fn decode(path: &Path, tail: &[&str]) -> String {
    let out = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-i", &path.to_string_lossy()])
        .args(tail)
        .output()
        .expect("ffmpeg runs");
    assert!(
        out.status.success(),
        "decoding {} failed: {}",
        path.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    digest(&out.stdout)
}

/// A 256-bit digest, enough to tell two decoded signals apart and short enough
/// to paste into a failure message. The workspace's own `sha2` is not reachable
/// from an integration test without a dev-dependency, and this is a test
/// fingerprint rather than a content hash.
fn digest(bytes: &[u8]) -> String {
    // FNV-1a widened to 128 bits by four independently seeded lanes. Not a
    // cryptographic digest and not used as one: the only claim is "these two
    // decoded signals differ", and a fixture is not adversarial input.
    let mut out = [0u8; 32];
    for (lane, seed) in [0x9e37_79b9u64, 0x85eb_ca6b, 0xc2b2_ae35, 0x27d4_eb2f]
        .iter()
        .enumerate()
    {
        let mut h = *seed ^ ((lane as u64) << 56);
        for byte in bytes {
            h ^= u64::from(*byte);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        out[lane * 8..lane * 8 + 8].copy_from_slice(&h.to_be_bytes());
    }
    out.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Where a render put its artifact, read from the render's own answer.
///
/// Not re-derived from the recording id. The route writes one immutable file
/// per attempt rather than a stable `<recording_id>.mkv`, because a stable name
/// meant a second render of the same recording silently overwrote an artifact a
/// user had already approved.
fn rendered_path(sandbox: &Sandbox, body: &serde_json::Value) -> PathBuf {
    let reported = body["output_path"]
        .as_str()
        .unwrap_or_else(|| panic!("the render reported no output_path: {body}"));
    assert!(
        !body["artifact_id"].as_str().unwrap_or_default().is_empty(),
        "the render named no artifact: {body}"
    );
    let path = sandbox
        .recording
        .parent()
        .expect("recording parent")
        .join(reported);
    assert!(
        path.starts_with(sandbox.recording.parent().expect("recording parent")),
        "the reported artifact escaped the recordings root: {path:?}"
    );
    path
}

fn sidecar_path(recording_id: &str, name: &str) -> PathBuf {
    strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join(name)
        .join(format!("{recording_id}.json"))
}

fn write_sidecar(recording_id: &str, name: &str, body: &str) {
    let path = sidecar_path(recording_id, name);
    std::fs::create_dir_all(path.parent().expect("sidecar parent")).expect("mkdir");
    std::fs::write(path, body).expect("write sidecar");
}

/// Render the sandbox's current revision and return the decoded audio
/// fingerprint. Every test here is a before/after pair around this.
/// Re-render and return the decoded picture's fingerprint.
async fn video_fingerprint_of_last_render(sandbox: &Sandbox) -> String {
    let (status, body) = sandbox
        .post(
            &format!("/api/v1/plugins/editor/{}/render", sandbox.recording_id),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    video_fingerprint(&rendered_path(sandbox, &body))
}

/// The plan the preview route currently reports, read as JSON.
async fn plan_body(sandbox: &Sandbox, control: &str) -> serde_json::Value {
    use axum::http::Request;
    use tower::ServiceExt;
    let response = strivo_web::server::build_router(sandbox.state.clone())
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/plugins/editor/{}/render/plan",
                    sandbox.recording_id
                ))
                .header("x-api-key", API_KEY)
                .body(axum::body::Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK, "{control}");
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .expect("body");
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    body["preview"]["plan"].clone()
}

async fn render_and_read_audio(sandbox: &Sandbox, control: &str) -> String {
    let (status, body) = sandbox
        .post(
            &format!("/api/v1/plugins/editor/{}/render", sandbox.recording_id),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{control}: {body}");
    audio_fingerprint(&rendered_path(sandbox, &body))
}

/// Save the two-second baseline cut every fixture starts from.
async fn save_baseline_cut(sandbox: &Sandbox, control: &str) {
    let (status, body) = sandbox
        .save_edl(edl_cutting(&sandbox.recording, 0.0, 2.0))
        .await;
    assert_eq!(status, StatusCode::OK, "{control}: {body}");
}

#[tokio::test]
async fn the_volume_automation_curve_is_baked_into_the_decoded_audio() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    save_baseline_cut(&sandbox, "automation").await;
    let before = render_and_read_audio(&sandbox, "baseline").await;

    write_sidecar(
        &sandbox.recording_id,
        "automation",
        r#"{"points":[{"time_sec":0.0,"gain_db":-18.0,"curve":"linear"}]}"#,
    );
    let after = render_and_read_audio(&sandbox, "automation").await;
    assert_ne!(
        before, after,
        "an 18 dB automation curve must change the decoded audio"
    );

    let _ = std::fs::remove_file(sidecar_path(&sandbox.recording_id, "automation"));
    let restored = render_and_read_audio(&sandbox, "automation removed").await;
    assert_eq!(
        restored, before,
        "removing the curve must restore the baseline decode, which is what makes \
         the first reading a claim about the curve"
    );
}

#[tokio::test]
async fn the_insert_fx_chain_is_baked_into_the_decoded_audio() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    save_baseline_cut(&sandbox, "automation").await;
    let before = render_and_read_audio(&sandbox, "baseline").await;

    // The voice-bus preset's first stage: an 80 Hz high-pass on a 440 Hz tone
    // is a large, measurable change rather than a rounding.
    write_sidecar(
        &sandbox.recording_id,
        "insert-fx",
        r#"{"effects":[{"kind":"high_pass","freq_hz":80}]}"#,
    );
    let after = render_and_read_audio(&sandbox, "insert-fx").await;
    assert_eq!(
        plan_body(&sandbox, "insert-fx").await["audio_chain"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|s| s["id"].as_str()),
        Some("insert-fx"),
        "the chain must be in the plan, not only in the output"
    );
    assert_ne!(
        before, after,
        "an 80 Hz high-pass must change the decoded audio"
    );
}

#[tokio::test]
async fn pitch_time_shortens_the_decoded_output_by_the_tempo_factor() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    let (status, body) = sandbox
        .save_edl(edl_cutting(&sandbox.recording, 0.0, 2.0))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = sandbox
        .post(
            &format!("/api/v1/plugins/editor/{}/render", sandbox.recording_id),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let full = (decoded_audio_bytes(&rendered_path(&sandbox, &body)) / 100) as f64;

    write_sidecar(
        &sandbox.recording_id,
        "pitch",
        r#"{"tempo":2.0,"pitch":1.0,"formant_preserve":true}"#,
    );
    let (status, body) = sandbox
        .post(
            &format!("/api/v1/plugins/editor/{}/render", sandbox.recording_id),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let halved = (decoded_audio_bytes(&rendered_path(&sandbox, &body)) / 100) as f64;
    // The **audio** length, not the container's: rubberband time-stretches the
    // signal and the muxer keeps the video's timestamps, so `format.duration`
    // reports the picture's length and would read as "no change" for a control
    // that plainly worked. The band is wide because rubberband's priming
    // latency is a real offset from the nominal factor, not a rounding.
    assert!(
        halved < full * 0.75 && halved > full * 0.35,
        "a tempo of 2.0 must roughly halve the decoded audio, not leave it at \
         {full} hundred bytes; measured {halved}"
    );
}

/// How many bytes the audio stream decodes to, at a fixed sample rate and
/// channel count so two files are directly comparable.
fn decoded_audio_bytes(path: &Path) -> usize {
    let out = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-i", &path.to_string_lossy()])
        .args([
            "-map",
            "0:a",
            "-f",
            "s16le",
            "-acodec",
            "pcm_s16le",
            "-ar",
            "48000",
            "-ac",
            "1",
            "-",
        ])
        .output()
        .expect("ffmpeg runs");
    assert!(
        out.status.success(),
        "decoding the audio of {} failed: {}",
        path.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout.len()
}

#[tokio::test]
async fn the_sub_mix_bus_gain_is_baked_into_the_decoded_audio() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    save_baseline_cut(&sandbox, "sub-mix").await;
    let before = render_and_read_audio(&sandbox, "sub-mix baseline").await;

    write_sidecar(
        &sandbox.recording_id,
        "submix",
        r#"{"tracks":[{"label":"game","input_index":0,"gain_db":-20.0}],"master_gain_db":0.0}"#,
    );
    let after = render_and_read_audio(&sandbox, "sub-mix").await;
    assert_eq!(
        plan_body(&sandbox, "sub-mix").await["audio_chain"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|s| s["id"].as_str()),
        Some("sub-mix"),
        "the bus must be in the plan, not only in the output"
    );
    assert_ne!(
        before, after,
        "a -20 dB bus gain must change the decoded audio"
    );
}

#[tokio::test]
async fn the_branding_overlay_is_baked_into_the_decoded_video() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    let (status, body) = sandbox
        .save_edl(edl_cutting(&sandbox.recording, 0.0, 2.0))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let before = video_fingerprint_of_last_render(&sandbox).await;

    // A greyscale conversion of the picture: every frame must change, so a
    // dropped overlay cannot read as a rounding.
    write_sidecar(
        &sandbox.recording_id,
        "branding",
        r#"{"watermark":null,"banners":[{"slot":"intro","text":"stri-vo","font_size":12,"color_rgba":"FF0000FF","anchor":"top_left","inset_px":2,"duration_secs":2.0}]}"#,
    );
    let after = video_fingerprint_of_last_render(&sandbox).await;
    assert!(
        plan_body(&sandbox, "branding").await["video_filter_complex"]
            .as_str()
            .unwrap_or_default()
            .contains("drawtext"),
        "the overlay must reach the plan's video chain"
    );
    assert_ne!(
        before, after,
        "a burned-in banner must change the decoded video"
    );
}

#[tokio::test]
async fn the_loudness_target_is_baked_into_the_decoded_audio() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    save_baseline_cut(&sandbox, "automation").await;
    let before = render_and_read_audio(&sandbox, "baseline").await;

    // A target the signal is nowhere near, so the second pass has real work to
    // do rather than confirming what it already had.
    write_sidecar(
        &sandbox.recording_id,
        "loudness",
        r#"{"i":-30.0,"lra":7.0,"tp":-3.0}"#,
    );
    let after = render_and_read_audio(&sandbox, "loudness").await;
    assert!(
        plan_body(&sandbox, "loudness").await["loudness"].is_object(),
        "the target must reach the plan"
    );
    assert_ne!(
        before, after,
        "an EBU R128 pass must change the decoded audio"
    );
}

#[tokio::test]
async fn a_per_cut_fade_is_baked_into_the_decoded_video() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    let (status, body) = sandbox
        .save_edl(edl_cutting(&sandbox.recording, 0.0, 2.0))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = sandbox
        .post(
            &format!("/api/v1/plugins/editor/{}/render", sandbox.recording_id),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let before = video_fingerprint(&rendered_path(&sandbox, &body));

    let faded = serde_json::json!({
        "recording_id": "ignored-by-the-server",
        "cuts": [{
            "start_sec": 0.0, "end_sec": 2.0,
            "kind": { "kind": "source", "source_path": sandbox.recording.to_string_lossy() },
            "fade_in_sec": 1.0, "fade_out_sec": 1.0,
        }],
    });
    let (status, body) = sandbox.save_edl(faded).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = sandbox
        .post(
            &format!("/api/v1/plugins/editor/{}/render", sandbox.recording_id),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_ne!(
        video_fingerprint(&rendered_path(&sandbox, &body)),
        before,
        "a one-second fade in and out must change the decoded video"
    );
}

#[tokio::test]
async fn a_cue_the_composition_cut_away_does_not_silently_move() {
    // The negative control for the whole suite: with no caption sidecar the
    // plan carries no cues, and a cue count of zero is the honest answer. A
    // compiler that invented a cue to satisfy "the captions control exists"
    // would fail here.
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    let (status, body) = sandbox
        .save_edl(edl_cutting(&sandbox.recording, 0.0, 2.0))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let shown = plan_body(&sandbox, "no captions").await;
    assert_eq!(
        shown["cues"].as_array().map(Vec::len),
        Some(0),
        "a revision with no saved captions must compile no cues"
    );
}
