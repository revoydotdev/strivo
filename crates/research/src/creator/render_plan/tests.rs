//! The plan row's own contract: a compiled plan round-trips, its hash is a
//! statement about the edit, and a stale row is detectable.
//!
//! The staleness test is the load-bearing one here. Todo 24 established that a
//! cache key hashes **declared inputs** and never a file's size or mtime; this
//! is the same rule applied to a plan, and the mutation that would break it is
//! a hash that ignores the plan body — which would report every plan fresh.

use strivo_editor::render::{example, CompiledPlan};

use super::*;
use crate::creator::entities::Variant;
use crate::creator::ids::{CompositionRevisionId, VariantId};

/// A destination with no overrides — which is what "rendered as authored" is.
fn plain_variant() -> Variant {
    Variant {
        id: VariantId::generate(),
        project_id: ProjectId::generate(),
        composition_revision_id: CompositionRevisionId::generate(),
        name: "as-authored".into(),
        format_overrides_json: String::new(),
        created_at: "2026-09-26T00:00:00Z".into(),
    }
}

fn plan() -> CompiledPlan {
    example::compiled(&example::request())
}

#[test]
fn a_compiled_plan_round_trips_through_its_row() {
    let compiled = plan();
    let variant = plain_variant();
    let row = compile_render_plan(
        variant.project_id,
        &variant,
        &compiled,
        "2026-09-26T00:00:00Z",
    )
    .expect("the row is written");
    let read_back = read_plan(&row).expect("the row parses");
    assert_eq!(
        read_back, compiled,
        "a stored plan must read back as itself"
    );
    assert_eq!(row.renderer, "ffmpeg");
    assert_eq!(row.renderer_version, "n9.0.2");
    assert_eq!(row.variant_id, variant.id);
}

#[test]
fn the_content_hash_covers_the_plan_body() {
    let compiled = plan();
    let baseline = content_hash(&compiled, "");
    assert_eq!(
        baseline,
        content_hash(&example::compiled(&example::request()), ""),
        "the same edit hashes the same"
    );

    // Each of these is a different edit. A hash that ignored the body would
    // call them all equal, which is the exact defect this guards.
    let mut shortened = example::request();
    shortened.clips.truncate(1);
    // The layer and the track name cut 1 too, so removing the cut has to
    // remove the references as well — the compiler refuses a dangling one, and
    // that refusal is the answer, not a fixture to work around.
    shortened.layers[0].clips = vec![0];
    shortened.tracks[0].clips = vec![0];
    shortened.transitions.clear();
    shortened.captions.cues.clear();
    assert_ne!(
        baseline,
        content_hash(&example::compiled(&shortened), ""),
        "a cut removed"
    );

    let mut louder = example::request();
    louder.loudness = Some(strivo_editor::render::LoudnessSpec {
        platform: "spotify".into(),
        i: -14.0,
        lra: 7.0,
        tp: -1.0,
    });
    assert_ne!(
        baseline,
        content_hash(&example::compiled(&louder), ""),
        "a target changed"
    );

    let mut toolchain = example::request();
    toolchain.profile.renderer_version = "n10.0.0".into();
    assert_ne!(
        baseline,
        content_hash(&example::compiled(&toolchain), ""),
        "a tool version changed"
    );

    assert_ne!(
        baseline,
        content_hash(&compiled, "{\"max_bitrate\":\"12M\"}"),
        "overrides"
    );
}

#[test]
fn a_row_is_stale_exactly_when_a_fresh_compile_differs() {
    let compiled = plan();
    let variant = plain_variant();
    let row = compile_render_plan(
        variant.project_id,
        &variant,
        &compiled,
        "2026-09-26T00:00:00Z",
    )
    .expect("the row is written");
    assert!(
        !is_stale(&row, &example::compiled(&example::request()), ""),
        "an unchanged edit is not stale"
    );

    // A revision edited after the plan was compiled. Re-saving identical
    // content is not an edit, and a revision-id comparison would call this
    // stale while missing a settings change that left the id alone.
    let mut edited = example::request();
    edited.tracks[0].gain_db = -6.0;
    assert!(
        is_stale(&row, &example::compiled(&edited), ""),
        "a track gain changed"
    );

    // Same plan, different variant overrides: a different render, so the row
    // does not answer for it.
    assert!(
        is_stale(&row, &plan(), "{\"max_bitrate\":\"12M\"}"),
        "different destination overrides are a different plan"
    );
}

#[test]
fn a_row_reads_back_through_the_variant_it_names() {
    let compiled = plan();
    let variant = Variant {
        name: "shorts".into(),
        format_overrides_json: "{\"max_bitrate\":\"12M\"}".into(),
        ..plain_variant()
    };
    let row = compile_render_plan(
        variant.project_id,
        &variant,
        &compiled,
        "2026-09-26T00:00:00Z",
    )
    .expect("the row is written");
    assert_eq!(row.variant_id, variant.id);
    assert!(!is_stale(&row, &compiled, &variant.format_overrides_json));
    assert!(
        is_stale(&row, &compiled, ""),
        "the overrides are part of the identity"
    );
}
