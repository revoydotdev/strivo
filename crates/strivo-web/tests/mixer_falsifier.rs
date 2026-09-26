//! Falsification harness for todo 44's central claim.
//!
//! The claim: **preview and render agree because they are the same plan**, not
//! because two code paths were written to match. This binary exists to be
//! *broken on purpose* and to be observed red — it is the teeth-check for every
//! gain agreement assertion in the mixer suite, which reuses its helpers
//! verbatim rather than restating them.
//!
//! ## The control it drives, and the probe that had to be thrown away first
//!
//! A **per-track gain**, read out of `plan.tracks[0].gain_db` and compared to
//! the decoded artifact. The first version of this probe drove the sub-mix bus
//! gain instead and read the dB back out of the plan's `SubMix` chain step. It
//! was green before the falsification and **stayed green through it**: the
//! planted fudge moved `plan.tracks[].gain_db`, a field that probe never read,
//! so a preview could disagree with the render by a whole dB while the probe
//! reported agreement. The lesson is the one this file exists to record — a
//! preview/render agreement probe must read *the field the control writes*, and
//! "it passed" is only evidence once the same mutation has been watched to turn
//! it red.
//!
//! The bus-gain probe is kept in `mixer.rs`, where it is paired with the field
//! it actually reads.
//!
//! ## The planted defect, and why it is the right one
//!
//! A preview fudge factor. It is the commonest way two render paths come to
//! disagree, it is plausible enough that it ships ("the preview is a touch
//! louder so it reads on a laptop speaker"), and **no structural probe catches
//! it**: both paths still call the same compiler, the plan is still the same
//! value, and the divergence is one `+ 1.0` added to a gain after the plan has
//! been built. That is exactly why the claim needs a *measured* assertion and
//! not only a type.
//!
//! ## Running it
//!
//! ```text
//! cargo test -p strivo-web --features creator --test mixer_falsifier
//! # green against the real code
//! # inject the fudge into crates/strivo-web/src/render.rs::preview
//! # red, with the dB gap in the failure message
//! # revert
//! ```

#![cfg(feature = "creator")]

mod common;

use axum::http::StatusCode;

use common::dsp::{calibrate_codec_round_trip, delta_db, pcm, rms_dbfs, tolerance_db, within};
use common::{edl_cutting, ffmpeg_available, Sandbox, API_KEY, EDITOR_STORE};

const ASKED_DB: f64 = -6.0;

fn sidecar_path(recording_id: &str, name: &str) -> std::path::PathBuf {
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

fn rendered_path(sandbox: &Sandbox, body: &serde_json::Value) -> std::path::PathBuf {
    let reported = body["output_path"]
        .as_str()
        .unwrap_or_else(|| panic!("the render reported no output_path: {body}"));
    sandbox
        .recording
        .parent()
        .expect("recording parent")
        .join(reported)
}

async fn render_to(sandbox: &Sandbox, control: &str) -> std::path::PathBuf {
    let (status, body) = sandbox
        .post(
            &format!("/api/v1/plugins/editor/{}/render", sandbox.recording_id),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{control}: {body}");
    rendered_path(sandbox, &body)
}

/// The plan the **preview** route reports.
///
/// This is the preview's half of the claim: every number the mixer UI draws
/// comes from this value, and the render is measured against it.
pub async fn plan_body(sandbox: &Sandbox, control: &str) -> serde_json::Value {
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

/// The dB the plan says the programme track is at.
///
/// The field the mixer control writes, read straight out of the plan the
/// preview route returned — which is the only place a preview may learn a gain
/// from, and the field the executor then runs.
fn planned_track_gain_db(plan: &serde_json::Value) -> f64 {
    plan["tracks"][0]["gain_db"]
        .as_f64()
        .unwrap_or_else(|| panic!("the plan carries no track gain: {plan}"))
}

/// The load-bearing claim, in its positive form: the gain the preview reports
/// out of the plan is the gain the render delivers into the decoded artifact,
/// to within a tolerance derived from this host's own codec round-trip error.
#[tokio::test]
async fn preview_and_render_agree_on_the_track_gain_within_the_derived_tolerance() {
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

    let unity = pcm(&render_to(&sandbox, "unity").await);
    let round_trip = calibrate_codec_round_trip(&sandbox.recording, sandbox.dir.path());
    eprintln!(
        "calibration: unity level {:.2} dBFS, codec round-trip error {round_trip:.3} dB, \
         tolerance {:.3} dB",
        rms_dbfs(&unity),
        tolerance_db(round_trip)
    );

    write_sidecar(
        &sandbox.recording_id,
        "mixer",
        &serde_json::json!({
            "tracks": [{
                "label": "programme",
                "bus": "master",
                "gain_db": ASKED_DB,
                "pan": 0.0,
                "muted": false,
                "solo": false,
            }]
        })
        .to_string(),
    );

    let planned = planned_track_gain_db(&plan_body(&sandbox, "track gain").await);
    assert_eq!(
        planned, ASKED_DB,
        "the preview must report the gain the request asked for"
    );

    let trimmed = pcm(&render_to(&sandbox, "track gain").await);
    let measured = delta_db(&unity, &trimmed);

    assert!(
        within(measured, -planned, round_trip),
        "the plan promises {planned} dB and the decoded artifact delivers {measured:.3} dB, \
         outside the derived tolerance of {:.3} dB (calibration error {round_trip:.3} dB) — \
         preview and render are NOT reading one plan",
        tolerance_db(round_trip)
    );

    let _ = std::fs::remove_file(sidecar_path(&sandbox.recording_id, "mixer"));
}
