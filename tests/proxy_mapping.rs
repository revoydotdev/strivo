//! Proxy/original mapping correctness, measured on a fixture with known
//! landmarks.
//!
//! `fixtures/media/landmarks.mkv` is todo 32's committed corpus: 8.0 s over
//! 200 frames at 25 fps, where second *k* is one flat luma value and the
//! seconds are distinguishable from each other by that value alone
//! (`fixtures/media/README.md:54-57`). That is what makes it the right
//! instrument: a frame decoded at the wrong second has a *different luma*, so
//! the mapping is checked against decoded pixels rather than against a
//! container timestamp that could be wrong in the same way as the mapping.
//!
//! The oracle is the source's own decoded luma, not a hard-coded number:
//! ffmpeg's geq plane range and the encoder's limited range differ by a fixed
//! offset that is a property of ffmpeg, not of this fixture, so an absolute
//! constant would test ffmpeg instead of the mapping.

mod common;

use strivo_core::media::preview::PreviewSurface;
use strivo_core::media::proxy::ProxySpec;

/// Luma tolerance. The field inside a landmark second is flat and the proxy
/// re-encodes it, so the only difference is rounding in the 8-bit plane: two
/// counts out of 255.
const LUMA_TOLERANCE: f32 = 2.0;

/// One second of a 25 fps fixture. A frame decode lands on a keyframe, so the
/// honest time tolerance is the source's own keyframe gap, asserted
/// separately below rather than folded into the luma check.
const FRAME_SECS: f64 = 1.0 / 25.0;

fn spec() -> ProxySpec {
    // The corpus fixture is 128x72. A proxy of the same size is what the
    // mapping question needs: this test is about time, and upscaling would add
    // a resampling error to the luma comparison for no reason.
    ProxySpec {
        width: 128,
        height: 72,
        keyframe_interval_secs: 1,
        ..ProxySpec::default()
    }
}

/// Mean luma of a PNG, decoded rather than trusted.
fn mean_luma(png: &std::path::Path) -> f32 {
    let bytes = std::fs::read(png).expect("a decoded frame");
    let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .expect("a decodable PNG")
        .to_luma8();
    image.iter().map(|v| *v as f32).sum::<f32>() / image.len() as f32
}

#[tokio::test]
async fn a_seek_through_the_proxy_lands_on_the_right_original_second() {
    let dir = tempfile::tempdir().expect("a scratch dir");
    let source = dir.path().join("landmarks.mkv");
    std::fs::copy(common::corpus().join("landmarks.mkv"), &source).expect("fixture copy");
    let fingerprint = common::content_fingerprint(&source);

    let mut surface =
        PreviewSurface::new(dir.path().join("cache"), spec()).expect("a preview surface");
    let proxy = surface.proxy(&source, &fingerprint).await.expect("a proxy");

    // Sanity on the instrument: the proxy must have a measured keyframe index
    // on both sides, or the mapping below is being tested against nothing.
    assert!(
        !proxy.index.source_keyframes_secs.is_empty(),
        "the source keyframe index is empty, so nothing can be mapped"
    );
    assert!(
        !proxy.index.proxy_keyframes_secs.is_empty(),
        "the proxy keyframe index is empty, so nothing can be mapped"
    );

    // The oracle: each landmark second's luma, read from the SOURCE. This is a
    // whole-file decode of an 8-second fixture in a test, not an interaction
    // path, and it is the only place a source is decoded at source resolution
    // in this file.
    let mut expected = Vec::new();
    for k in 0..8 {
        expected.push(mean_luma(
            &common::oracle_frame(&source, k as f64, 128).await,
        ));
    }
    for pair in expected.windows(2) {
        assert!(
            (pair[1] - pair[0]).abs() > 8.0,
            "the landmark seconds must be distinguishable, got {expected:?}"
        );
    }

    // The claim: a seek to original time k through the proxy decodes the frame
    // the source shows at k.
    for k in 0..8 {
        let decoded = surface
            .frame(&source, &fingerprint, k as f64, 128)
            .await
            .unwrap_or_else(|e| panic!("proxy frame at {k}s: {e}"));
        let got = mean_luma(&decoded);
        let want = expected[k as usize];
        assert!(
            (got - want).abs() <= LUMA_TOLERANCE,
            "seeking the proxy to {k}s decoded luma {got}, but the source shows {want} at {k}s \
             (all landmark lumas: {expected:?})"
        );
    }

    // And the mapping itself, read from the measured index rather than from
    // the decode: a request for original time k must map to a proxy time
    // within one source keyframe interval of k, so the decode above is not
    // passing by accident.
    let gap = proxy.index.worst_keyframe_gap_secs();
    assert!(
        gap <= 1.0 + FRAME_SECS,
        "the source's worst keyframe gap is {gap}s, so a frame decode can land \
         up to that far from the requested time"
    );
    for k in 0..8 {
        let t = k as f64 + 0.5;
        let mapped = proxy.index.proxy_time_for(t);
        assert!(
            (mapped - t).abs() <= gap,
            "original {t}s maps to proxy {mapped}s, further than the worst keyframe gap {gap}s"
        );
        let target = proxy
            .index
            .seek_target(t)
            .unwrap_or_else(|| panic!("no keyframe at or before {t}s"));
        assert!(
            target.residual_secs <= gap,
            "the seek target for {t}s leaves a residual of {}s, above the gap {gap}s",
            target.residual_secs
        );
    }
}

#[tokio::test]
async fn the_mapping_is_the_identity_for_a_proxy_at_the_source_frame_rate() {
    // The unit-level statement behind the landmark test: with the source's
    // frame rate preserved, both keyframe vectors are equal and the piecewise
    // map is the identity. Asserted directly so a change in the encoder that
    // broke it would be caught here rather than only through a decode.
    let index = strivo_core::media::proxy::ProxyIndex {
        source_keyframes_secs: vec![0.0, 1.0, 2.0, 3.0],
        proxy_keyframes_secs: vec![0.0, 1.0, 2.0, 3.0],
    };
    for t in [0.0, 0.25, 1.5, 2.999, 3.0] {
        assert!(
            (index.proxy_time_for(t) - t).abs() < 1e-9,
            "map({t}) drifted"
        );
        assert!(
            (index.original_time_for(t) - t).abs() < 1e-9,
            "inverse map({t}) drifted"
        );
    }
    assert_eq!(index.worst_keyframe_gap_secs(), 1.0);
    let target = index
        .seek_target(2.4)
        .expect("a keyframe at or before 2.4s");
    assert_eq!(target.proxy_secs, 2.0);
    assert!((target.residual_secs - 0.4).abs() < 1e-9);
    assert!(
        index.seek_target(-1.0).is_none(),
        "a negative time has no keyframe before it"
    );
}

#[tokio::test]
async fn a_rescaled_proxy_is_mapped_by_its_own_keyframes() {
    // The falsifier for the mapping being "just the identity": a proxy whose
    // timeline is half the source's must map by the measured keyframes, not by
    // an identity the container does not support.
    let index = strivo_core::media::proxy::ProxyIndex {
        source_keyframes_secs: vec![0.0, 2.0, 4.0],
        proxy_keyframes_secs: vec![0.0, 1.0, 2.0],
    };
    assert!((index.proxy_time_for(1.0) - 0.5).abs() < 1e-9);
    assert!((index.proxy_time_for(3.0) - 1.5).abs() < 1e-9);
    assert!((index.original_time_for(1.5) - 3.0).abs() < 1e-9);
    // Beyond the last keyframe, the last segment is extrapolated, not clamped
    // to the last keyframe: clamping would silently freeze the playhead.
    assert!(index.proxy_time_for(6.0) > 2.0);
}
