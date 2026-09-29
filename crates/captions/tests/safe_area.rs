//! The safe-area preview, and that it matches the render's geometry.
//!
//! The safe area is todo 47's, not a second notion: every number here comes out
//! of `strivo_branding::SafeArea::rect`, the same call a watermark goes through.
//! The test **measures** the caption band against that rectangle rather than
//! asserting a constant, and carries a control that proves the measurement can
//! fail.

use strivo_branding::{BrandVariant, SafeArea};
use strivo_captions::{clips_safe_area, preview_band, style_within_safe_area, AssStyle};

fn variant(width: u32, height: u32) -> BrandVariant {
    BrandVariant {
        name: format!("{width}x{height}"),
        width,
        height,
    }
}

#[test]
fn the_band_is_measured_inside_the_safe_rect_and_not_asserted() {
    let safe = SafeArea { x: 0.05, y: 0.05 };
    let v = variant(1920, 1080);
    let rect = safe.rect(&v).expect("a real frame");
    let band = preview_band(&AssStyle::default(), rect);

    assert_eq!(rect.x, 96, "5% of 1920");
    assert_eq!(rect.y, 54, "5% of 1080");
    assert!(
        !clips_safe_area(band, rect),
        "band {band:?} leaves the safe rect {rect:?}"
    );
    // The band is measured, so its own numbers have to be consistent with the
    // frame it was measured against.
    assert_eq!(band.x, rect.x);
    assert_eq!(band.y + band.height, rect.y + rect.height);
    assert_eq!(band.width, rect.width);
}

#[test]
fn the_measurement_can_fail() {
    // The anti-vacuity control. A check that has never failed has not run, so
    // this drives a band that genuinely leaves the rect and requires it to say
    // so. Without this, the test above would pass for a `clips_safe_area` that
    // always answers "fits".
    let safe = SafeArea { x: 0.05, y: 0.05 };
    let rect = safe.rect(&variant(1920, 1080)).expect("a real frame");

    let too_tall = AssStyle {
        font_size: rect.height + 1,
        ..AssStyle::default()
    };
    let band = preview_band(&too_tall, rect);
    assert!(
        clips_safe_area(band, rect),
        "a band taller than the safe rect must be reported, not quietly fitted"
    );

    // And one that is short enough by a single pixel.
    let just_fits = AssStyle {
        font_size: rect.height,
        margin_v_px: 0,
        ..AssStyle::default()
    };
    assert!(
        !clips_safe_area(preview_band(&just_fits, rect), rect),
        "a band exactly the safe rect's height fits"
    );
}

#[test]
fn a_safe_area_wider_than_the_frame_shrinks_its_own_inset() {
    // The safe area scales with the frame, so the same authored margin is a
    // different number of pixels on a vertical variant. Asserting one constant
    // across two frames is how a preview ends up right on one and wrong on the
    // other.
    let safe = SafeArea { x: 0.05, y: 0.1 };
    let wide = safe.rect(&variant(1920, 1080)).expect("a real frame");
    let tall = safe.rect(&variant(1080, 1920)).expect("a real frame");
    assert_eq!((wide.y, tall.y), (108, 192));
    assert_eq!((wide.height, tall.height), (864, 1536));
}

#[test]
fn a_margin_below_the_safe_inset_is_raised_and_one_above_it_is_kept() {
    let safe = SafeArea { x: 0.05, y: 0.1 };
    let rect = safe.rect(&variant(1920, 1080)).expect("a real frame");
    assert_eq!(rect.y, 108);

    let too_close = AssStyle {
        margin_v_px: 40,
        ..AssStyle::default()
    };
    assert_eq!(style_within_safe_area(&too_close, rect).margin_v_px, 108);

    // A creator who asked for 120px to clear a Shorts overlay still gets 120.
    let already_clear = AssStyle {
        margin_v_px: 120,
        ..AssStyle::default()
    };
    assert_eq!(
        style_within_safe_area(&already_clear, rect).margin_v_px,
        120
    );
}

#[test]
fn the_raised_margin_is_what_the_ass_document_carries() {
    // The preview geometry and the burn document must come from the same
    // number, so a style raised for the safe area reaches the renderer rather
    // than only the preview.
    let safe = SafeArea { x: 0.05, y: 0.1 };
    let rect = safe.rect(&variant(1920, 1080)).expect("a real frame");
    let style = style_within_safe_area(
        &AssStyle {
            margin_v_px: 40,
            ..AssStyle::default()
        },
        rect,
    );
    let ass = strivo_captions::to_ass(
        &[strivo_captions::Segment {
            start_sec: 0.0,
            end_sec: 1.0,
            text: "hi".into(),
            speaker: None,
        }],
        &style,
        &[],
    );
    assert!(
        ass.contains(",20,20,108,"),
        "the ASS style line carries the raised MarginV: {ass}"
    );
}

#[test]
fn a_degenerate_frame_is_refused_rather_than_making_the_check_vacuous() {
    // "Everything is inside a 0x0 box" is a vacuous guarantee, so a zero frame
    // is an error. This is branding's refusal; the point here is that a caption
    // export inherits it instead of inventing a full-frame default.
    let safe = SafeArea { x: 0.05, y: 0.05 };
    assert!(safe.rect(&variant(0, 1080)).is_err());
}
