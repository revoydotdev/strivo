//! The `Must NOT`: no interaction path spawns an original-resolution decode.
//!
//! Asserted against [`strivo_core::media::spawn::snapshot`] — a record of the
//! child processes that actually ran — not against the code's shape. A shape
//! assertion reads identically before and after someone adds a second spawn
//! site, which is the exact regression this exists to catch.
//!
//! Three things make the observation falsifiable rather than decorative:
//!
//! 1. **The window must be non-empty.** An empty record satisfies every "no
//!    spawn did X" assertion while proving nothing, so emptiness is a failure.
//! 2. **A falsifier.** A deliberately regressed interaction that decodes the
//!    source is driven through the same predicate and must be caught. A
//!    predicate that never matches is indistinguishable from one that works.
//! 3. **One sequential test, not four.** The record and the two-slot pool are
//!    both process-global, so a second `#[tokio::test]` in this file runs
//!    concurrently and its spawns land in the first one's window. That was not
//!    a theoretical concern: written as four tests, the falsifier's
//!    deliberately-regressed spawn made the real assertion fail, which is the
//!    pollution working exactly as designed and exactly as inconvenient. The
//!    phases below are sequential and each clears the record first.

mod common;

use std::path::Path;
use strivo_core::media::preview::PreviewSurface;
use strivo_core::media::proxy::ProxySpec;
use strivo_core::media::spawn::{self, Spawn};

fn spec() -> ProxySpec {
    // A small proxy keeps the test fast; the `Must NOT` is about which file is
    // decoded, not about how many pixels it has.
    ProxySpec {
        width: 160,
        height: 90,
        keyframe_interval_secs: 1,
        ..ProxySpec::default()
    }
}

/// The predicate the claim is made with. It lives in the test so a change to
/// the product cannot quietly weaken the assertion.
fn violations(spawns: &[Spawn], source: &Path) -> Vec<String> {
    let source = source.to_string_lossy();
    spawns
        .iter()
        .filter(|s| {
            // The strong form: during an interaction, nothing at all reads the
            // user's file — not even a header-only probe. The weak form,
            // `is_original_resolution`, is weaker than it looks: a regressed
            // call through `frame_from_proxy` still records `input_is_proxy`,
            // so only the input identity actually catches that regression.
            s.input == source || s.is_original_resolution()
        })
        .map(|s| format!("{} {} on {}", s.tool, s.purpose, s.input))
        .collect()
}

#[tokio::test]
async fn interaction_reads_only_proxies_and_the_check_can_fail() {
    let dir = tempfile::tempdir().expect("a scratch dir");
    let source = dir.path().join("landmarks.mkv");
    std::fs::copy(common::corpus().join("landmarks.mkv"), &source).expect("fixture copy");
    let fingerprint = common::content_fingerprint(&source);
    let mut surface =
        PreviewSurface::new(dir.path().join("cache"), spec()).expect("a preview surface");

    // ---- phase 1: the cold build, which is not an interaction ----------------
    // The one transcode reads the source by definition, so it is deliberately
    // outside every measured window.
    surface
        .proxy(&source, &fingerprint)
        .await
        .expect("a proxy is built once");
    let cold = spawn::snapshot();
    assert!(
        cold.iter()
            .any(|s| s.purpose == "proxy-build" && s.input == source.to_string_lossy()),
        "the cold build must be visible in the record, or the windows below are not \
         distinguishing a build from an interaction"
    );

    // ---- phase 2: the interaction window ------------------------------------
    spawn::clear();
    for pass in 0..2 {
        for k in 0..8 {
            surface
                .frame(&source, &fingerprint, k as f64, 160)
                .await
                .unwrap_or_else(|e| panic!("pass {pass}: frame at {k}s: {e}"));
        }
        let pyramid = surface
            .waveform(&source, &fingerprint)
            .await
            .expect("a waveform pyramid");
        assert!(
            !pyramid.levels.is_empty(),
            "the pyramid has no levels, so the waveform interaction decoded nothing"
        );
        let strip = surface
            .filmstrip(&source, &fingerprint, 8, 80, 4)
            .await
            .expect("a filmstrip");
        assert!(strip.path.exists(), "the filmstrip file is missing");
    }

    let window = spawn::snapshot();

    // Non-vacuity guard 1.
    assert!(
        !window.is_empty(),
        "no child process was recorded during the interaction window, so \
         'no original-resolution decode' is being asserted about nothing"
    );
    assert!(
        window.iter().any(|s| s.purpose == "proxy-frame"),
        "the frame interaction recorded no decode, so the window did not exercise it"
    );
    assert!(
        window.iter().any(|s| s.purpose == "pyramid-build"),
        "the waveform interaction recorded nothing, so the window did not exercise it"
    );
    assert!(
        window.iter().any(|s| s.purpose == "filmstrip-build"),
        "the filmstrip interaction recorded nothing, so the window did not exercise it"
    );

    // The claim.
    let bad = violations(&window, &source);
    assert!(
        bad.is_empty(),
        "an interaction path touched the original source: {bad:?}\nfull record: {window:#?}"
    );
    for s in &window {
        if s.decodes_picture {
            assert!(
                s.input != source.to_string_lossy(),
                "{} decoded picture from the source itself: {}",
                s.purpose,
                s.input
            );
        }
    }

    // ---- phase 3: a second scrub is served entirely from the cache ----------
    // "Navigation does not decode the whole thing" is only worth something if
    // repeated navigation is cheap.
    spawn::clear();
    for k in 0..8 {
        let first = surface
            .frame(&source, &fingerprint, k as f64, 160)
            .await
            .expect("a warm frame");
        let second = surface
            .frame(&source, &fingerprint, k as f64, 160)
            .await
            .expect("a warm frame");
        assert_eq!(first, second, "a cache hit must be the same file");
    }
    assert!(
        spawn::snapshot().is_empty(),
        "a second pass over already-decoded times spawned {:?}; a scrub must not re-decode",
        spawn::snapshot()
    );

    // ---- phase 4: FALSIFY. A regressed interaction must be caught. -----------
    spawn::clear();
    let regressed = strivo_core::media::proxy::ProxyArtifact {
        key: "regressed".into(),
        path: source.clone(),
        source: source.clone(),
        width: 160,
        height: 90,
        duration_secs: 8.0,
        rebuilt: false,
        index: Default::default(),
    };
    strivo_core::media::proxy::frame_from_proxy(&regressed, 1.0, 160, &dir.path().join("scratch"))
        .await
        .expect("the regressed decode runs");

    let regressed_window = spawn::snapshot();
    assert!(
        !regressed_window.is_empty(),
        "the regressed decode recorded nothing"
    );
    let caught = violations(&regressed_window, &source);
    assert_eq!(
        caught.len(),
        1,
        "the predicate must catch exactly the one regressed decode, got {caught:?} of \
         {regressed_window:#?}"
    );
    assert!(
        caught[0].contains("proxy-frame"),
        "the violation must name the interaction that caused it: {}",
        caught[0]
    );

    // ---- phase 5: the two-slot bound, enforced at acquisition ---------------
    assert_eq!(
        spawn::MEDIA_SLOTS,
        2,
        "the bound is two, the same one PERFORMANCE.md states"
    );
    let a = spawn::acquire_slot("test a").await.expect("slot 1");
    let b = spawn::acquire_slot("test b").await.expect("slot 2");
    assert_eq!(
        spawn::free_slots(),
        0,
        "two permits held must leave no free slot"
    );
    let refused = spawn::acquire_slot_within("seek", std::time::Duration::from_millis(50))
        .await
        .expect_err("a third interaction must not get a slot while two are held");
    assert!(
        refused.contains("no media slot") && refused.contains("2 slots"),
        "the refusal must name the bound it hit: {refused}"
    );
    drop(a);
    assert_eq!(
        spawn::free_slots(),
        1,
        "a dropped permit returns immediately"
    );
    let c = spawn::acquire_slot_within("seek", std::time::Duration::from_millis(50))
        .await
        .expect("a slot again once one is returned");
    drop(b);
    drop(c);
    assert_eq!(
        spawn::free_slots(),
        spawn::MEDIA_SLOTS,
        "permits return when dropped"
    );
}
