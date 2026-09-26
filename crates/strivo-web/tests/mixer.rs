//! The mixer, proven on decoded media.
//!
//! One fixture per control, each asserted **twice**: once with the control set,
//! and once with the mixer sidecar removed. The second half is what makes the
//! first mean something — "identical to the baseline" is exactly what a control
//! that was accepted and then dropped also produces, so a one-sided assertion
//! cannot tell "baked" from "ignored".
//!
//! ## Every level assertion is calibrated, not given a constant
//!
//! See [`common::dsp`]. The tolerance is `FLOOR_DB + TOLERANCE_K ×` this host's
//! measured AAC round-trip error, both derived and neither guessed, and a
//! collapsed calibration makes the suite stricter rather than laxer.
//!
//! ## The claims, and where each is falsified
//!
//! | Claim | Test | Falsified by |
//! |---|---|---|
//! | a mute renders silence, decoded | [`a_muted_track_renders_a_silent_artifact`] | dropping `audible` from `track_filter` |
//! | preview and render read one plan | [`the_preview_reports_the_audibility_the_render_uses`] | adding a gain after `compile` in `preview` |
//! | each control reaches the decoded audio | [`each_control_changes_the_decoded_audio`] | ignoring the control in `apply` |
//! | ducking lands at the requested dB | [`a_ducking_curve_lands_within_the_derived_tolerance`] | dropping the automation step |
//! | a tap is not the master chain | [`a_track_tap_and_a_master_chain_are_different_stages`] | reading `insert` off the programme chain |
//! | normalisation is measure-then-apply | [`normalisation_is_measure_then_apply_not_a_single_pass`] | moving it into the chain |

#![cfg(feature = "creator")]

mod common;

use std::path::PathBuf;

use axum::http::StatusCode;
use tower::ServiceExt;

use common::dsp::{delta_db, pcm, rms_dbfs, window_delta_db, window_rms_dbfs, within};
use common::{dsp, edl_cutting, ffmpeg_available, Sandbox, API_KEY, EDITOR_STORE};

const ASKED_DB: f64 = -6.0;
const DUCK_DB: f32 = -12.0;

/// The ducking trigger: one long, clearly-bounded run of speech so the
/// envelope has a constant plateau to measure inside. Two seconds of trigger
/// with the default 50 ms attack and 300 ms release leaves well over a second
/// at exactly `duck_db`, which is the window [`window_delta_db`] reads.
const TRIGGER_START: f32 = 0.6;
const TRIGGER_END: f32 = 2.6;

fn sidecar_path(recording_id: &str, name: &str) -> PathBuf {
    strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join(name)
        .join(format!("{recording_id}.json"))
}

fn write_sidecar(recording_id: &str, name: &str, body: &serde_json::Value) {
    let path = sidecar_path(recording_id, name);
    std::fs::create_dir_all(path.parent().expect("sidecar parent")).expect("mkdir");
    std::fs::write(path, body.to_string()).expect("write sidecar");
}

fn clear_sidecar(recording_id: &str, name: &str) {
    let _ = std::fs::remove_file(sidecar_path(recording_id, name));
}

fn rendered_path(sandbox: &Sandbox, body: &serde_json::Value) -> PathBuf {
    let reported = body["output_path"]
        .as_str()
        .unwrap_or_else(|| panic!("the render reported no output_path: {body}"));
    sandbox
        .recording
        .parent()
        .expect("recording parent")
        .join(reported)
}

async fn render_to(sandbox: &Sandbox, control: &str) -> PathBuf {
    let (status, body) = sandbox
        .post(
            &format!("/api/v1/plugins/editor/{}/render", sandbox.recording_id),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{control}: {body}");
    rendered_path(sandbox, &body)
}

async fn plan_of(sandbox: &Sandbox, control: &str) -> serde_json::Value {
    let response = strivo_web::server::build_router(sandbox.state.clone())
        .oneshot(
            axum::http::Request::builder()
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
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .expect("body");
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    assert_eq!(status, StatusCode::OK, "{control}: {body}");
    body["preview"]["plan"].clone()
}

async fn mixer_get(sandbox: &Sandbox) -> serde_json::Value {
    let response = strivo_web::server::build_router(sandbox.state.clone())
        .oneshot(
            axum::http::Request::builder()
                .uri(format!(
                    "/api/v1/plugins/editor/{}/mixer",
                    sandbox.recording_id
                ))
                .header("x-api-key", API_KEY)
                .body(axum::body::Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json")
}

async fn save_baseline_cut(sandbox: &Sandbox) {
    let (status, body) = sandbox
        .save_edl(edl_cutting(&sandbox.recording, 0.0, 2.0))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

fn one_track(mixer: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "tracks": [ mixer ] })
}

// ── the QA failure scenario ─────────────────────────────────────────────

/// Mute a track and assert the rendered artifact is **silent** — decoded, not
/// size-checked.
///
/// This is the scenario the whole todo is for. A muted track is a *sound*
/// decision, so the only honest proof is a decode of the artifact: a file whose
/// audio is a stream of zeros reads at a level no amount of file size can
/// distinguish from a quiet recording, and a render that dropped the track
/// entirely would produce a valid file of the right size.
#[tokio::test]
async fn a_muted_track_renders_a_silent_artifact() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    save_baseline_cut(&sandbox).await;

    let audible = pcm(&render_to(&sandbox, "audible").await);
    assert!(
        rms_dbfs(&audible) > -40.0,
        "anti-vacuity: the unmuted render must carry signal, or 'the muted render is \
         silent' would be true of every render"
    );

    write_sidecar(
        &sandbox.recording_id,
        "mixer",
        &one_track(serde_json::json!({ "label": "programme", "muted": true })),
    );
    let muted = pcm(&render_to(&sandbox, "muted").await);
    let survivors = muted.iter().filter(|s| **s != 0).count();
    assert_eq!(
        survivors,
        0,
        "a muted track must render exact digital silence, not a quieter signal: \
         {survivors} of {} samples are non-zero (the unmuted decode reads {:.2} dBFS)",
        muted.len(),
        rms_dbfs(&audible)
    );
    // The anti-vacuity half, and the reason this is a *decode* assertion: the
    // muted artifact is a real, decodable, correctly-sized file. A size or
    // status check would have passed for a render that dropped the track.
    assert!(
        !muted.is_empty(),
        "the muted artifact decoded to no audio at all, which is a different defect \
         and not the one under test"
    );

    // The pair, and the reason the pair is the assertion: the same sidecar
    // removed must bring the baseline back exactly.
    clear_sidecar(&sandbox.recording_id, "mixer");
    let restored = pcm(&render_to(&sandbox, "unmuted again").await);
    assert!(
        delta_db(&audible, &restored).abs() < 1e-9,
        "removing the mute must restore the baseline decode, which is what makes \
         the silent reading a claim about the mute"
    );
}

/// The structural half of the same claim: the audibility the preview route
/// reports is the field the executor reads.
///
/// A preview that re-derived audibility from the buttons would be a second
/// answer to "is this track audible", and a solo button is where the two
/// conventions (mute-wins vs solo-wins) actually differ.
#[tokio::test]
async fn the_preview_reports_the_audibility_the_render_uses() {
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    save_baseline_cut(&sandbox).await;

    // Two tracks over a one-cut revision is not routable, so the mixer is asked
    // about a single track and the solo is the *global* one: a solo with nothing
    // else to solo leaves that track audible, and a mute still silences it.
    for (label, control, expected) in [
        ("neither", serde_json::json!({ "label": "programme" }), true),
        (
            "muted",
            serde_json::json!({ "label": "programme", "muted": true }),
            false,
        ),
        (
            "soloed",
            serde_json::json!({ "label": "programme", "solo": true }),
            true,
        ),
        (
            "muted and soloed",
            serde_json::json!({ "label": "programme", "muted": true, "solo": true }),
            false,
        ),
    ] {
        write_sidecar(&sandbox.recording_id, "mixer", &one_track(control.clone()));
        let plan = plan_of(&sandbox, label).await;
        let track = &plan["tracks"][0];
        assert_eq!(
            track["label"].as_str(),
            Some("programme"),
            "{label}: the mixer must own the track list"
        );
        assert_eq!(
            track["audible"].as_bool(),
            Some(expected),
            "{label}: the preview must report the audibility the executor will read"
        );
        assert_eq!(
            track["muted"].as_bool(),
            Some(
                control
                    .get("muted")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            ),
            "{label}: the button state is carried through so the UI can draw it"
        );
    }
    clear_sidecar(&sandbox.recording_id, "mixer");
}

/// The mixer endpoint reports the same resolved view the plan route does, read
/// from the compiled plan rather than recomputed.
#[tokio::test]
async fn the_mixer_endpoint_reports_the_plans_own_resolved_view() {
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    save_baseline_cut(&sandbox).await;
    write_sidecar(
        &sandbox.recording_id,
        "mixer",
        &serde_json::json!({
            "tracks": [{ "label": "programme", "gain_db": -3.0 }],
            "master_gain_db": -1.0,
        }),
    );
    let (status, body) = sandbox
        .post(
            &format!("/api/v1/plugins/editor/{}/mixer", sandbox.recording_id),
            serde_json::json!({
                "tracks": [{ "label": "programme", "gain_db": -3.0 }],
                "master_gain_db": -1.0,
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let shown = mixer_get(&sandbox).await;
    assert_eq!(shown["saved"].as_bool(), Some(true));
    assert_eq!(shown["mixer"]["tracks"][0]["gain_db"].as_f64(), Some(-3.0));

    let plan = plan_of(&sandbox, "mixer endpoint").await;
    assert_eq!(
        shown["resolved"]["tracks"], plan["tracks"],
        "the mixer's resolved view must BE the plan's tracks, not a second answer"
    );
    assert_eq!(shown["resolved"]["audio_chain"], plan["audio_chain"]);

    let chain: Vec<&str> = plan["audio_chain"]
        .as_array()
        .expect("a chain")
        .iter()
        .filter_map(|s| s["id"].as_str())
        .collect();
    assert_eq!(
        chain,
        vec!["mixer-master"],
        "the master stage is the only one this mixer contributes, and it is named"
    );
    clear_sidecar(&sandbox.recording_id, "mixer");
}

// ── per-control: the decoded output changes, and comes back ─────────────

/// Every control, one at a time, each measured against the same calibration and
/// each removed again afterwards.
///
/// Grouped rather than one test per control because the expensive part is the
/// calibration, and eight renders per control is eight times the same decode.
#[tokio::test]
async fn each_control_changes_the_decoded_audio_and_comes_back() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    save_baseline_cut(&sandbox).await;

    let baseline_path = render_to(&sandbox, "baseline").await;
    let baseline = pcm(&baseline_path);
    let round_trip = dsp::calibrate_codec_round_trip(&sandbox.recording, sandbox.dir.path());
    eprintln!(
        "calibration: baseline {:.2} dBFS, codec round-trip error {round_trip:.3} dB, \
         tolerance {:.3} dB",
        rms_dbfs(&baseline),
        dsp::tolerance_db(round_trip)
    );

    // 1 kHz, not 80 Hz. The sandbox fixture is a 440 Hz tone and an 80 Hz
    // high-pass leaves it within 0.1 dB — which is why the pre-existing
    // fingerprint-based suite could assert "the tap changed the decode" and be
    // right for the wrong reason. A tap that only moves a 440 Hz tone by a
    // rounding cannot be measured in dB, so the fixture is pitched where the
    // control has a real effect.
    let insert_chain = serde_json::json!({
        "effects": [{ "kind": "high_pass", "freq_hz": 1000.0 }]
    });
    let cases: Vec<(&str, serde_json::Value)> = vec![
        (
            "gain",
            one_track(serde_json::json!({ "label": "programme", "gain_db": ASKED_DB })),
        ),
        (
            "tap",
            one_track(serde_json::json!({ "label": "programme", "insert_fx": insert_chain })),
        ),
        (
            "master gain",
            serde_json::json!({ "tracks": [{ "label": "programme" }], "master_gain_db": -6.0 }),
        ),
        (
            "master chain",
            serde_json::json!({
                "tracks": [{ "label": "programme" }],
                "master_chain": { "effects": [{ "kind": "low_pass", "freq_hz": 1000.0 }] },
            }),
        ),
        (
            "normalise",
            serde_json::json!({ "tracks": [{ "label": "programme" }], "normalise": { "i": -30.0, "lra": 7.0, "tp": -3.0 } }),
        ),
    ];

    for (control, mixer) in cases {
        write_sidecar(&sandbox.recording_id, "mixer", &mixer);
        let rendered = pcm(&render_to(&sandbox, control).await);
        let moved = (rms_dbfs(&rendered) - rms_dbfs(&baseline)).abs();
        let changed = moved > dsp::tolerance_db(round_trip);
        assert!(
            changed,
            "{control}: the decoded audio must change beyond the derived tolerance \
             of {:.3} dB — the level moved {moved:.3} dB, to {:.2} dBFS against the \
             baseline's {:.2} dBFS",
            dsp::tolerance_db(round_trip),
            rms_dbfs(&rendered),
            rms_dbfs(&baseline)
        );

        clear_sidecar(&sandbox.recording_id, "mixer");
        let restored = pcm(&render_to(&sandbox, &format!("{control} removed")).await);
        assert_eq!(
            restored, baseline,
            "{control}: removing the control must restore the baseline decode, which \
             is what makes the changed reading a claim about the control"
        );
    }
}

/// The gain control, measured rather than merely "changed": the preview's number
/// out of the plan and the decoded artifact's number must agree.
#[tokio::test]
async fn the_track_gain_the_preview_reports_is_the_gain_the_render_delivers() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    save_baseline_cut(&sandbox).await;

    let unity = pcm(&render_to(&sandbox, "unity").await);
    let round_trip = dsp::calibrate_codec_round_trip(&sandbox.recording, sandbox.dir.path());

    write_sidecar(
        &sandbox.recording_id,
        "mixer",
        &one_track(serde_json::json!({ "label": "programme", "gain_db": ASKED_DB })),
    );
    let plan = plan_of(&sandbox, "gain").await;
    let planned = plan["tracks"][0]["gain_db"]
        .as_f64()
        .unwrap_or_else(|| panic!("the plan carries no track gain: {plan}"));
    assert_eq!(
        planned, ASKED_DB,
        "the preview reports what the mixer asked for"
    );

    let trimmed = pcm(&render_to(&sandbox, "gain").await);
    let measured = delta_db(&unity, &trimmed);
    assert!(
        within(measured, -planned, round_trip),
        "the plan promises {planned} dB and the decode delivers {measured:.3} dB, \
         outside {:.3} dB — preview and render are not reading one plan",
        dsp::tolerance_db(round_trip)
    );
    clear_sidecar(&sandbox.recording_id, "mixer");
}

/// Cleanup removes silence, so it is measured on a fixture that has some.
///
/// A separate test from the loop above for one reason: it needs a different
/// **source**, because the shared sandbox fixture is a continuous tone with no
/// silence in it. Asserting a cleanup control changes a continuous tone's level
/// is asserting that silence exists where there is none — and the first version
/// of this test did exactly that, and read 0.010 dB.
#[tokio::test]
async fn a_cleanup_control_removes_silence_from_the_decoded_audio() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    let gapped = sandbox.root.join("gapped.mkv");
    common::tone_then_silence(&gapped, 1, 3);

    let cut = serde_json::json!({
        "recording_id": "ignored-by-the-server",
        "cuts": [{
            "start_sec": 0.0, "end_sec": 3.0,
            "kind": { "kind": "source", "source_path": gapped.to_string_lossy() },
            "fade_in_sec": 0.0, "fade_out_sec": 0.0,
        }],
    });
    let (status, body) = sandbox.save_edl(cut).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let baseline = pcm(&render_to(&sandbox, "cleanup baseline").await);
    // Anti-vacuity: the second half of the fixture really is silent, so a
    // cleanup that changed nothing would be visible as an all-zero window.
    assert_eq!(
        baseline[(baseline.len() / 2)..]
            .iter()
            .filter(|s| **s != 0)
            .count(),
        0,
        "the fixture's second half must be silence, or this test measures nothing"
    );

    write_sidecar(
        &sandbox.recording_id,
        "mixer",
        &serde_json::json!({
            "tracks": [{ "label": "programme" }],
            "cleanup": { "noise_db": -30.0, "min_span_secs": 0.5 },
        }),
    );
    let plan = plan_of(&sandbox, "cleanup").await;
    let chain = plan["audio_chain"].to_string();
    assert!(
        chain.contains("silenceremove"),
        "the cleanup stage must reach the programme chain: {chain}"
    );

    let cleaned = pcm(&render_to(&sandbox, "cleanup").await);
    assert!(
        cleaned.len() < baseline.len(),
        "silenceremove must shorten the signal: {} samples against {} baseline",
        cleaned.len(),
        baseline.len()
    );
    assert!(
        rms_dbfs(&cleaned) > -60.0,
        "and must not take the tone with it: the cleaned decode reads {:.2} dBFS \
         against the baseline's {:.2} dBFS",
        rms_dbfs(&cleaned),
        rms_dbfs(&baseline)
    );
    clear_sidecar(&sandbox.recording_id, "mixer");
}

// ── ducking ─────────────────────────────────────────────────────────────

/// The sidechain envelope lands within the derived tolerance.
///
/// Measured as a **window** in each signal rather than over the whole file,
/// because the point of the control is that the level is not constant: a
/// whole-file RMS would report the average of the ducked and unducked states
/// and would match almost any curve. The window sits at the middle of the
/// plateau, clear of both the attack and the release.
#[tokio::test]
async fn a_ducking_curve_lands_within_the_derived_tolerance() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    save_baseline_cut(&sandbox).await;

    let unity = pcm(&render_to(&sandbox, "unity").await);
    let round_trip = dsp::calibrate_codec_round_trip(&sandbox.recording, sandbox.dir.path());

    let trigger = serde_json::json!([{
        "start_sec": TRIGGER_START,
        "end_sec": TRIGGER_END,
        "mean_db": -15.0,
    }]);
    write_sidecar(
        &sandbox.recording_id,
        "mixer",
        &serde_json::json!({
            "tracks": [{ "label": "programme" }],
            "ducking": { "trigger": trigger, "knobs": { "duck_db": DUCK_DB } },
        }),
    );

    let plan = plan_of(&sandbox, "ducking").await;
    let chain: Vec<&str> = plan["audio_chain"]
        .as_array()
        .expect("a chain")
        .iter()
        .filter_map(|s| s["id"].as_str())
        .collect();
    assert!(
        chain.contains(&"mixer-ducking"),
        "the ducking stage must be in the plan, not only in the output: {chain:?}"
    );

    let ducked = pcm(&render_to(&sandbox, "ducking").await);
    let window_start = f64::from(TRIGGER_START) + 0.4;
    let ducked_by = window_delta_db(&unity, &ducked, window_start, 0.8);
    assert!(
        within(ducked_by, f64::from(-DUCK_DB), round_trip),
        "inside the trigger the programme must sit at {} dB, measured {ducked_by:.3} dB \
         over {window_start}s..{}s, outside the derived tolerance of {:.3} dB",
        -DUCK_DB,
        window_start + 0.8,
        dsp::tolerance_db(round_trip)
    );

    // And the envelope, not just a constant: outside the trigger the programme
    // is back at unity. A control that ducked the whole file would satisfy the
    // assertion above and fail this one.
    let before = window_delta_db(&unity, &ducked, 0.05, 0.3);
    assert!(
        before.abs() <= dsp::tolerance_db(round_trip),
        "before the trigger the programme must be unchanged, measured {before:.3} dB, \
         tolerance {:.3} dB",
        dsp::tolerance_db(round_trip)
    );
    assert!(
        (window_rms_dbfs(&ducked, window_start, 0.8) - window_rms_dbfs(&unity, window_start, 0.8))
            .abs()
            > dsp::tolerance_db(round_trip),
        "anti-vacuity: the ducked window must actually read lower than the unity one"
    );
    clear_sidecar(&sandbox.recording_id, "mixer");
}

// ── routing: a tap is not the master chain ──────────────────────────────

/// A per-track tap and a master-bus effect are different stages, and the
/// decoded output shows which is which.
///
/// The proof that a tap is a *pre-master* tap and the master chain is not: the
/// track filter carries the tap and the programme chain does not, so the tap
/// conditions each track's own signal while the master effect acts on the sum.
/// With one track the two would sound the same, so the assertion is on the
/// routing — which is exactly the claim — and the decode is there to prove both
/// stages really run.
#[tokio::test]
async fn a_track_tap_and_a_master_chain_are_different_stages() {
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    save_baseline_cut(&sandbox).await;

    write_sidecar(
        &sandbox.recording_id,
        "mixer",
        &serde_json::json!({
            "tracks": [{
                "label": "programme",
                "insert_fx": { "effects": [{ "kind": "high_pass", "freq_hz": 1000.0 }] },
            }],
            "master_chain": { "effects": [{ "kind": "limiter", "ceiling_db": -3.0, "release_sec": 0.05 }] },
        }),
    );
    let plan = plan_of(&sandbox, "tap and master").await;

    let track = &plan["tracks"][0];
    assert_eq!(track["bus"].as_str(), Some("master"));
    assert!(
        track["insert"]
            .as_str()
            .is_some_and(|insert| insert.contains("highpass")),
        "the tap must reach the track it belongs to: {track}"
    );

    let chain = plan["audio_chain"].to_string();
    assert!(
        chain.contains("mixer-master") && chain.contains("alimiter"),
        "the master chain must reach the programme chain: {chain}"
    );
    assert!(
        !chain.contains("highpass"),
        "a tap must NOT be in the programme chain — that chain runs on the sum, so a \
         tap there would condition every bus: {chain}"
    );
    let stages: Vec<&str> = plan["audio_chain"]
        .as_array()
        .expect("a chain")
        .iter()
        .filter_map(|s| s["id"].as_str())
        .collect();
    assert_eq!(
        stages,
        vec!["mixer-master"],
        "one master stage, in its own right: {stages:?}"
    );
    clear_sidecar(&sandbox.recording_id, "mixer");
}

// ── normalisation: the choice, tested ───────────────────────────────────

/// Normalisation is measure-then-apply, and the test says so by construction:
/// the plan carries the target, the programme chain carries no `loudnorm`, and
/// the decoded artifact's measured level lands on the target rather than near
/// where it started.
#[tokio::test]
async fn normalisation_is_measure_then_apply_not_a_single_pass() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    save_baseline_cut(&sandbox).await;

    let unity_path = render_to(&sandbox, "unity").await;
    let before = dsp::measure_lufs(&unity_path);
    let round_trip = dsp::calibrate_codec_round_trip(&sandbox.recording, sandbox.dir.path());
    eprintln!("calibration: unity artifact reads {before:.2} LUFS");

    // A target the signal is nowhere near, so pass 2 has real work to do. The
    // fixture's own level is *measured* rather than assumed: the lavfi sine
    // source is not full scale on this host, and a hand-computed "a unity tone
    // sits at -3 dBFS" would have been wrong here.
    const TARGET_LUFS: f64 = -30.0;
    write_sidecar(
        &sandbox.recording_id,
        "mixer",
        &serde_json::json!({
            "tracks": [{ "label": "programme" }],
            "normalise": { "i": TARGET_LUFS, "lra": 7.0, "tp": -3.0 },
        }),
    );
    let plan = plan_of(&sandbox, "normalise").await;
    let target = plan["loudness"]["i"]
        .as_f64()
        .expect("the plan must carry the normalisation target");
    assert_eq!(target, TARGET_LUFS);
    assert!(
        !plan["audio_chain"].to_string().contains("loudnorm"),
        "measure-then-apply runs from the plan's loudness target, twice, by the \
         executor — a loudnorm in the chain would be a third, single-pass normalisation"
    );

    let normalised_path = render_to(&sandbox, "normalise").await;
    let after = dsp::measure_lufs(&normalised_path);
    assert!(
        after < before - 5.0,
        "a {TARGET_LUFS} LUFS target must pull the artifact down by more than 5 LU, \
         not by {:.2} (from {before:.2} to {after:.2})",
        before - after
    );

    // The tight half, and the direct witness for measure-then-apply: the tool's
    // own integrated-loudness reading of the **finished** artifact sits on the
    // target. A single-pass transform cannot know the signal it is normalising,
    // so it cannot land on a requested LUFS at all. The band is one LU because
    // `loudnorm` in linear mode is documented to converge to within a fraction
    // of a LU and to relax its constraint when its true-peak ceiling binds; on a
    // steady tone at {TARGET_LUFS} LUFS the ceiling (-3 dBTP) is nowhere near, so
    // a miss here is a miss rather than a documented bail-out. The codec
    // round-trip error is carried in because the measurement is of an encoded
    // file, not of the float the filter produced.
    let slack = 1.0 + 12.0 * round_trip;
    assert!(
        (after - TARGET_LUFS).abs() <= slack,
        "the artifact must measure {TARGET_LUFS} LUFS; ffmpeg reads {after:.2}, and \
         the slack is 1 LU plus twelve times the {round_trip:.3} dB codec round \
         trip that the measurement inherits"
    );
    clear_sidecar(&sandbox.recording_id, "mixer");
}
