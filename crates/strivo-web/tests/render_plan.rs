//! Two routes, one plan: the convergence proof at the HTTP surface.
//!
//! The editor's own contract test drives [`strivo_editor::render::preview`]
//! and [`strivo_editor::render::export`] directly. This file drives the two
//! **routes** that wrap them, because a route is a distinct caller in its own
//! right: it loads the revision from the store, resolves the six sidecars, and
//! hands the request on. A bug that made only the route diverge — a sidecar
//! read in one and not the other, a plan projected for the response and not
//! for the render — would be invisible to the unit-level test.
//!
//! ## Non-emptiness first
//!
//! `plan_is_non_empty_before_the_two_routes_agree` is a separate test that
//! runs **before** the comparison in file order, and the comparison itself
//! asserts non-emptiness first. Two empty plans are equal, so an
//! equality-only assertion would be a green gate over a compiler that emits
//! nothing.

#![cfg(feature = "creator")]

mod common;

use std::path::PathBuf;

use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use common::{edl_cutting, ffmpeg_available, video_with_audio, Sandbox, API_KEY, EDITOR_STORE};
use strivo_web::server::build_router;

fn plan_route(recording_id: &str) -> String {
    format!("/api/v1/plugins/editor/{recording_id}/render/plan")
}

fn render_route(recording_id: &str) -> String {
    format!("/api/v1/plugins/editor/{recording_id}/render")
}

async fn get(state: &strivo_web::server::AppState, path: &str) -> (StatusCode, serde_json::Value) {
    let router = build_router(state.clone());
    let request = Request::builder()
        .uri(path)
        .header("x-api-key", API_KEY)
        .body(axum::body::Body::empty())
        .expect("request");
    let response = router.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

/// Where the render route writes, derived the same way the route derives it.
/// Where a render put its artifact, read from the render's own answer.
///
/// Not re-derived from the recording id: the route writes one immutable file
/// per attempt, because a stable `<recording_id>.mkv` meant a second render
/// silently overwrote an artifact a user had already approved.
fn rendered_path(sandbox: &Sandbox, body: &serde_json::Value) -> PathBuf {
    let reported = body["output_path"]
        .as_str()
        .unwrap_or_else(|| panic!("the render reported no output_path: {body}"));
    assert!(
        !body["artifact_id"].as_str().unwrap_or_default().is_empty(),
        "the render named no artifact: {body}"
    );
    sandbox
        .recording
        .parent()
        .expect("recording parent")
        .join(reported)
}

/// Write one of the six per-recording sidecars the resolver reads.
fn sidecar(recording_id: &str, name: &str, body: &str) {
    let path = strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join(name)
        .join(format!("{recording_id}.json"));
    std::fs::create_dir_all(path.parent().expect("sidecar parent")).expect("mkdir");
    std::fs::write(path, body).expect("write sidecar");
}

#[tokio::test]
async fn the_plan_route_compiles_a_non_empty_plan() {
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    let (status, _) = sandbox
        .save_edl(edl_cutting(&sandbox.recording, 0.0, 2.0))
        .await;
    assert_eq!(status, StatusCode::OK, "the save is the fixture");

    let (status, body) = get(&sandbox.state, &plan_route(&sandbox.recording_id)).await;
    assert_eq!(status, StatusCode::OK, "plan route: {body}");
    let plan = &body["preview"]["plan"];
    assert_eq!(
        plan["clips"].as_array().map(Vec::len),
        Some(1),
        "one cut must be placed: {plan}"
    );
    assert_eq!(
        plan["output_duration"], 2_000_000,
        "2s of source is 2s of output, in whole microseconds"
    );
    assert_eq!(
        plan["revision"],
        format!("{}:1", sandbox.recording_id),
        "the plan must name the revision it came from"
    );
    assert!(
        body["preview"]["rendered_preview"]
            .as_str()
            .unwrap_or_default()
            .ends_with("/render"),
        "a preview must offer the rendered preview: {body}"
    );
    assert!(
        body["preview"]["plan"]["profile"]["renderer"] == "ffmpeg",
        "the plan records the tool that will run it: {body}"
    );
}

#[tokio::test]
async fn the_two_routes_compile_one_plan() {
    if !ffmpeg_available() {
        // Asserting the render route answered without a media tool would be
        // asserting nothing. The plan route is covered on its own above.
        return;
    }
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    let (status, _) = sandbox
        .save_edl(edl_cutting(&sandbox.recording, 0.0, 2.0))
        .await;
    assert_eq!(status, StatusCode::OK);

    // All four sidecars `editor_render` used to skip, so the convergence is
    // asserted on a revision that actually has the effects the UI promises.
    sidecar(
        &sandbox.recording_id,
        "insert-fx",
        r#"{"effects":[{"kind":"high_pass","freq_hz":80}]}"#,
    );
    sidecar(
        &sandbox.recording_id,
        "pitch",
        r#"{"tempo":1.0,"pitch":1.0,"formant_preserve":true}"#,
    );
    sidecar(
        &sandbox.recording_id,
        "submix",
        r#"{"tracks":[{"label":"game","input_index":0,"gain_db":-1.5}],"master_gain_db":0.0}"#,
    );
    sidecar(
        &sandbox.recording_id,
        "automation",
        r#"{"points":[{"time_sec":0.0,"gain_db":2.0,"curve":"linear"}]}"#,
    );

    let (preview_status, preview_body) =
        get(&sandbox.state, &plan_route(&sandbox.recording_id)).await;
    assert_eq!(preview_status, StatusCode::OK, "{preview_body}");

    let (render_status, render_body) = sandbox
        .post(&render_route(&sandbox.recording_id), serde_json::json!({}))
        .await;
    assert_eq!(render_status, StatusCode::OK, "{render_body}");

    // Non-emptiness FIRST: two empty plans are equal.
    let shown = &preview_body["preview"]["plan"];
    let done = &render_body["plan"];
    let work = [
        "clips",
        "layers",
        "transitions",
        "tracks",
        "audio_chain",
        "cues",
    ]
    .iter()
    .filter_map(|k| shown[*k].as_array().map(Vec::len))
    .sum::<usize>();
    assert!(
        work >= 4,
        "the plan carries {work} entries; a thinner plan makes the equality below pass for free"
    );
    assert_eq!(
        shown["audio_chain"].as_array().map(Vec::len),
        Some(3),
        "the insert chain, the sub-mix bus and the automation curve all reach the plan: {shown}"
    );
    assert_eq!(
        done, shown,
        "the preview route and the render route must compile one plan from one revision"
    );
    assert!(
        rendered_path(&sandbox, &render_body).exists(),
        "the render route must produce the artifact it planned"
    );
}

#[tokio::test]
async fn a_sidecar_that_does_not_parse_is_refused_rather_than_rendered_without() {
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    let (status, _) = sandbox
        .save_edl(edl_cutting(&sandbox.recording, 0.0, 1.0))
        .await;
    assert_eq!(status, StatusCode::OK);
    sidecar(&sandbox.recording_id, "insert-fx", "{not json");

    let (status, body) = get(&sandbox.state, &plan_route(&sandbox.recording_id)).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an unparseable sidecar must be reported, not rendered without: {body}"
    );
    assert!(
        Sandbox::detail(&body).contains("insert-fx"),
        "the refusal must name the sidecar: {body}"
    );
}

#[tokio::test]
async fn the_plan_is_admissible_by_the_execution_boundarys_rules() {
    // The relation: what the compiler places must be exactly what
    // `media_input::admit_edl` admits. If the compiler could emit a plan whose
    // implied cuts the boundary rejects, the two would have drifted, and the
    // drift would only show up as a render that fails after the user waited
    // for it.
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    let (status, _) = sandbox
        .save_edl(edl_cutting(&sandbox.recording, 0.0, 2.0))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = get(&sandbox.state, &plan_route(&sandbox.recording_id)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let edl = strivo_editor::store::EdlStore::open(&common::editor_store_path())
        .expect("store")
        .current(&sandbox.recording_id)
        .expect("read")
        .expect("a saved revision")
        .edl;
    let admitted = admit(&sandbox, &edl).await;
    assert!(
        admitted.is_ok(),
        "a plan the compiler produced must be admissible at the boundary: {admitted:?}"
    );

    // And the compiler's own answer agrees with the boundary's integer output
    // length, which is the one number both must produce.
    let plan_output = body["preview"]["plan"]["output_duration"]
        .as_i64()
        .expect("an integer output duration");
    assert_eq!(
        plan_output,
        admitted.expect("admitted").output.micros(),
        "the plan and the boundary must agree on the output length, in whole microseconds"
    );

    // The negative half: a cut past the end of its source is refused by the
    // boundary, so it must never reach the compiler.
    let over = edl_cutting(&sandbox.recording, 0.0, 99.0);
    let (status, body) = sandbox.save_edl(over).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "the boundary refuses an out-of-range cut before compilation: {body}"
    );
    assert!(
        Sandbox::detail(&body).contains("cut-out-of-range"),
        "the typed code must be the boundary's own: {body}"
    );
}

#[tokio::test]
async fn a_cut_outside_the_media_root_never_reaches_the_compiler() {
    let _guard = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    let outside = sandbox.dir.path().join("outside.mkv");
    video_with_audio(&outside, 2);
    let (status, body) = sandbox.save_edl(edl_cutting(&outside, 0.0, 1.0)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        Sandbox::detail(&body).contains("outside-root"),
        "the typed code must be the boundary's own: {body}"
    );
}

/// Stage 1 + stage 2 + stage 3 of todo 25's gate, driven by hand.
///
/// A `block_on` here would deadlock: `#[tokio::test]` runs a current-thread
/// runtime, so blocking its only worker on a future that needs that same
/// worker to drive a child process waits forever. Every stage is therefore
/// awaited.
async fn admit(
    sandbox: &Sandbox,
    edl: &strivo_editor::Edl,
) -> Result<strivo_web::routes::media_input::AdmittedEdl, String> {
    use strivo_web::routes::media_input::{admit_edl, admit_path, probe_admitted, AdmittedMedia};
    let mut admitted = std::collections::HashMap::new();
    for cut in &edl.cuts {
        let raw = match &cut.kind {
            strivo_editor::CutKind::Source { source_path } => source_path.clone(),
            strivo_editor::CutKind::Broll { broll_path } => broll_path.clone(),
        };
        let real = admit_path(Some(&sandbox.root), &raw).map_err(|e| e.to_string())?;
        let bytes = std::fs::metadata(&real).map(|m| m.len()).unwrap_or(0);
        let probe = probe_admitted(&real, bytes)
            .await
            .map_err(|e| e.to_string())?;
        admitted.insert(
            real,
            AdmittedMedia {
                path: probe.path,
                bytes: probe.bytes,
                duration: probe.duration,
            },
        );
    }
    admit_edl(&sandbox.root, edl, &admitted).map_err(|e| e.to_string())
}
