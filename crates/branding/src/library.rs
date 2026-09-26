//! The reusable brand library: named sets, destination variants, and the
//! preconditions an export has to pass.
//!
//! Report item E07. The crate previously held a **per-recording** overlay
//! spec and nothing else, which is the gap this module closes: a set is
//! authored once and applied to many variants, instead of being retyped per
//! recording.
//!
//! ## One set, many variants
//!
//! [`BrandSet`] is authored in a **reference frame** ([`BrandSet::reference`])
//! — usually 1920×1080 — and [`BrandSet::apply`] projects it onto a
//! [`BrandVariant`]. Insets and font sizes are stored as **fractions of the
//! axis**, not pixels, so the projection is a scale and not a table of magic
//! numbers per destination. A 9:16 cut of the same set keeps the same logo, the
//! same palette and the same proportions; only the pixel budget moves.
//!
//! That is the whole of "reusable", and it is testable without reading this
//! paragraph: [`three_destination_shapes_share_one_set`] applies one set to
//! landscape, portrait and square and asserts the three specs are distinct
//! while every asset identity in them is the same.
//!
//! ## Missing is a refusal, not a warning
//!
//! [`BrandSet::apply`] returns `Result`, and the error path is the todo's
//! `Must NOT`: a brand asset whose bytes are gone, or whose terms nobody has
//! recorded, **stops the export and names the asset**. There is no code path
//! from a [`BrandSet`] to a [`BrandingSpec`](super::BrandingSpec) that skips
//! the check, so a caller cannot obtain a filter graph referencing a file that
//! is not there. A `drawtext` overlay has no bytes to lose and is never
//! refused for an asset it does not use.
//!
//! **This runs at the export boundary, not at render.** The caller is
//! `strivo_web::render::resolve`, which both the plan route and the render
//! route go through, so a missing asset is visible in a preview that renders
//! nothing. A check that only fired once ffmpeg was already running would be
//! the defect the report names.
//!
//! ## Safe areas are enforced by the projection, not by the caller
//!
//! [`SafeArea::margin`] is a fraction per axis, so it scales with the
//! destination like everything else. [`BrandSet::apply`] raises every
//! placement's inset to at least the safe margin, which means the guarantee
//! "nothing is drawn outside the safe rect" is a property of the projection
//! and cannot be forgotten at a call site. [`SafeArea::rect`] is the same
//! number read as a pixel rectangle, for a caller that has the real output
//! geometry in hand — the geometry test in `tests/safe_area.rs` decodes a
//! rendered frame and compares against that, not against a constant written
//! next to the assertion.
//!
//! ## What is deliberately absent
//!
//! No IO, no registry, no persistence. Where the library is stored, who may
//! write it and how it is versioned are the host's business; this crate is the
//! model plus the two pure functions over it, so every refusal above is
//! unit-testable without a database or a temp directory.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::asset::{AssetKind, AssetRef};
use super::{Anchor, Banner, BannerSlot, BrandingSpec, Watermark, WatermarkSource};

/// A destination shape: the frame the output is rendered into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrandVariant {
    pub name: String,
    pub width: u32,
    pub height: u32,
}

impl BrandVariant {
    /// A variant is refused at zero on either axis rather than producing a
    /// zero-area safe rect, because "everything is inside a 0×0 box" is a
    /// vacuous guarantee and a zero frame is a data error.
    fn pixels(&self) -> Result<(u32, u32), PreconditionError> {
        if self.width == 0 || self.height == 0 {
            return Err(PreconditionError::DegenerateVariant {
                variant: self.name.clone(),
                width: self.width,
                height: self.height,
            });
        }
        Ok((self.width, self.height))
    }

    /// The short side, which is what a single proportional inset is measured
    /// against. Using the short side makes the inset conservative on the long
    /// axis too, so one number never under-protects one edge.
    fn short_side(&self) -> Result<u32, PreconditionError> {
        let (w, h) = self.pixels()?;
        Ok(w.min(h))
    }
}

/// The rectangle every branded element must stay inside, as a fraction of each
/// axis. `0.05` is a 5% inset on both edges.
///
/// A fraction rather than a pixel count because the safe rect has to mean the
/// same thing at 1080p and at 1080×1920; a pixel count would be a per-variant
/// correction, and per-variant corrections are what this module exists to
/// remove.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SafeArea {
    /// Fraction of the width, applied to the left and right edges.
    pub x: f32,
    /// Fraction of the height, applied to the top and bottom edges.
    pub y: f32,
}

impl SafeArea {
    /// No safe area: the full frame. The default so a set that says nothing
    /// about safe areas is not silently cropped, and the test that proves the
    /// safe area *moved* the overlay uses this as its control.
    pub const NONE: Self = Self { x: 0.0, y: 0.0 };

    /// Clamp a declared margin into `[0, 0.5]`. Half the frame is the
    /// degenerate limit at which the safe rect has no area; a larger value is
    /// a data error and is clamped rather than trusted, because a safe area
    /// that excludes the whole picture is not a stricter rule, it is a broken
    /// one.
    fn clamped(self) -> Self {
        const LIMIT: f32 = 0.5;
        let fix = |v: f32| {
            if v.is_finite() {
                v.clamp(0.0, LIMIT)
            } else {
                0.0
            }
        };
        Self {
            x: fix(self.x),
            y: fix(self.y),
        }
    }
}

/// A resolved safe rectangle in output pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SafeRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl SafeArea {
    /// The safe rectangle of a variant, in output pixels. The same numbers
    /// [`BrandSet::apply`] used to raise the insets, so a caller checking the
    /// render and the projection cannot disagree.
    pub fn rect(self, variant: &BrandVariant) -> Result<SafeRect, PreconditionError> {
        let (w, h) = variant.pixels()?;
        let m = self.clamped();
        let inset_x = (f64::from(w) * f64::from(m.x)).round() as u32;
        let inset_y = (f64::from(h) * f64::from(m.y)).round() as u32;
        Ok(SafeRect {
            x: inset_x,
            y: inset_y,
            width: w.saturating_sub(inset_x.saturating_mul(2)),
            height: h.saturating_sub(inset_y.saturating_mul(2)),
        })
    }

    /// The largest inset, in pixels, that a single-axis placement may use
    /// without leaving the safe rect on **either** edge.
    ///
    /// `Anchor::xy_expr` takes one inset for both axes, so the number has to
    /// clear the larger of the two margins — measuring x against the width and
    /// y against the height, not against the short side. Measuring against the
    /// short side would under-protect the long axis, which is exactly where a
    /// vertical cut would put a title-safe violation.
    ///
    /// **Ceiling.** This is necessary, not sufficient: an overlay wider than
    /// the safe rect cannot be placed inside it at any inset. That case is not
    /// detected here — the projection has no idea how large the overlay is —
    /// and it is the one way a safe area can still be violated. A set that
    /// needs the guarantee enforced for large plates has to check the rendered
    /// geometry, which is what `tests/safe_area.rs` does.
    fn min_inset_px(self, variant: &BrandVariant) -> Result<u32, PreconditionError> {
        let (w, h) = variant.pixels()?;
        let m = self.clamped();
        let by_x = f64::from(w) * f64::from(m.x);
        let by_y = f64::from(h) * f64::from(m.y);
        Ok(by_x.max(by_y).ceil() as u32)
    }
}

/// What is drawn, and how it is placed. One type for a logo, a lower-third
/// plate and a stinger, because they differ in duration and not in geometry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Overlay {
    /// A brand asset composited with `movie=` + `overlay`.
    Image { asset: AssetRef },
    /// Text drawn with `drawtext`. Carries no bytes, so it is never refused for
    /// a missing asset.
    Text { text: String },
}

/// Where a piece of branding sits. `inset` is a fraction of the axis in the
/// set's reference frame; `BrandSet::apply` turns it into pixels per variant.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub anchor: Anchor,
    pub inset: f32,
    pub opacity: f32,
}

impl Placement {
    const fn new(anchor: Anchor, inset: f32, opacity: f32) -> Self {
        Self {
            anchor,
            inset,
            opacity,
        }
    }
}

/// Which part of the output a piece of branding belongs to. Mirrors
/// [`BannerSlot`] plus a start time for the lower third, which is not a
/// bookend and so cannot be expressed as one.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "slot", rename_all = "snake_case")]
pub enum Slot {
    Intro,
    Outro,
    LowerThird { at_secs: f32 },
}

/// A timed piece of branding: a lower third, an intro, an outro.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stinger {
    pub id: String,
    pub slot: Slot,
    pub overlay: Overlay,
    pub placement: Placement,
    /// Font size in the reference frame, for a [`Overlay::Text`].
    pub font_size: u32,
    pub color: String,
    pub duration_secs: f32,
}

/// A [`Placement`] resolved against one variant: pixels rather than fractions.
struct Placement2 {
    anchor: Anchor,
    inset_px: u32,
    opacity: f32,
    font_size: u32,
}

/// The colours a set carries. Named slots rather than a free map, so "which
/// colour is the lower third?" has one answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Palette {
    pub primary: String,
    pub secondary: String,
    pub background: String,
    pub text: String,
}

/// A reusable brand set: one authored frame's worth of branding, applied to
/// any number of destination variants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrandSet {
    pub name: String,
    /// The frame the insets and font sizes below are authored in. Recorded so
    /// a set's numbers can be read without a variant in hand.
    pub reference: BrandVariant,
    pub palette: Palette,
    pub safe_area: SafeArea,
    pub font: Option<AssetRef>,
    pub music: Option<AssetRef>,
    pub watermark: Option<Stinger>,
    pub lower_third: Option<Stinger>,
    pub intro: Option<Stinger>,
    pub outro: Option<Stinger>,
}

impl BrandSet {
    /// Every asset this set depends on, in a stable order. The preconditions
    /// walk this and nothing else, so "what does this set need?" has one
    /// answer rather than one per caller.
    #[must_use]
    pub fn assets(&self) -> Vec<&AssetRef> {
        let mut out: Vec<&AssetRef> = Vec::new();
        out.extend(self.font.iter());
        out.extend(self.music.iter());
        for stinger in [
            self.watermark.as_ref(),
            self.lower_third.as_ref(),
            self.intro.as_ref(),
            self.outro.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if let Overlay::Image { asset } = &stinger.overlay {
                out.push(asset);
            }
        }
        out
    }

    /// Project this set onto a destination variant, refusing anything that
    /// would render a different picture than the operator asked for.
    ///
    /// The returned spec is the only way a [`BrandSet`] becomes a filter graph,
    /// and it is produced **only** once every asset resolved and every terms
    /// note is a decision. `Err` names the asset; there is no partial result
    /// and no "warn and carry on" variant of this function.
    pub fn apply(&self, variant: &BrandVariant) -> Result<BrandingSpec, PreconditionError> {
        let min_inset = self.safe_area.min_inset_px(variant)?;
        let short = f64::from(variant.short_side()?);
        // Insets and font sizes are authored as fractions of the set's
        // reference frame, so the projection is a scale and not a per-variant
        // table. Short side for both, which is what makes one number
        // conservative on the long axis too.
        let scale = short / f64::from(self.reference.short_side()?);

        for asset in self.assets() {
            self.require(asset)?;
        }

        // The authored inset, raised to the safe minimum, so the guarantee
        // holds even for a set authored with an inset of zero.
        let inset_px = |declared: f32| -> u32 {
            let authored = (f64::from(declared) * short).max(0.0);
            (authored.round().max(0.0) as u32).max(min_inset)
        };
        let font_px = |declared: u32| -> u32 {
            // A one-pixel floor: `drawtext` with `fontsize=0` draws nothing,
            // which would be a silently dropped title.
            ((f64::from(declared) * scale).round().max(1.0)) as u32
        };
        let placement = |stinger: &Stinger| -> Placement2 {
            Placement2 {
                anchor: stinger.placement.anchor,
                inset_px: inset_px(stinger.placement.inset),
                opacity: stinger.placement.opacity.clamp(0.0, 1.0),
                font_size: font_px(stinger.font_size),
            }
        };

        let watermark = match &self.watermark {
            Some(stinger) => {
                let at = placement(stinger);
                Some(Watermark {
                    source: match &stinger.overlay {
                        Overlay::Text { text } => WatermarkSource::Text {
                            text: text.clone(),
                            font_size: at.font_size,
                            color_rgba: stinger.color.clone(),
                        },
                        Overlay::Image { asset } => WatermarkSource::Image {
                            path: asset
                                .resolved_path()
                                .expect("every asset resolved above")
                                .to_string_lossy()
                                .into_owned(),
                        },
                    },
                    anchor: at.anchor,
                    inset_px: at.inset_px,
                    opacity: at.opacity,
                })
            }
            None => None,
        };

        // Fixed order — intro, lower thirds, outro — so one set produces the
        // same filter graph whichever way its fields were written.
        let mut banners: Vec<Banner> = Vec::new();
        for stinger in [
            self.intro.as_ref(),
            self.lower_third.as_ref(),
            self.outro.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            let Overlay::Text { text } = &stinger.overlay else {
                // An image plate is not a `drawtext` banner and the banner model
                // has no image field. Refusing it by name beats rendering the
                // text of a set that asked for a plate, which is the
                // silent-substitution defect this crate exists to end.
                return Err(PreconditionError::UnsupportedOverlay {
                    set: self.name.clone(),
                    id: stinger.id.clone(),
                });
            };
            let at = placement(stinger);
            banners.push(Banner {
                slot: match stinger.slot {
                    Slot::Intro => BannerSlot::Intro,
                    Slot::Outro => BannerSlot::Outro,
                    Slot::LowerThird { at_secs } => BannerSlot::LowerThird { at_secs },
                },
                text: text.clone(),
                font_size: at.font_size,
                color_rgba: stinger.color.clone(),
                anchor: at.anchor,
                inset_px: at.inset_px,
                duration_secs: stinger.duration_secs,
            });
        }

        Ok(BrandingSpec { watermark, banners })
    }

    /// The one place an asset is admitted. Both refusals name it.
    fn require(&self, asset: &AssetRef) -> Result<(), PreconditionError> {
        if asset.identity().is_empty() {
            return Err(PreconditionError::Unidentified {
                set: self.name.clone(),
                name: asset.name.clone(),
                kind: asset.kind,
            });
        }
        if !asset.terms_cleared() {
            return Err(PreconditionError::TermsUndecided {
                name: asset.name.clone(),
                kind: asset.kind,
            });
        }
        if asset.resolved_path().is_none() {
            return Err(PreconditionError::MissingAsset {
                name: asset.name.clone(),
                kind: asset.kind,
                locator: asset.locator.clone(),
                reason: if asset.locator.is_none() {
                    MissingReason::NoLocator
                } else {
                    MissingReason::LocatorGone
                },
            });
        }
        Ok(())
    }
}

/// A library of named sets and the destination variants they apply to. The
/// reusable half: many recordings reference one set by name, and no recording
/// carries its own copy of the spec.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BrandLibrary {
    pub sets: Vec<BrandSet>,
    pub variants: Vec<BrandVariant>,
}

impl BrandLibrary {
    /// The set with this name, or the refusal a caller can print. Missing is
    /// named rather than defaulted to an empty set, because an empty set is a
    /// successful export with no branding on it — the quiet outcome this todo
    /// exists to rule out.
    pub fn set(&self, name: &str) -> Result<&BrandSet, PreconditionError> {
        self.sets
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| PreconditionError::UnknownSet {
                name: name.to_string(),
                known: self.sets.iter().map(|s| s.name.clone()).collect(),
            })
    }

    /// How many asset references the whole library holds. A count rather than
    /// a set of fingerprints, because the reusability claim is "one set, one
    /// asset, many variants" — a total is what proves nothing is duplicated.
    #[must_use]
    pub fn assets_count(&self) -> usize {
        self.sets
            .iter()
            .map(BrandSet::assets)
            .map(|a| a.len())
            .sum()
    }

    /// The variant with this name.
    pub fn variant(&self, name: &str) -> Result<&BrandVariant, PreconditionError> {
        self.variants
            .iter()
            .find(|v| v.name == name)
            .ok_or_else(|| PreconditionError::UnknownVariant {
                name: name.to_string(),
                known: self.variants.iter().map(|v| v.name.clone()).collect(),
            })
    }

    /// The three destination shapes the built-in library ships. Named because
    /// a set applied to "three variants" is a claim about a real catalogue, and
    /// a test that invents its own three numbers would not be testing that.
    pub fn stri_vo() -> Self {
        let reference = BrandVariant {
            name: "reference".into(),
            width: 1920,
            height: 1080,
        };
        let variants = vec![
            reference.clone(),
            BrandVariant {
                name: "landscape".into(),
                width: 1920,
                height: 1080,
            },
            BrandVariant {
                name: "portrait".into(),
                width: 1080,
                height: 1920,
            },
            BrandVariant {
                name: "square".into(),
                width: 1080,
                height: 1080,
            },
            BrandVariant {
                name: "uhd".into(),
                width: 3840,
                height: 2160,
            },
        ];
        let set = BrandSet {
            name: "stri-vo".into(),
            reference,
            // DESIGN.md "Logo / Brand Identity Direction" and its Decisions
            // Log: cyan #00E5FF primary, amber #FFB020 secondary, background
            // #1A1B26. The set's colours are the brand's own, deliberately not
            // the SPA's JellySkin tokens, which DESIGN.md says diverge.
            palette: Palette {
                primary: "#00E5FF".into(),
                secondary: "#FFB020".into(),
                background: "#1A1B26".into(),
                text: "#EEF3F8".into(),
            },
            safe_area: SafeArea { x: 0.05, y: 0.05 },
            font: None,
            music: None,
            watermark: None,
            lower_third: Some(Stinger {
                id: "lower-third".into(),
                slot: Slot::LowerThird { at_secs: 10.0 },
                overlay: Overlay::Text {
                    text: "StriVo".into(),
                },
                placement: Placement::new(Anchor::BottomLeft, 0.05, 0.9),
                font_size: 48,
                color: "#EEF3F8".into(),
                duration_secs: 5.0,
            }),
            intro: Some(Stinger {
                id: "intro".into(),
                slot: Slot::Intro,
                overlay: Overlay::Text {
                    text: "starting soon".into(),
                },
                placement: Placement::new(Anchor::TopCenter, 0.08, 1.0),
                font_size: 64,
                color: "#00E5FF".into(),
                duration_secs: 3.0,
            }),
            outro: Some(Stinger {
                id: "outro".into(),
                slot: Slot::Outro,
                overlay: Overlay::Text {
                    text: "thanks for watching".into(),
                },
                placement: Placement::new(Anchor::BottomCenter, 0.08, 1.0),
                font_size: 56,
                color: "#FFB020".into(),
                duration_secs: 5.0,
            }),
        };
        Self {
            sets: vec![set],
            variants,
        }
    }
}

/// Why an asset's bytes are not there. Kept apart because the two are
/// different operator actions: find the file, or remove the reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingReason {
    /// The reference names no locator at all.
    NoLocator,
    /// A locator was recorded and nothing is at it.
    LocatorGone,
}

/// A recording's binding to the reusable library: which set, and which
/// destination shape this recording is cut as.
///
/// This is the whole per-recording document when a recording uses the library,
/// and the proof that the *spec* is not duplicated: two bytes of names where
/// the previous model stored a whole overlay spec per recording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingBrand {
    pub set: String,
    pub variant: String,
}

/// Every way a brand asset is named in a spec this crate can reach, so the
/// per-recording path is checked by the same code as the library path.
fn inline_assets(spec: &BrandingSpec) -> Vec<(&str, &str)> {
    let mut out: Vec<(&str, &str)> = Vec::new();
    if let Some(WatermarkSource::Image { path }) = spec.watermark.as_ref().map(|w| &w.source) {
        out.push((path.as_str(), path));
    }
    out
}

/// Check a spec that was written per recording rather than projected from a
/// set. An inline spec carries a **path** and no fingerprint, so the identity
/// rule cannot apply to it and the name a refusal prints is the path the
/// operator typed — which is the most specific thing available.
///
/// A `drawtext` overlay has no bytes to lose and is never refused here.
pub fn preflight_spec(spec: &BrandingSpec) -> Result<(), PreconditionError> {
    for (name, path) in inline_assets(spec) {
        if !std::path::Path::new(path).exists() {
            return Err(PreconditionError::MissingAsset {
                name: name.to_string(),
                kind: AssetKind::Logo,
                locator: Some(std::path::PathBuf::from(path)),
                reason: MissingReason::LocatorGone,
            });
        }
    }
    Ok(())
}

/// The one entry a host uses to turn a recording's branding into a spec.
///
/// * with a binding, the library is the source of truth: the set is projected
///   onto the named variant and the projection is what runs;
/// * without one, the per-recording spec is used, as before.
///
/// Either way the result passed [`preflight_spec`]'s equivalent first, so
/// there is no path through this function that yields a spec naming an asset
/// that is not there. The caller runs this **before** it starts a render, which
/// is what makes the problem visible in a preview that renders nothing.
pub fn resolve(
    library: &BrandLibrary,
    binding: Option<&RecordingBrand>,
    inline: &BrandingSpec,
) -> Result<BrandingSpec, PreconditionError> {
    match binding {
        Some(binding) => library
            .set(&binding.set)?
            .apply(library.variant(&binding.variant)?),
        None => {
            preflight_spec(inline)?;
            Ok(inline.clone())
        }
    }
}

/// Everything this crate refuses, as one function's worth of variants.
///
/// Typed because each is a different thing the operator has to do, and one
/// opaque "branding is invalid" would collapse them into a single triage item.
#[derive(Debug, Clone, PartialEq)]
pub enum PreconditionError {
    /// An asset the set depends on has no reachable bytes. **The named
    /// refusal this todo is about.**
    MissingAsset {
        name: String,
        kind: AssetKind,
        locator: Option<std::path::PathBuf>,
        reason: MissingReason,
    },
    /// The asset's terms were never recorded. Blocks, like a missing asset,
    /// and is kept apart because the fix is a note, not a file.
    TermsUndecided { name: String, kind: AssetKind },
    /// The asset has no content fingerprint, so it has no identity and cannot
    /// be deduplicated or re-attached after a move.
    Unidentified {
        set: String,
        name: String,
        kind: AssetKind,
    },
    /// The library has no set with this name. The known names travel with the
    /// refusal so a caller does not have to go looking.
    UnknownSet { name: String, known: Vec<String> },
    /// The library has no variant with this name.
    UnknownVariant { name: String, known: Vec<String> },
    /// A zero-area destination frame.
    DegenerateVariant {
        variant: String,
        width: u32,
        height: u32,
    },
    /// A lower third, intro or outro authored as an image plate, which the
    /// `drawtext` banner model cannot express. Named rather than rendered as
    /// its text.
    UnsupportedOverlay { set: String, id: String },
    /// The brand library exists and does not parse. Refused rather than treated
    /// as absent: falling back to "no branding" is a render that silently
    /// drops every logo in the set.
    UnreadableLibrary { path: PathBuf, detail: String },
    /// A recording's binding to the library exists and does not parse. Same
    /// reasoning as [`Self::UnreadableLibrary`].
    UnreadableBinding {
        recording_id: String,
        detail: String,
    },
}

impl fmt::Display for PreconditionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingAsset {
                name,
                kind,
                locator,
                reason,
            } => {
                let kind_noun = kind.noun();
                match (reason, locator) {
                    (MissingReason::NoLocator, _) => write!(
                        f,
                        "the {kind_noun} {name:?} names no file, so it cannot be drawn; \
                     point it at one or drop it from the set"
                    ),
                    (MissingReason::LocatorGone, Some(path)) => write!(
                        f,
                        "the {kind_noun} {name:?} is missing: nothing is at {}",
                        path.display()
                    ),
                    (MissingReason::LocatorGone, None) => write!(
                        f,
                        "the {kind_noun} {name:?} is missing and has no file to look for"
                    ),
                }
            }
            Self::TermsUndecided { name, kind } => {
                let kind_noun = kind.noun();
                write!(
                    f,
                    "the {kind_noun} {name:?} has no recorded terms, so it cannot be exported; \
                 record them on the asset"
                )
            }
            Self::Unidentified { set, name, kind } => {
                let kind_noun = kind.noun();
                write!(
                    f,
                    "set {set:?} references the {kind_noun} {name:?}, which has no content \
                 fingerprint and therefore no identity"
                )
            }
            Self::UnknownSet { name, known } => write!(
                f,
                "no brand set named {name:?}; the library has {}",
                if known.is_empty() {
                    "none".to_string()
                } else {
                    known.join(", ")
                }
            ),
            Self::UnknownVariant { name, known } => write!(
                f,
                "no destination variant named {name:?}; the library has {}",
                if known.is_empty() {
                    "none".to_string()
                } else {
                    known.join(", ")
                }
            ),
            Self::DegenerateVariant {
                variant,
                width,
                height,
            } => write!(
                f,
                "variant {variant:?} is {width}×{height}, which has no area to keep \
                 branding inside"
            ),
            Self::UnsupportedOverlay { set, id } => write!(
                f,
                "set {set:?} draws {id:?} as an image plate, which the overlay model \
                 cannot express as a title"
            ),
            Self::UnreadableLibrary { path, detail } => write!(
                f,
                "the brand library at {} does not parse, so no set can be read from \
                 it: {detail}",
                path.display()
            ),
            Self::UnreadableBinding {
                recording_id,
                detail,
            } => write!(
                f,
                "recording {recording_id}'s brand-set binding does not parse: {detail}"
            ),
        }
    }
}

impl std::error::Error for PreconditionError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::AssetTerms;
    use std::fs;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("strivo-brand-lib-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// A cleared logo that exists.
    fn live_logo(dir: &std::path::Path) -> AssetRef {
        let path = dir.join("logo.png");
        fs::write(&path, b"png").expect("write logo");
        AssetRef {
            name: "primary mark".into(),
            kind: AssetKind::Logo,
            fingerprint: "sha256:logo".into(),
            locator: Some(path),
            terms: AssetTerms::Cleared {
                note: "mine".into(),
            },
        }
    }

    /// A set whose only asset is `logo`, with a 5% safe area.
    fn set_with(logo: AssetRef) -> BrandSet {
        let mut set = BrandLibrary::stri_vo().sets.remove(0);
        set.name = "under test".into();
        set.watermark = Some(Stinger {
            id: "watermark".into(),
            slot: Slot::Outro,
            overlay: Overlay::Image { asset: logo },
            placement: Placement::new(Anchor::BottomRight, 0.0, 1.0),
            font_size: 0,
            color: "#FFFFFF".into(),
            duration_secs: 1.0,
        });
        set.lower_third = None;
        set.intro = None;
        set.outro = None;
        set
    }

    fn variant(name: &str, width: u32, height: u32) -> BrandVariant {
        BrandVariant {
            name: name.into(),
            width,
            height,
        }
    }

    #[test]
    fn one_set_applies_to_three_destination_shapes() {
        let dir = tmpdir("three");
        let lib = BrandLibrary {
            sets: vec![set_with(live_logo(&dir))],
            ..BrandLibrary::stri_vo()
        };
        let set = lib.set("under test").expect("set");

        // Reuse, proven rather than asserted: the SAME asset identity in all
        // three projections, and a different pixel budget in each — computed
        // from that destination's own frame, not read from a table.
        let mut insets = Vec::new();
        let mut rects = Vec::new();
        for name in ["landscape", "portrait", "square"] {
            let variant = lib.variant(name).expect("the built-in variants");
            let spec = set.apply(variant).expect("apply");
            let watermark = spec.watermark.as_ref().expect("watermark");
            assert!(
                matches!(&watermark.source, WatermarkSource::Image { path } if path.ends_with("logo.png")),
                "{name} lost the logo"
            );
            insets.push(watermark.inset_px);
            rects.push(set.safe_area.rect(variant).expect("rect"));
        }

        // Three destinations, three different safe rects, from one set.
        assert_ne!(rects[0], rects[1], "landscape and portrait share a frame");
        assert_ne!(rects[1], rects[2], "portrait and square share a frame");
        assert_ne!(rects[0], rects[2]);
        // The single inset clears the larger of the two margins in each case.
        for (inset, rect) in insets.iter().zip(&rects) {
            assert!(
                *inset >= rect.x.max(rect.y),
                "inset {inset} does not clear the safe rect {rect:?}"
            );
        }
        // Landscape's binding margin is the horizontal one; portrait's is the
        // vertical one. Same set, different number, because the frame differs.
        // `>=` and not `==`: the inset is rounded *up* out of the margin, so it
        // may overshoot by a pixel, and an overshoot is the safe direction.
        assert!(insets[0] >= rects[0].x, "{} vs {:?}", insets[0], rects[0]);
        assert!(insets[1] >= rects[1].y, "{} vs {:?}", insets[1], rects[1]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_per_variant_copy_of_the_spec_is_not_needed() {
        // The thing this todo removed. One set, one asset, three variants, and
        // no variant carries a spec of its own: the library holds a single
        // BrandSet and the pixel budgets are computed, not stored.
        let dir = tmpdir("nocopy");
        let lib = BrandLibrary {
            sets: vec![set_with(live_logo(&dir))],
            ..BrandLibrary::stri_vo()
        };
        assert_eq!(lib.sets.len(), 1, "one reusable set, not one per variant");
        assert_eq!(lib.assets_count(), 1, "one asset, referenced once");
        let set = &lib.sets[0];
        let insets: Vec<u32> = lib
            .variants
            .iter()
            .map(|v| set.apply(v).expect("apply").watermark.expect("w").inset_px)
            .collect();
        // Every inset is derived from its own variant, so no two entries of a
        // stored table could have been reused.
        for (variant, inset) in lib.variants.iter().zip(&insets) {
            let rect = set.safe_area.rect(variant).expect("rect");
            assert!(*inset >= rect.x.max(rect.y));
        }
        // A 4K destination gets twice the budget of a 1080p one, from the same
        // set — which is the scale, not a fourth row of a table.
        let uhd = lib.variant("uhd").expect("the 4K variant");
        let landscape = lib.variant("landscape").expect("landscape");
        let at = |v: &BrandVariant| set.apply(v).expect("apply").watermark.expect("w").inset_px;
        assert!(at(uhd) > at(landscape), "4K must not get a 1080p budget");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_safe_area_scales_with_the_destination_frame() {
        let dir = tmpdir("scale");
        let set = set_with(live_logo(&dir));
        let lib = BrandLibrary {
            sets: vec![],
            variants: vec![],
        };
        let landscape = variant("landscape", 1920, 1080);
        let uhd = variant("uhd", 3840, 2160);
        // The authored inset was 0.0, so the safe area is the only thing that
        // could have produced a non-zero inset.
        let at = |v: &BrandVariant| {
            set.apply(v)
                .expect("apply")
                .watermark
                .expect("watermark")
                .inset_px
        };
        assert!(
            at(&landscape) >= 96,
            "5% of 1920 is 96; got {}",
            at(&landscape)
        );
        assert!(at(&uhd) >= 192, "5% of 3840 is 192; got {}", at(&uhd));
        assert_eq!(lib.sets.len(), 0);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_set_with_no_safe_area_leaves_the_inset_where_it_was_authored() {
        // The control the geometry test needs: with no safe area the same set
        // places the overlay flush to the corner, so a later measurement of
        // "inside the safe rect" cannot pass by accident.
        let dir = tmpdir("nosafe");
        let mut set = set_with(live_logo(&dir));
        set.safe_area = SafeArea::NONE;
        let v = variant("landscape", 1920, 1080);
        let spec = set.apply(&v).expect("apply");
        assert_eq!(spec.watermark.as_ref().expect("watermark").inset_px, 0);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_logo_is_refused_by_name() {
        let dir = tmpdir("missing");
        let set = set_with(live_logo(&dir));
        fs::remove_file(dir.join("logo.png")).expect("delete the logo");
        let v = variant("landscape", 1920, 1080);
        let err = set
            .apply(&v)
            .expect_err("a deleted logo must refuse the export");
        match &err {
            PreconditionError::MissingAsset {
                name,
                kind,
                reason,
                locator,
            } => {
                assert_eq!(name, "primary mark");
                assert_eq!(*kind, AssetKind::Logo);
                assert_eq!(*reason, MissingReason::LocatorGone);
                assert!(locator.is_some());
            }
            other => panic!("expected a named missing asset, got {other:?}"),
        }
        assert!(err.to_string().contains("primary mark"), "{err}");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_logo_with_no_locator_is_refused_and_distinguished_from_a_deleted_one() {
        let dir = tmpdir("nolocator");
        let mut logo = live_logo(&dir);
        fs::remove_file(dir.join("logo.png")).ok();
        logo.locator = None;
        let err = set_with(logo)
            .apply(&variant("landscape", 1920, 1080))
            .expect_err("refuse");
        assert!(matches!(
            err,
            PreconditionError::MissingAsset {
                reason: MissingReason::NoLocator,
                ..
            }
        ));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_asset_with_no_recorded_terms_is_refused_by_name() {
        let dir = tmpdir("terms");
        let mut logo = live_logo(&dir);
        logo.terms = AssetTerms::Undecided;
        let err = set_with(logo)
            .apply(&variant("landscape", 1920, 1080))
            .expect_err("refuse");
        assert!(matches!(err, PreconditionError::TermsUndecided { .. }));
        assert!(err.to_string().contains("primary mark"), "{err}");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_asset_with_no_fingerprint_has_no_identity_and_is_refused() {
        let dir = tmpdir("noid");
        let mut logo = live_logo(&dir);
        logo.fingerprint = String::new();
        let err = set_with(logo)
            .apply(&variant("landscape", 1920, 1080))
            .expect_err("refuse");
        assert!(matches!(err, PreconditionError::Unidentified { .. }));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_music_bed_is_also_refused_when_missing_even_though_nothing_draws_it() {
        // The music bed has no overlay in the current spec model, but it is
        // still an export precondition: a set that names a bed the export will
        // not have is a set the operator has not got.
        let dir = tmpdir("music");
        let mut set = set_with(live_logo(&dir));
        set.music = Some(AssetRef {
            name: "intro stinger".into(),
            kind: AssetKind::Music,
            fingerprint: "sha256:music".into(),
            locator: Some(dir.join("gone.flac")),
            terms: AssetTerms::Cleared {
                note: "licensed".into(),
            },
        });
        let err = set
            .apply(&variant("landscape", 1920, 1080))
            .expect_err("refuse");
        assert!(err.to_string().contains("intro stinger"), "{err}");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unknown_set_and_an_unknown_variant_name_themselves() {
        let lib = BrandLibrary::stri_vo();
        let err = lib.set("nope").expect_err("no such set");
        assert!(err.to_string().contains("stri-vo"), "{err}");
        let err = lib.variant("nope").expect_err("no such variant");
        assert!(err.to_string().contains("portrait"), "{err}");
    }

    #[test]
    fn a_zero_area_variant_is_refused_rather_than_producing_a_vacuous_safe_rect() {
        let set = BrandLibrary::stri_vo().sets.remove(0);
        let err = set.apply(&variant("broken", 0, 1080)).expect_err("refuse");
        assert!(matches!(err, PreconditionError::DegenerateVariant { .. }));
    }

    #[test]
    fn a_banner_authored_as_an_image_plate_is_refused_not_rendered_as_its_text() {
        let dir = tmpdir("plate");
        let mut set = set_with(live_logo(&dir));
        set.lower_third = Some(Stinger {
            id: "plate".into(),
            slot: Slot::LowerThird { at_secs: 1.0 },
            overlay: Overlay::Image {
                asset: live_logo(&dir),
            },
            placement: Placement::new(Anchor::BottomLeft, 0.05, 1.0),
            font_size: 40,
            color: "#FFFFFF".into(),
            duration_secs: 4.0,
        });
        let err = set
            .apply(&variant("landscape", 1920, 1080))
            .expect_err("refuse");
        assert!(err.to_string().contains("plate"), "{err}");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_text_overlay_is_never_refused_for_an_asset_it_does_not_use() {
        // The control for the whole gate: a set with no assets at all must
        // still produce a spec, or "refuses everything" would satisfy every
        // refusal test above.
        let set = BrandLibrary::stri_vo().sets.remove(0);
        let spec = set.apply(&variant("landscape", 1920, 1080)).expect("apply");
        let chain = spec.build_filter_chain("[0:v]").filter_complex;
        assert!(chain.contains("drawtext"), "{chain}");
        assert!(spec.watermark.is_none());
    }

    #[test]
    fn the_font_size_never_reaches_zero() {
        // `fontsize=0` draws nothing: a silently dropped title.
        let mut set = BrandLibrary::stri_vo().sets.remove(0);
        set.intro.as_mut().expect("intro").font_size = 0;
        let spec = set.apply(&variant("square", 1080, 1080)).expect("apply");
        for banner in &spec.banners {
            assert!(banner.font_size >= 1, "font_size {}", banner.font_size);
        }
    }

    #[test]
    fn banners_are_ordered_intro_first_regardless_of_declaration_order() {
        let mut set = BrandLibrary::stri_vo().sets.remove(0);
        set.lower_third = None;
        set.outro = Some(Stinger {
            id: "outro".into(),
            slot: Slot::Outro,
            overlay: Overlay::Text { text: "bye".into() },
            placement: Placement::new(Anchor::BottomCenter, 0.05, 1.0),
            font_size: 40,
            color: "#FFFFFF".into(),
            duration_secs: 4.0,
        });
        let spec = set.apply(&variant("landscape", 1920, 1080)).expect("apply");
        let slots: Vec<BannerSlot> = spec.banners.iter().map(|b| b.slot.clone()).collect();
        assert_eq!(slots, vec![BannerSlot::Intro, BannerSlot::Outro]);
    }

    #[test]
    fn a_safe_area_margin_beyond_half_the_frame_is_clamped_not_trusted() {
        let rect = SafeArea { x: 4.0, y: -1.0 }
            .rect(&variant("v", 1000, 1000))
            .expect("rect");
        assert_eq!(
            rect,
            SafeRect {
                x: 500,
                y: 0,
                width: 0,
                height: 1000
            }
        );
    }

    #[test]
    fn a_non_finite_margin_is_treated_as_no_margin() {
        let rect = SafeArea {
            x: f32::NAN,
            y: f32::INFINITY,
        }
        .rect(&variant("v", 1000, 1000))
        .expect("rect");
        assert_eq!(
            rect,
            SafeArea::NONE
                .rect(&variant("v", 1000, 1000))
                .expect("rect")
        );
    }

    #[test]
    fn assets_are_listed_in_a_stable_order_with_the_font_and_music_first() {
        let dir = tmpdir("order");
        let mut set = set_with(live_logo(&dir));
        set.font = Some(AssetRef {
            name: "display".into(),
            kind: AssetKind::Font,
            fingerprint: "sha256:font".into(),
            locator: Some(dir.join("font.otf")),
            terms: AssetTerms::Cleared {
                note: "open licence".into(),
            },
        });
        fs::write(dir.join("font.otf"), b"otf").ok();
        let kinds: Vec<AssetKind> = set.assets().iter().map(|a| a.kind).collect();
        assert_eq!(kinds, vec![AssetKind::Font, AssetKind::Logo]);
        fs::remove_dir_all(&dir).ok();
    }
}
