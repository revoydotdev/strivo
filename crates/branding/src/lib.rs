//! Branding overlay spec → ffmpeg filter graph.
//!
//! Pure data: takes a [`BrandingSpec`] (one optional watermark + zero-or-more
//! intro/outro banners) and emits a single `-filter_complex` chain plus the
//! resulting output label. The host wires the chain into ffmpeg; this crate
//! does no IO and is exhaustively unit-tested against canned filter strings.
//!
//! ## Two ways to get a spec, and one gate
//!
//! * [`BrandingSpec`] — a whole overlay spec, written per recording. Kept, and
//!   still honoured, because it is what the existing editor panel persists.
//! * [`library::BrandSet`] — a **reusable** set in the [`asset`] library,
//!   projected onto a destination variant by [`library::BrandSet::apply`].
//!   Many recordings name one set; none of them stores a copy of it.
//!
//! Both go through the same preconditions before they can become a filter
//! graph, and a missing or unreviewed asset **refuses the export by name**
//! rather than producing a graph that would drop it. See [`library`] for the
//! refusal vocabulary and the export-boundary contract.

pub mod asset;
pub mod library;

pub use asset::{AssetAvailability, AssetKind, AssetRef, AssetTerms};
pub use library::{
    resolve, BrandLibrary, BrandSet, BrandVariant, MissingReason, Overlay, Palette, Placement,
    PreconditionError, RecordingBrand, SafeArea, SafeRect, Slot, Stinger,
};

use serde::{Deserialize, Serialize};

/// Nine-point anchor grid (matches CSS-style positioning).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    TopLeft,
    TopCenter,
    TopRight,
    MiddleLeft,
    MiddleCenter,
    MiddleRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl Anchor {
    /// ffmpeg `overlay=x:y` expression for the given anchor + inset (px).
    /// Uses `main_w`/`main_h`/`overlay_w`/`overlay_h` so it scales with input.
    pub fn xy_expr(self, inset_px: u32) -> (String, String) {
        let m = inset_px;
        match self {
            Anchor::TopLeft => (format!("{m}"), format!("{m}")),
            Anchor::TopCenter => ("(main_w-overlay_w)/2".into(), format!("{m}")),
            Anchor::TopRight => (format!("main_w-overlay_w-{m}"), format!("{m}")),
            Anchor::MiddleLeft => (format!("{m}"), "(main_h-overlay_h)/2".into()),
            Anchor::MiddleCenter => ("(main_w-overlay_w)/2".into(), "(main_h-overlay_h)/2".into()),
            Anchor::MiddleRight => (
                format!("main_w-overlay_w-{m}"),
                "(main_h-overlay_h)/2".into(),
            ),
            Anchor::BottomLeft => (format!("{m}"), format!("main_h-overlay_h-{m}")),
            Anchor::BottomCenter => (
                "(main_w-overlay_w)/2".into(),
                format!("main_h-overlay_h-{m}"),
            ),
            Anchor::BottomRight => (
                format!("main_w-overlay_w-{m}"),
                format!("main_h-overlay_h-{m}"),
            ),
        }
    }
}

/// Either a static image or rendered text watermark.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WatermarkSource {
    /// Path to a PNG/SVG/etc. loaded via a `movie=` input.
    Image { path: String },
    /// Rendered with `drawtext`.
    Text {
        text: String,
        font_size: u32,
        color_rgba: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Watermark {
    pub source: WatermarkSource,
    pub anchor: Anchor,
    pub inset_px: u32,
    /// 0.0 – 1.0
    pub opacity: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BannerSlot {
    /// Visible from t=0 to t=duration_secs.
    Intro,
    /// Visible from t=video_duration-duration_secs to end. Host substitutes
    /// `{end}` token (we emit `t>=main_duration-D`).
    Outro,
    /// Visible for `duration_secs` starting at `at_secs`. A lower third is
    /// neither bookend — it is placed, not opened or closed with the video —
    /// so it could not be expressed as one of the two above.
    LowerThird { at_secs: f32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Banner {
    pub slot: BannerSlot,
    pub text: String,
    pub font_size: u32,
    pub color_rgba: String,
    pub anchor: Anchor,
    pub inset_px: u32,
    pub duration_secs: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BrandingSpec {
    pub watermark: Option<Watermark>,
    pub banners: Vec<Banner>,
}

/// Output of [`BrandingSpec::build_filter_chain`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterChain {
    /// ffmpeg `-filter_complex` argument (without the flag itself).
    pub filter_complex: String,
    /// Label of the final mapped video stream (e.g. `[vout]`).
    pub video_label: String,
    /// Additional `-i <path>` inputs the host must prepend, in order.
    /// Image watermarks are wired via `movie=` inside the filter graph so they
    /// don't appear here — this is reserved for future use.
    pub extra_inputs: Vec<String>,
}

impl BrandingSpec {
    /// Whether this spec asks for nothing. A projection that came back empty
    /// must produce an **empty** `filter_complex`, not a passthrough `copy`:
    /// the plan's `video_filter_complex` is what the executor runs, and a
    /// `[0:v]copy[vout]` there would claim a video stage the export never does.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.watermark.is_none() && self.banners.is_empty()
    }

    /// Build a `-filter_complex` chain.
    ///
    /// `in_video_label` is the label of the host's source video stream, e.g.
    /// `[0:v]` for the first input. The chain ends at [`FilterChain::video_label`].
    ///
    /// If the spec is empty (no watermark, no banners), returns a passthrough
    /// chain `<in>copy<out>` so the host can blindly substitute it.
    pub fn build_filter_chain(&self, in_video_label: &str) -> FilterChain {
        let mut steps: Vec<String> = Vec::new();
        let mut current = in_video_label.to_string();
        let mut next_label_idx = 0usize;
        let mut next_label = || {
            let s = format!("[b{next_label_idx}]");
            next_label_idx += 1;
            s
        };

        if let Some(w) = &self.watermark {
            let out = next_label();
            steps.push(watermark_step(w, &current, &out));
            current = out;
        }
        for banner in &self.banners {
            let out = next_label();
            steps.push(banner_step(banner, &current, &out));
            current = out;
        }

        if steps.is_empty() {
            // Passthrough: explicit `copy` keeps host code uniform.
            let out = "[vout]".to_string();
            return FilterChain {
                filter_complex: format!("{in_video_label}copy{out}"),
                video_label: out,
                extra_inputs: vec![],
            };
        }

        // Rename the final step's output to a stable `[vout]`.
        let final_step = steps.pop().unwrap();
        let renamed = final_step.replace(&current, "[vout]");
        steps.push(renamed);

        FilterChain {
            filter_complex: steps.join(";"),
            video_label: "[vout]".into(),
            extra_inputs: vec![],
        }
    }
}

fn watermark_step(w: &Watermark, in_label: &str, out_label: &str) -> String {
    let opacity = w.opacity.clamp(0.0, 1.0);
    let (x, y) = w.anchor.xy_expr(w.inset_px);
    match &w.source {
        WatermarkSource::Image { path } => {
            // [movie][in]overlay=x:y:format=auto[out] — opacity via colorchannelmixer
            let escaped = ffmpeg_escape(path);
            format!(
                "movie={escaped},format=rgba,colorchannelmixer=aa={opacity:.3}[wm];{in_label}[wm]overlay={x}:{y}{out_label}"
            )
        }
        WatermarkSource::Text {
            text,
            font_size,
            color_rgba,
        } => {
            let escaped = drawtext_escape(text);
            let color = color_with_alpha(color_rgba, opacity);
            format!(
                "{in_label}drawtext=text='{escaped}':fontsize={font_size}:fontcolor={color}:x={x}:y={y}{out_label}"
            )
        }
    }
}

fn banner_step(b: &Banner, in_label: &str, out_label: &str) -> String {
    let (x, y) = b.anchor.xy_expr(b.inset_px);
    let escaped = drawtext_escape(&b.text);
    let enable = match b.slot {
        BannerSlot::Intro => format!("lt(t,{:.3})", b.duration_secs),
        BannerSlot::Outro => format!("gte(t,main_duration-{:.3})", b.duration_secs),
        BannerSlot::LowerThird { at_secs } => {
            format!("between(t,{at_secs:.3},{:.3})", at_secs + b.duration_secs)
        }
    };
    format!(
        "{in_label}drawtext=text='{escaped}':fontsize={size}:fontcolor={color}:x={x}:y={y}:enable='{enable}'{out_label}",
        size = b.font_size,
        color = b.color_rgba,
    )
}

/// Escape a literal path/value for use inside an ffmpeg filter chain.
/// Backslash, single-quote and colon are the troublemakers.
pub fn ffmpeg_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace(':', "\\:")
        .replace('\'', "\\'")
}

/// Escape a string for use inside a `drawtext` `text='…'` literal.
pub fn drawtext_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace(':', "\\:")
}

/// Apply opacity to an `rgba(R,G,B,A)`-ish or `0xRRGGBB`-ish colour.
/// We accept ffmpeg's own `color@alpha` shorthand: `white@0.5` etc., and
/// multiply through if alpha is already present.
fn color_with_alpha(color: &str, opacity: f32) -> String {
    let op = opacity.clamp(0.0, 1.0);
    if let Some((c, a)) = color.split_once('@') {
        let existing: f32 = a.parse().unwrap_or(1.0);
        format!("{c}@{:.3}", existing.clamp(0.0, 1.0) * op)
    } else {
        format!("{color}@{op:.3}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wm_text() -> Watermark {
        Watermark {
            source: WatermarkSource::Text {
                text: "@channel".into(),
                font_size: 32,
                color_rgba: "white".into(),
            },
            anchor: Anchor::BottomRight,
            inset_px: 24,
            opacity: 0.7,
        }
    }

    #[test]
    fn empty_spec_is_passthrough() {
        let chain = BrandingSpec::default().build_filter_chain("[0:v]");
        assert_eq!(chain.filter_complex, "[0:v]copy[vout]");
        assert_eq!(chain.video_label, "[vout]");
    }

    #[test]
    fn anchor_bottom_right_uses_overlay_w_h() {
        let (x, y) = Anchor::BottomRight.xy_expr(24);
        assert_eq!(x, "main_w-overlay_w-24");
        assert_eq!(y, "main_h-overlay_h-24");
    }

    #[test]
    fn anchor_top_left_inset_zero() {
        let (x, y) = Anchor::TopLeft.xy_expr(0);
        assert_eq!(x, "0");
        assert_eq!(y, "0");
    }

    #[test]
    fn anchor_middle_center_uses_centred_expr() {
        let (x, y) = Anchor::MiddleCenter.xy_expr(0);
        assert_eq!(x, "(main_w-overlay_w)/2");
        assert_eq!(y, "(main_h-overlay_h)/2");
    }

    #[test]
    fn text_watermark_emits_drawtext() {
        let spec = BrandingSpec {
            watermark: Some(wm_text()),
            banners: vec![],
        };
        let chain = spec.build_filter_chain("[0:v]");
        assert!(chain
            .filter_complex
            .starts_with("[0:v]drawtext=text='@channel'"));
        assert!(chain.filter_complex.contains("fontsize=32"));
        assert!(chain.filter_complex.contains("fontcolor=white@0.700"));
        assert!(chain.filter_complex.contains("x=main_w-overlay_w-24"));
        assert!(chain.filter_complex.ends_with("[vout]"));
    }

    #[test]
    fn image_watermark_uses_movie_and_colorchannelmixer() {
        let spec = BrandingSpec {
            watermark: Some(Watermark {
                source: WatermarkSource::Image {
                    path: "/tmp/logo.png".into(),
                },
                anchor: Anchor::TopRight,
                inset_px: 12,
                opacity: 0.5,
            }),
            banners: vec![],
        };
        let chain = spec.build_filter_chain("[0:v]");
        assert!(chain.filter_complex.contains("movie=/tmp/logo.png"));
        assert!(chain.filter_complex.contains("colorchannelmixer=aa=0.500"));
        assert!(chain
            .filter_complex
            .contains("overlay=main_w-overlay_w-12:12"));
    }

    #[test]
    fn opacity_clamps_to_unit_range() {
        let spec = BrandingSpec {
            watermark: Some(Watermark {
                source: WatermarkSource::Text {
                    text: "x".into(),
                    font_size: 16,
                    color_rgba: "white".into(),
                },
                anchor: Anchor::TopLeft,
                inset_px: 0,
                opacity: 1.7,
            }),
            banners: vec![],
        };
        let chain = spec.build_filter_chain("[0:v]");
        assert!(chain.filter_complex.contains("white@1.000"));
    }

    #[test]
    fn opacity_negative_clamps_to_zero() {
        let spec = BrandingSpec {
            watermark: Some(Watermark {
                source: WatermarkSource::Text {
                    text: "x".into(),
                    font_size: 16,
                    color_rgba: "white".into(),
                },
                anchor: Anchor::TopLeft,
                inset_px: 0,
                opacity: -0.4,
            }),
            banners: vec![],
        };
        let chain = spec.build_filter_chain("[0:v]");
        assert!(chain.filter_complex.contains("white@0.000"));
    }

    #[test]
    fn existing_alpha_is_multiplied_through() {
        // 0.8 (in color) * 0.5 (opacity) = 0.400
        let c = color_with_alpha("red@0.8", 0.5);
        assert_eq!(c, "red@0.400");
    }

    #[test]
    fn intro_banner_uses_lt_enable() {
        let spec = BrandingSpec {
            watermark: None,
            banners: vec![Banner {
                slot: BannerSlot::Intro,
                text: "Welcome".into(),
                font_size: 48,
                color_rgba: "yellow".into(),
                anchor: Anchor::TopCenter,
                inset_px: 40,
                duration_secs: 3.5,
            }],
        };
        let chain = spec.build_filter_chain("[0:v]");
        assert!(chain.filter_complex.contains("enable='lt(t,3.500)'"));
        assert!(chain.filter_complex.contains("fontcolor=yellow"));
    }

    #[test]
    fn outro_banner_uses_gte_main_duration_enable() {
        let spec = BrandingSpec {
            watermark: None,
            banners: vec![Banner {
                slot: BannerSlot::Outro,
                text: "Like & Sub".into(),
                font_size: 48,
                color_rgba: "white".into(),
                anchor: Anchor::BottomCenter,
                inset_px: 40,
                duration_secs: 5.0,
            }],
        };
        let chain = spec.build_filter_chain("[0:v]");
        assert!(chain
            .filter_complex
            .contains("enable='gte(t,main_duration-5.000)'"));
    }

    #[test]
    fn watermark_plus_two_banners_chains_in_order() {
        let spec = BrandingSpec {
            watermark: Some(wm_text()),
            banners: vec![
                Banner {
                    slot: BannerSlot::Intro,
                    text: "Hi".into(),
                    font_size: 40,
                    color_rgba: "white".into(),
                    anchor: Anchor::TopCenter,
                    inset_px: 20,
                    duration_secs: 2.0,
                },
                Banner {
                    slot: BannerSlot::Outro,
                    text: "Bye".into(),
                    font_size: 40,
                    color_rgba: "white".into(),
                    anchor: Anchor::BottomCenter,
                    inset_px: 20,
                    duration_secs: 4.0,
                },
            ],
        };
        let chain = spec.build_filter_chain("[0:v]");
        // Three steps separated by `;`
        assert_eq!(chain.filter_complex.matches(';').count(), 2);
        // Final stream is `[vout]`
        assert!(chain.filter_complex.ends_with("[vout]"));
        // Ordering preserved: watermark first, then intro, then outro
        let wm_pos = chain.filter_complex.find("@channel").unwrap();
        let intro_pos = chain.filter_complex.find("Hi").unwrap();
        let outro_pos = chain.filter_complex.find("Bye").unwrap();
        assert!(wm_pos < intro_pos && intro_pos < outro_pos);
    }

    #[test]
    fn drawtext_escapes_colon_quote_and_backslash() {
        let s = drawtext_escape("It's 9:00 \\o/");
        assert_eq!(s, "It\\'s 9\\:00 \\\\o/");
    }

    #[test]
    fn ffmpeg_escape_handles_colon_in_path() {
        let s = ffmpeg_escape("/tmp/a:b/logo.png");
        assert_eq!(s, "/tmp/a\\:b/logo.png");
    }

    #[test]
    fn anchor_serialises_snake_case_for_spa() {
        // SPA sends `bottom_right`, not `BottomRight`.
        let s = serde_json::to_string(&Anchor::BottomRight).unwrap();
        assert_eq!(s, "\"bottom_right\"");
        let back: Anchor = serde_json::from_str("\"top_center\"").unwrap();
        assert_eq!(back, Anchor::TopCenter);
    }

    #[test]
    fn json_roundtrip_preserves_spec() {
        let spec = BrandingSpec {
            watermark: Some(wm_text()),
            banners: vec![Banner {
                slot: BannerSlot::Outro,
                text: "End".into(),
                font_size: 32,
                color_rgba: "white".into(),
                anchor: Anchor::BottomCenter,
                inset_px: 30,
                duration_secs: 3.0,
            }],
        };
        let s = serde_json::to_string(&spec).unwrap();
        let back: BrandingSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(back.banners.len(), 1);
        assert_eq!(back.banners[0].slot, BannerSlot::Outro);
        assert!(back.watermark.is_some());
    }
}
