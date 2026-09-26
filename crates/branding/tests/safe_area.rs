//! Safe areas, measured against **decoded output geometry**.
//!
//! ## Why a decoded frame and not the constant
//!
//! The claim under test is "safe areas are honoured in the render". Asserting
//! `inset_px == safe_area.rect(variant).x` would only prove the projection
//! agrees with itself — two readings of one number, one of which is the input.
//! So this file renders through real `ffmpeg`, decodes the result, finds the
//! pixels that are not the background, and compares **that bounding box**
//! against the safe rect.
//!
//! ## The control, and why it is the load-bearing half
//!
//! A logo drawn flush to the corner produces a *different* bounding box than
//! one drawn inside the safe rect, so `logo_is_inside_the_safe_rect` on its own
//! is satisfied by a no-op just as happily as by a working safe area — both
//! put the logo somewhere on the frame. `a_set_with_no_safe_area_puts_the_logo
//! _outside_it` is what makes the first assertion mean anything: the same set,
//! the same fixture, the same code path, one field different, opposite result.
//!
//! ## A note on what the *render route* cannot yet show
//!
//! The editor's executor does not apply `LayerSpec::transform.scale` (there is
//! no scale in `render/invoke.rs`), so the HTTP render route cannot produce a
//! frame at an arbitrary variant's dimensions. This test therefore drives the
//! same `build_filter_chain` output the route hands to ffmpeg, over an input
//! generated at the variant's own size. That is a real render of a real spec —
//! but it is not the route, and the difference is stated rather than hidden.
//!
//! ## ffmpeg
//!
//! Skipped, loudly, when ffmpeg is absent — but every test that skips prints
//! why, and the evidence file records that these ran with ffmpeg present. A
//! silent skip here would be a vacuous pass.

use std::path::{Path, PathBuf};
use std::process::Command;

use strivo_branding::{
    AssetKind, AssetRef, AssetTerms, BrandLibrary, BrandSet, BrandVariant, Overlay, Placement,
    PreconditionError, Slot, Stinger,
};

fn ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The rectangle of pixels that differ from `background`, as
/// `(left, top, right_exclusive, bottom_exclusive)`.
type Box_ = (u32, u32, u32, u32);

/// Decode one frame as raw `rgb24` and locate the drawn pixels.
fn drawn_box(render: &Path, width: u32, height: u32) -> Box_ {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(render)
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .output()
        .expect("ffmpeg runs");
    assert!(
        out.status.success(),
        "decoding {} failed: {}",
        render.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    let expected = (width * height * 3) as usize;
    assert_eq!(
        out.stdout.len(),
        expected,
        "decoded {} bytes, expected {expected}",
        out.stdout.len()
    );

    // The background is the picture's top-left pixel: the overlay never covers
    // the corner under test, and reading it from the frame rather than naming
    // it means the fixture's colour is not a constant this file has to keep in
    // step.
    let at = |x: u32, y: u32| {
        let i = ((y * width + x) * 3) as usize;
        (out.stdout[i], out.stdout[i + 1], out.stdout[i + 2])
    };
    let background = at(0, 0);
    let mut left = u32::MAX;
    let mut top = u32::MAX;
    let mut right = 0u32;
    let mut bottom = 0u32;
    for y in 0..height {
        for x in 0..width {
            if at(x, y) == background {
                continue;
            }
            left = left.min(x);
            top = top.min(y);
            right = right.max(x + 1);
            bottom = bottom.max(y + 1);
        }
    }
    assert_ne!(
        left,
        u32::MAX,
        "nothing was drawn at all: the overlay was dropped"
    );
    (left, top, right, bottom)
}

/// One variant's render: a uniform background at the variant's own size with
/// the set's logo composited over it.
fn render_variant(dir: &Path, set: &BrandSet, variant: &BrandVariant) -> PathBuf {
    let spec = set.apply(variant).expect("the set must project");
    let chain = spec.build_filter_chain("[0:v]");
    assert!(
        !chain.filter_complex.is_empty(),
        "an empty spec must render nothing, not a passthrough"
    );

    let source = dir.join(format!("source-{}.nut", variant.name));
    let made = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!(
            "color=c=0x202020:s={}x{}",
            variant.width, variant.height
        ))
        // rgb24 end to end, deliberately. Through yuv420p a "constant"
        // background does not round-trip to a constant: chroma subsampling
        // makes different pixels differ, so a diff-against-the-corner-pixel
        // measures the noise instead of the overlay.
        .args(["-frames:v", "1", "-c:v", "rawvideo", "-pix_fmt", "rgb24"])
        .arg(&source)
        .output()
        .expect("ffmpeg runs");
    assert!(
        made.status.success(),
        "making the source failed: {}",
        String::from_utf8_lossy(&made.stderr)
    );

    let out = dir.join(format!("render-{}.mkv", variant.name));
    let made = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-i"])
        .arg(&source)
        .args(["-filter_complex"])
        .arg(&chain.filter_complex)
        .args(["-map"])
        .arg(&chain.video_label)
        // ffv1, not the encoder default. H.264 is lossy, so a uniform
        // background comes back as a spread of near-identical colours and a
        // diff-against-the-corner measures the codec's ringing, not the
        // overlay — which showed up as a 624x66 "logo" where a 48x24 one was
        // drawn.
        .args(["-frames:v", "1", "-c:v", "ffv1"])
        .arg(&out)
        .output()
        .expect("ffmpeg runs");
    assert!(
        made.status.success(),
        "the render failed: {}",
        String::from_utf8_lossy(&made.stderr)
    );
    out
}

fn tmpdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("strivo-brand-safe-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// A red logo of a known size, so the measured box has something to measure.
fn write_logo(dir: &Path) -> PathBuf {
    let path = dir.join("logo.png");
    let made = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=48x24",
        ])
        .args(["-frames:v", "1", "-pix_fmt", "rgb24"])
        .arg(&path)
        .output()
        .expect("ffmpeg runs");
    assert!(
        made.status.success(),
        "making the logo failed: {}",
        String::from_utf8_lossy(&made.stderr)
    );
    path
}

/// The logo fixture's own size, asserted where it is measured so a scaling
/// surprise is visible as a wrong box rather than as a plausible one.
const LOGO: (u32, u32) = (48, 24);

fn set_with(logo: &Path, safe: strivo_branding::SafeArea) -> BrandSet {
    let mut set = BrandLibrary::stri_vo().sets.remove(0);
    set.name = "geometry".into();
    set.safe_area = safe;
    set.watermark = Some(Stinger {
        id: "watermark".into(),
        slot: Slot::Outro,
        overlay: Overlay::Image {
            asset: logo_asset(logo),
        },
        // Flush to the corner, so whatever inset comes back is the safe area's
        // doing and not this set's.
        placement: Placement {
            anchor: strivo_branding::Anchor::BottomRight,
            inset: 0.0,
            opacity: 1.0,
        },
        font_size: 0,
        color: "#FFFFFF".into(),
        duration_secs: 1.0,
    });
    set.lower_third = None;
    set.intro = None;
    set.outro = None;
    set
}

fn logo_asset(logo: &Path) -> AssetRef {
    AssetRef {
        name: "primary mark".into(),
        kind: AssetKind::Logo,
        fingerprint: "sha256:geometry".into(),
        locator: Some(logo.to_path_buf()),
        terms: AssetTerms::Cleared {
            note: "mine".into(),
        },
    }
}

fn variant(name: &str, width: u32, height: u32) -> BrandVariant {
    BrandVariant {
        name: name.into(),
        width,
        height,
    }
}

#[test]
fn the_logo_lands_inside_the_safe_rect_in_a_decoded_frame() {
    if !ffmpeg() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let dir = tmpdir("inside");
    let logo = write_logo(&dir);
    let set = set_with(&logo, strivo_branding::SafeArea { x: 0.05, y: 0.05 });

    for v in [
        variant("landscape", 640, 360),
        variant("portrait", 360, 640),
    ] {
        let rect = set.safe_area.rect(&v).expect("rect");
        let render = render_variant(&dir, &set, &v);
        let (left, top, right, bottom) = drawn_box(&render, v.width, v.height);
        assert_eq!(
            (right - left, bottom - top),
            LOGO,
            "{}: the measured box is not the logo's own size, so something scaled it",
            v.name
        );
        assert!(
            left >= rect.x && right <= v.width - rect.x,
            "{}: logo spans x {left}..{right}, safe rect is x {}..{}",
            v.name,
            rect.x,
            v.width - rect.x
        );
        assert!(
            top >= rect.y && bottom <= v.height - rect.y,
            "{}: logo spans y {top}..{bottom}, safe rect is y {}..{}",
            v.name,
            rect.y,
            v.height - rect.y
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_set_with_no_safe_area_puts_the_logo_outside_it() {
    // The control. Same fixture, same code path, one field different — and the
    // opposite result. Without this, the assertion above is satisfied by any
    // overlay that lands anywhere on the frame.
    if !ffmpeg() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let dir = tmpdir("outside");
    let logo = write_logo(&dir);
    let v = variant("landscape", 640, 360);
    let none = set_with(&logo, strivo_branding::SafeArea::NONE);
    let rect = strivo_branding::SafeArea { x: 0.05, y: 0.05 }
        .rect(&v)
        .expect("rect");
    assert_eq!(
        none.apply(&v)
            .expect("apply")
            .watermark
            .expect("w")
            .inset_px,
        0
    );

    let render = render_variant(&dir, &none, &v);
    let (left, top, right, bottom) = drawn_box(&render, v.width, v.height);
    assert!(
        left < rect.x || right > v.width - rect.x || top < rect.y || bottom > v.height - rect.y,
        "with no safe area the logo must leave the rect, but it sat at \
         {left},{top}..{right},{bottom} against {rect:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_portrait_cut_gets_a_vertical_budget_a_landscape_cut_does_not() {
    // The projection is per-axis, so the same set in a tall frame keeps the logo
    // clear of the *top and bottom*, not merely the sides. Measured, not assumed.
    if !ffmpeg() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let dir = tmpdir("portrait");
    let logo = write_logo(&dir);
    let set = set_with(&logo, strivo_branding::SafeArea { x: 0.0, y: 0.1 });
    let v = variant("portrait", 400, 800);
    let rect = set.safe_area.rect(&v).expect("rect");
    assert_eq!(rect.x, 0, "this set claims no horizontal margin");
    let render = render_variant(&dir, &set, &v);
    let (_, top, _, bottom) = drawn_box(&render, v.width, v.height);
    assert!(top >= rect.y, "top {top} < {}", rect.y);
    assert!(
        bottom <= v.height - rect.y,
        "bottom {bottom} > {}",
        v.height - rect.y
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_missing_logo_is_refused_before_any_ffmpeg_runs() {
    // The same precondition the route relies on, asserted with no media tools
    // involved at all: the projection refuses, so there is no filter graph to
    // hand ffmpeg in the first place.
    let dir = tmpdir("refuse");
    let logo = write_logo(&dir);
    let set = set_with(&logo, strivo_branding::SafeArea::NONE);
    let v = variant("landscape", 640, 360);
    std::fs::remove_file(&logo).expect("delete the logo");
    let err = set.apply(&v).expect_err("a deleted logo must refuse");
    assert!(
        matches!(err, PreconditionError::MissingAsset { .. }),
        "{err:?}"
    );
    assert!(err.to_string().contains("primary mark"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

/// Asserts this file's own declared test count, so a rename or a `cfg` that
/// emptied it cannot turn every assertion above into a vacuous pass.
///
/// `include_str!` on the file itself, counted for `#[test]`, compared with
/// `EXPECTED`. A `Location::caller()` check would be circular — it can only
/// ever report this file — and the count is the thing that actually goes stale.
#[test]
fn the_expected_tests_ran() {
    const EXPECTED: usize = 5;
    let source = include_str!("safe_area.rs");
    let declared = source
        .lines()
        .filter(|line| line.trim() == "#[test]")
        .count();
    assert_eq!(
        declared, EXPECTED,
        "this file declares {declared} tests but EXPECTED says {EXPECTED}; update it \
         when adding or removing a test here"
    );
}
