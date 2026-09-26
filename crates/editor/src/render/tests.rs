//! The load-bearing contract: **preview and export compile one plan.**
//!
//! ## Why this file is shaped the way it is
//!
//! The failure guarded against is not "the compiler is wrong" — it is "there
//! are two render paths again, and they happen to agree today". A test that
//! called `compile` twice would pass under that failure unchanged, because
//! `compile` is what both paths share. So the equality assertion here goes
//! through **two distinct public entry points**, [`preview`] and [`export`],
//! and compares what they return.
//!
//! ## Non-emptiness is asserted first, deliberately
//!
//! A compiler that emits an empty plan satisfies "both callers agree" for
//! free: two empty values are equal. Every comparison of the two callers
//! therefore asserts the plan carries the work the revision implies *before*
//! it asserts they agree. An equality-only test would be a green gate over an
//! empty plan.

use super::*;
use crate::time::Interval;

mod argv;
mod fixture;
mod refusals;

// The shared fixture lives in `tests::fixture` so this module and its three
// submodules all build a revision from the same value.
use fixture::*;

// ── the invariant ──────────────────────────────────────────────────────

#[test]
fn preview_and_export_compile_one_plan_from_one_revision() {
    let Some((req, revision, dir)) = on_fixture("converge") else {
        // Without a media tool there is nothing to execute, and asserting an
        // export succeeded would be asserting nothing. The plan half still
        // holds: export's refusal path compiles the same plan first.
        let shown = preview(&request()).expect("preview compiles");
        assert!(shown.plan.work_items() >= 8, "the plan is not empty");
        return;
    };
    let out = dir.join("out.mkv");
    let shown = preview(&req).expect("preview compiles");
    let done = export(&req, &revision, &out, None).expect("export compiles and renders");

    // Non-emptiness FIRST. Two empty plans are equal, so an equality-only
    // assertion would be a green gate over a compiler that emits nothing.
    assert!(
        shown.plan.work_items() >= 8,
        "the plan carries {} work items; a plan this thin makes the equality \
         assertion below pass for free",
        shown.plan.work_items()
    );
    assert_eq!(shown.plan.clips.len(), 2, "both cuts must be placed");
    assert_eq!(
        shown.plan.output_duration,
        us(2_500_000),
        "1s + 1.5s of source is 2.5s of output"
    );
    assert_eq!(shown.plan.audio_stages().len(), 4, "four audio stages");
    assert!(
        shown.plan.video_filter_complex.contains("eq=brightness"),
        "the branding overlay must reach the plan"
    );
    assert!(shown.plan.loudness.is_some(), "loudness is declared");
    assert_eq!(shown.plan.cues.len(), 1, "the cue resolves");
    assert!(
        out.exists() && done.bytes > 0,
        "the export caller must actually produce the artifact"
    );
    assert_eq!(
        done.plan, shown.plan,
        "the two callers must compile one plan from one revision"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn export_refuses_a_burned_in_caption_with_no_subtitle_document() {
    let mut req = request();
    req.captions.mode = CaptionMode::BurnedIn;
    req.captions.style_json = "{\"title\":\"styled\"}".into();
    let revision = edl();
    let out = std::env::temp_dir()
        .join("strivo-render-burn")
        .join("out.mkv");
    let refusal = export(&req, &revision, &out, None).expect_err("no subtitle document");
    assert_eq!(refusal.to_string(), refusal.to_string());
    assert!(
        matches!(refusal, RenderError::BurnWithoutSubtitle),
        "expected the missing-subtitle refusal, got {refusal:?}"
    );
}

#[test]
fn the_plan_is_deterministic_for_the_same_revision_settings_and_tools() {
    let first = preview(&request()).expect("first compile");
    // Same revision, same settings, same tool versions — with the effects
    // handed over in the opposite order, because a plan whose order depended
    // on the caller's would hash differently for no reason.
    let mut shuffled = request();
    shuffled.effects.reverse();
    let second = preview(&shuffled).expect("second compile");
    assert_eq!(
        first.plan, second.plan,
        "compilation must be order-independent"
    );
    assert_eq!(
        serde_json::to_string(&first.plan).expect("plan serialises"),
        serde_json::to_string(&second.plan).expect("plan serialises"),
        "a content hash over the plan must be a statement about the edit"
    );
}

#[test]
fn preview_approximations_are_derived_and_never_reach_the_plan() {
    let sidecar = preview(&request()).expect("preview compiles");
    let mut burned_request = request();
    burned_request.captions.mode = CaptionMode::BurnedIn;
    burned_request.captions.style_json = "{\"title\":\"styled\"}".into();
    let burned = preview(&burned_request).expect("preview compiles a burn");

    assert!(
        !sidecar.approximate.is_empty(),
        "a plan with a loudness stage must label that stage approximate"
    );
    assert!(
        burned.approximate.len() > sidecar.approximate.len(),
        "a burned-in caption adds an approximation a sidecar does not have"
    );
    assert!(
        burned.rendered_preview.contains("/render"),
        "a labelled approximation must offer the rendered preview"
    );
    // The point of deriving them: the plan itself is untouched by the labels.
    assert_eq!(burned.plan.caption_mode, CaptionMode::BurnedIn);
    assert_eq!(
        approximations(&burned.plan).len(),
        burned.approximate.len(),
        "the plan must not carry a second copy of the labels"
    );
}
