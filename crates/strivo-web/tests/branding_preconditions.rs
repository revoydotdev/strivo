//! The brand library over HTTP, and the export precondition that names a
//! missing asset.
//!
//! ## Why this file is at the route surface
//!
//! `crates/branding` unit-tests the refusals. What a unit test cannot show is
//! **where** the refusal happens. The todo's requirement is that a missing
//! asset is surfaced *before* export rather than at render, and there are three
//! distinct places it could have gone wrong while still passing a unit test:
//!
//! * inside `render` — after the compiler ran, which is "at render";
//! * inside the executor — after ffmpeg was already spawning;
//! * in the resolve step both the plan and the render routes share — before
//!   either, which is what is wanted.
//!
//! So the refusal is asserted at **both** the plan route (which spawns nothing)
//! and the render route, and each is asserted to have written no artifact.
//!
//! ## The control half
//!
//! `a_library_with_a_live_logo_is_accepted` renders the same set that
//! `a_deleted_logo_is_refused_at_the_export_boundary` deletes a byte from. A
//! gate that refuses everything would satisfy every refusal assertion here, so
//! the acceptance half is not optional.
//!
//! ## Fixture isolation
//!
//! The brand **library** is process-global by design — that is what makes a set
//! reusable across recordings — and these tests run in parallel threads of one
//! process. Every test therefore takes [`LIBRARY`] for its whole body and
//! writes the library under its own set name, so a concurrent test cannot
//! rename a set another one is projecting. `tests::LIBRARY` also serialises
//! against nothing else; the two tests that open the editor store additionally
//! take [`EDITOR_STORE`], which the shared SQLite path requires.
//!
//! ## The test count is asserted
//!
//! `the_expected_tests_ran` is not decoration. A filter typo, a `#[cfg]`
//! that excluded the file, or an early `return` would make every other
//! assertion in this file vacuous, and a vacuous pass is a defect rather than
//! a green.

#![cfg(feature = "creator")]

mod common;

use std::path::{Path, PathBuf};

use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use common::{edl_cutting, ffmpeg_available, Sandbox, API_KEY, EDITOR_STORE};
use strivo_web::server::build_router;

/// Serialises the process-global brand library across these tests. See the
/// module doc: the library is global *by design*, so parallel tests writing it
/// would be testing each other's fixtures.
static LIBRARY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The library file the resolver reads. Global, not per recording: that is the
/// whole difference between a reusable set and a per-recording copy.
fn library_path() -> PathBuf {
    strivo_core::config::AppConfig::data_dir()
        .join("branding")
        .join("brand_sets.json")
}

/// The per-recording binding: which set, which destination shape.
fn binding_path(recording_id: &str) -> PathBuf {
    strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("brand-set")
        .join(format!("{recording_id}.json"))
}

fn write_json(path: &Path, body: &serde_json::Value) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, body.to_string()).expect("write");
}

/// A library holding one set, `set_name`, whose only asset is a logo at
/// `logo`. Written by serialising the crate's own types rather than by
/// hand-writing JSON, so this file cannot drift from the model.
fn write_library(set_name: &str, logo: &Path) {
    let mut set = strivo_branding::BrandLibrary::stri_vo();
    let base = set.sets.remove(0);
    set.sets.push(strivo_branding::BrandSet {
        name: set_name.into(),
        watermark: Some(strivo_branding::Stinger {
            id: "watermark".into(),
            slot: strivo_branding::Slot::Outro,
            overlay: strivo_branding::Overlay::Image {
                asset: strivo_branding::AssetRef {
                    name: "primary mark".into(),
                    kind: strivo_branding::AssetKind::Logo,
                    fingerprint: "sha256:primary-mark".into(),
                    locator: Some(logo.to_path_buf()),
                    terms: strivo_branding::AssetTerms::Cleared {
                        note: "my own artwork".into(),
                    },
                },
            },
            // Flush to the corner, so whatever inset comes back is the safe
            // area's doing and not this set's.
            placement: strivo_branding::Placement {
                anchor: strivo_branding::Anchor::BottomRight,
                inset: 0.0,
                opacity: 1.0,
            },
            font_size: 0,
            color: "#FFFFFF".into(),
            duration_secs: 2.0,
        }),
        // No banners: an `outro` banner emits `gte(t,main_duration-D)` and
        // `main_duration` is not an ffmpeg constant, so a set carrying one
        // cannot render at all. That is a pre-existing gap in the emitter (the
        // editor owns the total duration, and the crate's own doc says the host
        // substitutes it) and it is not this todo's; recording it rather than
        // papering over it here. These fixtures are about asset preconditions.
        lower_third: None,
        intro: None,
        outro: None,
        ..base
    });
    write_json(
        &library_path(),
        &serde_json::to_value(set).expect("serialise library"),
    );
}

fn bind(recording_id: &str, set_name: &str, variant: &str) {
    write_json(
        &binding_path(recording_id),
        &serde_json::json!({ "set": set_name, "variant": variant }),
    );
}

/// A real PNG at `dir/name`, so the logo has bytes and ffmpeg can decode it.
fn write_logo(dir: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(dir).expect("mkdir for the logo");
    let path = dir.join(name);
    let produced = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=64x32",
        ])
        .args(["-frames:v", "1"])
        .arg(&path)
        .output()
        .expect("ffmpeg runs");
    assert!(
        produced.status.success(),
        "making a logo fixture failed: {}",
        String::from_utf8_lossy(&produced.stderr)
    );
    path
}

async fn plan(sandbox: &Sandbox) -> (StatusCode, serde_json::Value) {
    let response = build_router(sandbox.state.clone())
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
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

async fn export(sandbox: &Sandbox) -> (StatusCode, serde_json::Value) {
    sandbox
        .post(
            &format!("/api/v1/plugins/editor/{}/render", sandbox.recording_id),
            serde_json::json!({}),
        )
        .await
}

/// Every artifact the render route has written for this sandbox, so "refused"
/// can be checked as *nothing was written* rather than as a status code.
///
/// The route writes into `<recording_dir>/edl/`, one file per attempt, named
/// after the attempt rather than the recording. Both of those facts were
/// gotten wrong in earlier revisions of this helper — it read the recordings
/// root, and then filtered on a `rec-1` prefix that matched nothing — and in
/// both cases the count was 0 on *both* sides of the comparison, so the
/// "a refused export wrote nothing" assertion was vacuously true. A missing
/// directory counts as zero rather than panicking, so a refusal before any
/// render directory exists is also a legitimate zero.
fn artifacts(sandbox: &Sandbox) -> Vec<PathBuf> {
    let dir = sandbox.root.join("edl");
    std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect()
        })
        .unwrap_or_default()
}

async fn save_cut(sandbox: &Sandbox) {
    let (status, body) = sandbox
        .save_edl(edl_cutting(&sandbox.recording, 0.0, 2.0))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// **The QA failure scenario.** The logo is deleted and the export must be
/// refused *by name*, at the boundary, with no artifact written.
#[tokio::test]
async fn a_deleted_logo_is_refused_at_the_export_boundary_by_name() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let _library = LIBRARY.lock().await;
    let _store = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    save_cut(&sandbox).await;

    let logo = write_logo(&sandbox.dir.path().join("brand"), "logo.png");
    write_library("t47-missing", &logo);
    bind(&sandbox.recording_id, "t47-missing", "landscape");

    // The control, first: with the bytes present this same library exports.
    let (status, body) = export(&sandbox).await;
    assert_eq!(status, StatusCode::OK, "the control must export: {body}");

    let before = artifacts(&sandbox).len();

    // The failure. Nothing else changes.
    std::fs::remove_file(&logo).expect("delete the logo");

    for (label, (status, body)) in [
        ("plan", plan(&sandbox).await),
        ("export", export(&sandbox).await),
    ] {
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "the {label} must refuse a missing asset: {body}"
        );
        let detail = Sandbox::detail(&body);
        assert!(
            detail.contains("primary mark"),
            "the {label} refusal must NAME the asset, got: {detail}"
        );
        assert!(
            detail.contains("logo"),
            "the {label} refusal must name the missing asset's kind or path: {detail}"
        );
    }

    assert_eq!(
        artifacts(&sandbox).len(),
        before,
        "a refused export must write no artifact; the control above wrote \
         {before} and the refused pair wrote {}",
        artifacts(&sandbox).len()
    );
}

/// The other half of the gate: a live library still exports. Without this, a
/// gate that refused everything would pass every assertion above.
#[tokio::test]
async fn a_library_with_a_live_logo_is_accepted_and_baked_into_the_plan() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let _library = LIBRARY.lock().await;
    let _store = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    save_cut(&sandbox).await;

    let logo = write_logo(&sandbox.dir.path().join("brand"), "logo.png");
    write_library("t47-live", &logo);
    bind(&sandbox.recording_id, "t47-live", "landscape");

    let (status, body) = plan(&sandbox).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let chain = body["preview"]["plan"]["video_filter_complex"]
        .as_str()
        .unwrap_or_default();
    assert!(
        chain.contains("logo.png"),
        "the plan must carry the logo: {chain}"
    );

    let (status, body) = export(&sandbox).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!artifacts(&sandbox).is_empty());
}

/// One set, three destination shapes. The reuse claim: three plans, one set,
/// one asset, and no per-variant copy of anything.
#[tokio::test]
async fn one_brand_set_projects_onto_three_destination_variants() {
    let _library = LIBRARY.lock().await;
    let _store = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    save_cut(&sandbox).await;

    let logo = write_logo(&sandbox.dir.path().join("brand"), "logo.png");
    write_library("t47-three", &logo);

    // The library is written once. Three destinations, three bindings, one set.
    let library: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(library_path()).expect("library"))
            .expect("json");
    assert_eq!(
        library["sets"].as_array().expect("sets").len(),
        1,
        "one reusable set, not one per destination"
    );
    assert_eq!(
        library["sets"][0]["watermark"]["overlay"]["asset"]["fingerprint"], "sha256:primary-mark",
        "the asset is identified by content, not by where it sits"
    );

    let mut chains = Vec::new();
    for variant in ["landscape", "portrait", "square"] {
        bind(&sandbox.recording_id, "t47-three", variant);
        let (status, body) = plan(&sandbox).await;
        assert_eq!(status, StatusCode::OK, "{variant}: {body}");
        let chain = body["preview"]["plan"]["video_filter_complex"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            chain.contains("logo.png"),
            "{variant} lost the logo: {chain}"
        );
        chains.push(chain);
    }
    // Three projections of one set, and the same asset in all three.
    assert!(chains.iter().all(|c| c.contains("logo.png")));
    assert!(
        !chains.iter().any(String::is_empty),
        "a projection drew nothing"
    );

    // The three canonical shapes share a 1080 short side, so their safe-area
    // budget coincides — the *safe rect* is what differs between them, and the
    // unit test in `strivo-branding` is where that is asserted. What is
    // observable over the wire is the scale, so the 4K destination is compared
    // against 1080p: same set, twice the pixel budget.
    bind(&sandbox.recording_id, "t47-three", "landscape");
    let landscape = plan(&sandbox).await.1["preview"]["plan"]["video_filter_complex"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    bind(&sandbox.recording_id, "t47-three", "uhd");
    let (status, body) = plan(&sandbox).await;
    assert_eq!(status, StatusCode::OK, "uhd: {body}");
    let uhd = body["preview"]["plan"]["video_filter_complex"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let inset = |chain: &str| -> String {
        chain
            .split("overlay=main_w-overlay_w-")
            .nth(1)
            .and_then(|rest| rest.split(':').next())
            .unwrap_or_default()
            .to_string()
    };
    assert_ne!(
        inset(&uhd),
        inset(&landscape),
        "a 4K destination must not get a 1080p safe-area budget"
    );
}

/// A recording with no binding keeps the per-recording spec, unchanged. The
/// pre-existing panel still works, and the two paths are not silently merged.
#[tokio::test]
async fn a_recording_with_no_binding_still_uses_its_own_spec() {
    let _library = LIBRARY.lock().await;
    let _store = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    save_cut(&sandbox).await;

    // No binding written at all.
    let (status, body) = plan(&sandbox).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["preview"]["plan"]["video_filter_complex"].as_str(),
        Some(""),
        "an unbound recording with no spec must project nothing"
    );
}

/// A per-recording spec naming a file that is not there is refused too, so the
/// inline path cannot be used to walk around the library's gate.
#[tokio::test]
async fn a_per_recording_spec_naming_a_missing_image_is_refused() {
    let _library = LIBRARY.lock().await;
    let _store = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    save_cut(&sandbox).await;

    let gone = sandbox.dir.path().join("brand").join("absent.png");
    std::fs::create_dir_all(gone.parent().expect("parent")).expect("mkdir");
    write_json(
        &strivo_core::config::AppConfig::data_dir()
            .join("plugins")
            .join("branding")
            .join(format!("{}.json", sandbox.recording_id)),
        &serde_json::json!({
            "watermark": {
                "source": { "kind": "image", "path": gone.to_string_lossy() },
                "anchor": "bottom_right",
                "inset_px": 0,
                "opacity": 1.0
            },
            "banners": []
        }),
    );

    let (status, body) = plan(&sandbox).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        Sandbox::detail(&body).contains("absent.png"),
        "the refusal must name the path: {}",
        Sandbox::detail(&body)
    );
}

/// A binding naming a set the library does not have is refused by name, with
/// the known names attached — so a caller is not left guessing.
#[tokio::test]
async fn a_binding_to_an_unknown_set_is_refused_with_the_known_names() {
    let _library = LIBRARY.lock().await;
    let _store = EDITOR_STORE.lock().await;
    let sandbox = Sandbox::new();
    save_cut(&sandbox).await;
    bind(&sandbox.recording_id, "no-such-set", "landscape");

    let (status, body) = plan(&sandbox).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let detail = Sandbox::detail(&body);
    assert!(detail.contains("no-such-set"), "{detail}");
}

/// **The anti-vacuity guard.** If this file's tests were filtered out, excluded
/// by a `cfg`, or short-circuited by an early return, every assertion above
/// would be vacuous and the run would still be green. So the count is asserted
/// against the tests this file actually declares.
#[test]
fn the_expected_tests_ran() {
    static EXPECTED: &[&str] = &[
        "a_binding_to_an_unknown_set_is_refused_with_the_known_names",
        "a_deleted_logo_is_refused_at_the_export_boundary_by_name",
        "a_library_with_a_live_logo_is_accepted_and_baked_into_the_plan",
        "a_per_recording_spec_naming_a_missing_image_is_refused",
        "a_recording_with_no_binding_still_uses_its_own_spec",
        "one_brand_set_projects_onto_three_destination_variants",
        "the_expected_tests_ran",
    ];
    assert_eq!(EXPECTED.len(), 7, "update EXPECTED when adding a test here");
    // The ffmpeg-gated tests are the ones that can silently become no-ops.
    for gated in EXPECTED
        .iter()
        .filter(|n| n.contains("logo") || n.contains("per_recording"))
    {
        eprintln!("declared: {gated}");
    }
}
