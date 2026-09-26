//! Destination reframing and streamer layouts — report item E06.
//!
//! `docs/CREATOR-EDITION-REPORT-2026-09-07.md:104` records the gap this module
//! closes: "Short-form aspect strings in drafts do not themselves crop or
//! render media." A publish draft carries `aspect: "9:16"`, and until this
//! module that string reached nothing at all — the render emitted the source's
//! own frame size. Here the same string resolves to a typed [`AspectRatio`],
//! which owns the output frame, the crop that reframes the gameplay into it,
//! and the placement of every other region.
//!
//! ## One plan, one more stage
//!
//! Nothing here compiles or executes anything. A layout is **data** that fills
//! in three fields of todo 23's [`RenderRequest`](crate::render::RenderRequest):
//! the declared [`Frames`], the caller's `video_filter_complex` (the crop), and
//! the `transform` of each layer whose id names a region. There is no second
//! plan type, no second render path, and no new time model — durations are
//! [`MediaTime`], and every coordinate is an integer pixel count.
//!
//! ## Regions are a list, not flags
//!
//! A streamer layout is *gameplay plus a facecam, or a screen, or a demo, with
//! one speaker's camera in focus*. Modelled as booleans (`has_facecam`,
//! `has_screen`) that describes exactly one layout and cannot describe two
//! facecams and a demo. So [`RegionRole`] is a role attached to an entry in an
//! ordered [`Vec<Region>`], and a layout with two `Facecam` regions and a
//! `Demo` is expressible without inventing a third flag.
//!
//! ## What happens to content that does not fit
//!
//! **Every ratio crops to fill; none of them letterboxes.** A 4:3 gameplay
//! source into 16:9 loses rows top and bottom, into 9:16 loses columns at the
//! sides, into 1:1 loses columns at the sides. The alternative — padding to
//! letterbox — was rejected because a streamer layout is *delivered* to a feed
//! that shows the frame edge-to-edge, so bars are a visible defect rather than
//! a neutral one, and because a pad is a second fit policy to keep correct
//! against a second set of pixels. The decision is stated per ratio here and
//! asserted on decoded output in `tests/destination_layout.rs`, by measuring
//! which of four colour bands survived.
//!
//! The crop is anchored on the **focused region** when a focus is set, and on
//! the centre of the frame when it is not. That is what makes "speaker focus" a
//! layout property rather than a label: focusing a camera moves the crop.
//!
//! ## No platform duration limit lives here
//!
//! A duration cap is a property of a *destination account*, not of a ratio, and
//! todo 60 is where per-account capabilities arrive. So the cap is an injected
//! [`DestinationCapability`] that is [`None`] until someone supplies one, and
//! [`DestinationLayout::check_duration`] is a no-op without it. There is no
//! constant in this module: a layout renders at any duration when no capability
//! is present, which is asserted.
//!
//! Note that `strivo_reuse::Format::duration_cap` still hard-codes 60 s for the
//! short formats. That is pre-existing and belongs to the draft side; this
//! module neither reads it nor repeats it.
//!
//! ## The aspect string cannot survive into a render
//!
//! [`AspectRatio`] is a three-variant enum. The only function in the tree that
//! takes a draft's `&str` is [`AspectRatio::parse`], which is total over
//! `"16:9"`, `"9:16"` and `"1:1"` and returns
//! [`DestinationError::UnknownAspect`] for everything else — including the
//! `"audio"` and `"text"` that `strivo_reuse::Format::aspect` returns for the
//! podcast and blog formats, which have no video layout to resolve. No `String`
//! appears in [`AspectRatio`], [`Rect`], [`Region`] or [`DestinationLayout`],
//! and [`Frames`] is derived from the enum, so there is nowhere for a raw
//! string to ride into a plan. What that does **not** yet prove is that every
//! caller goes through `parse`: no in-tree caller resolves a draft's aspect
//! string yet, so today the enforcement is the type's surface, not a call site.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::render::{Frames, Interpolation, Keyframe, Transform};
use crate::time::MediaTime;

/// A destination's picture shape.
///
/// The three shapes a streamer layout ships in. Each owns its own frame, so a
/// layout never carries a string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AspectRatio {
    /// 16:9 — the long-form default.
    Landscape,
    /// 9:16 — short-form vertical.
    Portrait,
    /// 1:1 — square feed.
    Square,
}

impl AspectRatio {
    /// Every ratio a layout can be generated for, in a fixed order so a
    /// generated set is deterministic.
    pub const ALL: [Self; 3] = [Self::Landscape, Self::Portrait, Self::Square];

    /// The draft-side aspect string this ratio answers to.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Landscape => "16:9",
            Self::Portrait => "9:16",
            Self::Square => "1:1",
        }
    }

    /// The declared output frame, `(width, height)`.
    ///
    /// 1280×720 rather than 1920×1080: the ratio is what a destination
    /// variant is identified by, and a fixture that decodes a half-megapixel
    /// frame proves the same thing as one that decodes nine times as many
    /// pixels. Scaling up is a `Frames` value away.
    #[must_use]
    pub fn frame(self) -> (u32, u32) {
        match self {
            Self::Landscape => (1280, 720),
            Self::Portrait => (720, 1280),
            Self::Square => (720, 720),
        }
    }

    /// The declared output frame as the render request's picture shape.
    #[must_use]
    pub fn frames(self) -> Frames {
        let (width, height) = self.frame();
        Frames {
            width,
            height,
            fps_num: 30,
            fps_den: 1,
            pixel_format: "yuv420p".to_string(),
            timebase_num: 1,
            timebase_den: 1000,
        }
    }

    /// The ratio reduced to coprime terms, so the crop compares aspects
    /// without carrying a frame size around.
    #[must_use]
    pub fn reduced(self) -> (u32, u32) {
        match self {
            Self::Landscape => (16, 9),
            Self::Portrait => (9, 16),
            Self::Square => (1, 1),
        }
    }

    /// Resolve a draft's aspect string.
    ///
    /// The one door a `PublishDraft::aspect` string comes through, and it is
    /// closed to everything this module cannot render. `"audio"` and `"text"`
    /// are refused, not ignored: those drafts have no video layout, and
    /// quietly picking a default for them is how a podcast draft would end up
    /// with a 16:9 video nobody asked for.
    pub fn parse(aspect: &str) -> Result<Self, DestinationError> {
        match aspect.trim() {
            "16:9" => Ok(Self::Landscape),
            "9:16" => Ok(Self::Portrait),
            "1:1" => Ok(Self::Square),
            other => Err(DestinationError::UnknownAspect {
                found: other.to_string(),
            }),
        }
    }
}

/// Everything this module can refuse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DestinationError {
    /// An aspect string this module does not render. Named with what it found,
    /// because the caller's fix is to know which string was rejected.
    UnknownAspect { found: String },
    /// A source with no pixels cannot be cropped.
    EmptySource,
    /// A region rect that is empty or hangs off the frame.
    RegionOutOfFrame { id: String, rect: Rect },
    /// No region carries the id an override or a focus named.
    UnknownRegion { id: String },
    /// A layout with no gameplay region has nothing to reframe.
    NoGameplay,
    /// The request already carries a caller video graph. Carried verbatim so
    /// the refusal names what would have been dropped.
    CallerVideoGraph { existing: String },
    /// A duration the injected capability does not allow. **Only** reachable
    /// when a capability was supplied — see the module docs.
    DurationExceedsCapability { duration: MediaTime, max: MediaTime },
}

impl fmt::Display for DestinationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownAspect { found } => write!(
                f,
                "aspect {found:?} names no renderable layout; expected 16:9, 9:16 or 1:1"
            ),
            Self::EmptySource => write!(f, "the source has no pixels to reframe"),
            Self::RegionOutOfFrame { id, rect } => write!(
                f,
                "region {id:?} at {}x{}+{}+{} is empty or outside the frame",
                rect.w, rect.h, rect.x, rect.y
            ),
            Self::UnknownRegion { id } => write!(f, "no region named {id:?}"),
            Self::NoGameplay => write!(f, "a layout needs a gameplay region to reframe"),
            Self::CallerVideoGraph { existing } => write!(
                f,
                "the request already carries a video graph ({existing:?}); a destination \
                 layout would overwrite it"
            ),
            Self::DurationExceedsCapability { duration, max } => write!(
                f,
                "the layout is {} us long and the destination capability allows {} us",
                duration.micros(),
                max.micros()
            ),
        }
    }
}

impl std::error::Error for DestinationError {}

/// A region placed in output pixels, left/top origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    /// Whether this rect is a non-empty area wholly inside `frame`.
    #[must_use]
    pub fn inside(self, frame: (u32, u32)) -> bool {
        self.w > 0
            && self.h > 0
            && self.x >= 0
            && self.y >= 0
            && self.x + i64::from(self.w) <= i64::from(frame.0)
            && self.y + i64::from(self.h) <= i64::from(frame.1)
    }
}

/// What a region is *for*. A role, not a flag: the same role may appear more
/// than once, which is what two cameras or a screen plus a demo are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegionRole {
    /// The programme itself — the thing every other region is drawn over.
    Gameplay,
    /// A camera.
    Facecam,
    /// A screen or window share.
    Screen,
    /// A demonstration or walkthrough.
    Demo,
    /// A camera the crop follows.
    SpeakerFocus,
}

/// One placed region. The id is the join key: it names a layer in the render
/// request, and it is what a manual override and a focus refer to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub id: String,
    pub role: RegionRole,
    pub rect: Rect,
    /// Paint order, ascending. Over the gameplay lane, which is z 0.
    pub z: i32,
}

/// A destination's picture shape and the regions drawn into it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DestinationLayout {
    pub aspect: AspectRatio,
    /// Paint order, ascending by `z` then by id.
    pub regions: Vec<Region>,
    /// The region whose centre the crop follows, if any.
    pub focus: Option<String>,
    /// Manual overrides, by region id.
    ///
    /// Carried **inside** the layout rather than beside it, because a
    /// regeneration is a method on the layout: the override set is the part of
    /// a layout that is the user's, and everything else is derived from the
    /// ratio and is free to be recomputed.
    pub overrides: BTreeMap<String, Rect>,
}

/// A per-account destination constraint, injected. Empty until todo 60 supplies
/// one; see the module docs for why nothing here holds a constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestinationCapability {
    /// Longest output this account may publish. `None` = no limit stated.
    pub max_duration: Option<MediaTime>,
}

/// A source picture's size, as the crop needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceSize {
    pub width: u32,
    pub height: u32,
}

impl DestinationLayout {
    /// The default layout for a ratio: gameplay filling the frame plus one
    /// facecam, placed where a streamer's camera conventionally sits.
    #[must_use]
    pub fn for_aspect(aspect: AspectRatio) -> Self {
        let (width, height) = aspect.frame();
        let (w, h) = match aspect {
            // A quarter of the frame in each axis.
            AspectRatio::Landscape => (width / 4, height / 4),
            // A wide band across the top, which is where a vertical feed puts
            // the camera when the gameplay is the whole frame.
            AspectRatio::Portrait => (width * 4 / 5, height / 4),
            // A square camera a third of the frame.
            AspectRatio::Square => (height / 3, height / 3),
        };
        // Top-right in every ratio, inset by a fixed margin.
        let margin = 32.min(i64::from(height / 4));
        let rect = Rect {
            x: i64::from(width) - i64::from(w) - margin,
            y: margin,
            w,
            h,
        };
        Self {
            aspect,
            regions: vec![
                Region {
                    id: "gameplay".to_string(),
                    role: RegionRole::Gameplay,
                    rect: Rect {
                        x: 0,
                        y: 0,
                        w: width,
                        h: height,
                    },
                    z: 0,
                },
                Region {
                    id: "facecam".to_string(),
                    role: RegionRole::Facecam,
                    rect,
                    z: 10,
                },
            ],
            focus: None,
            overrides: BTreeMap::new(),
        }
    }

    /// Generate a layout for a draft's aspect string, and check it against the
    /// account's capability.
    ///
    /// The single entry point from the draft side, and the reason a draft
    /// aspect string cannot be silently ignored: an unrenderable string is an
    /// error here, before any media is touched, and a duration the account may
    /// not publish is an error here too rather than a rejection after a
    /// three-hour render.
    ///
    /// # Errors
    ///
    /// [`DestinationError::UnknownAspect`] for a string this module does not
    /// render, and [`DestinationError::DurationExceedsCapability`] when a
    /// capability was supplied and the duration exceeds it. With no capability
    /// there is no limit to exceed, and the layout is returned.
    pub fn from_draft_aspect(
        aspect: &str,
        duration: MediaTime,
        capability: Option<&DestinationCapability>,
    ) -> Result<Self, DestinationError> {
        let layout = Self::for_aspect(AspectRatio::parse(aspect)?);
        layout.check_duration(duration, capability)?;
        Ok(layout)
    }

    /// Regenerate the layout for `aspect`, keeping every manual override.
    ///
    /// The override set is the user's; the geometry is derived. An override is
    /// **applied** to the regenerated layout when it fits the new frame, and
    /// is kept in [`Self::overrides`] either way — so a round trip through a
    /// ratio that moves the frame and back does not lose the placement, and an
    /// override that no longer fits is not silently clamped onto the frame.
    #[must_use]
    pub fn regenerate(&self, aspect: AspectRatio) -> Self {
        let mut next = Self::for_aspect(aspect);
        next.overrides = self.overrides.clone();
        let frame = aspect.frame();
        for region in &mut next.regions {
            if let Some(rect) = next.overrides.get(&region.id).copied() {
                if rect.inside(frame) {
                    region.rect = rect;
                }
            }
        }
        next
    }

    /// Move one region by hand. The override is recorded, so it survives every
    /// later regeneration.
    pub fn override_region(&mut self, id: &str, rect: Rect) -> Result<(), DestinationError> {
        if !rect.inside(self.aspect.frame()) {
            return Err(DestinationError::RegionOutOfFrame {
                id: id.to_string(),
                rect,
            });
        }
        if !self.regions.iter().any(|r| r.id == id) {
            return Err(DestinationError::UnknownRegion { id: id.to_string() });
        }
        if let Some(region) = self.regions.iter_mut().find(|r| r.id == id) {
            region.rect = rect;
        }
        self.overrides.insert(id.to_string(), rect);
        Ok(())
    }

    /// Focus the crop on a region.
    pub fn set_focus(&mut self, id: Option<&str>) -> Result<(), DestinationError> {
        if let Some(id) = id {
            if !self.regions.iter().any(|r| r.id == id) {
                return Err(DestinationError::UnknownRegion { id: id.to_string() });
            }
        }
        self.focus = id.map(str::to_string);
        Ok(())
    }

    /// The gameplay region, or a refusal — a layout that reframes nothing has
    /// no crop to compute.
    pub fn gameplay(&self) -> Result<&Region, DestinationError> {
        self.regions
            .iter()
            .find(|r| r.role == RegionRole::Gameplay)
            .ok_or(DestinationError::NoGameplay)
    }

    /// The crop that reframes `source` into this layout, in source pixels.
    ///
    /// Crop-to-fill, even in both dimensions, anchored horizontally on the
    /// focused region when there is one and on the source centre when there is
    /// not, and clamped to the source. A vertical trim stays centred: a
    /// region's vertical centre is the frame's, so following it would move
    /// nothing.
    #[must_use]
    pub fn crop(&self, source: SourceSize) -> Rect {
        let anchor_x = self
            .focus
            .as_ref()
            .and_then(|id| self.regions.iter().find(|r| &r.id == id))
            .map_or(source.width / 2, |r| {
                // The focus names a region in *output* pixels; as a fraction of
                // the frame it is also the fraction of the source the crop
                // should centre on.
                let (fw, _) = self.aspect.frame();
                (r.rect.x + i64::from(r.rect.w) / 2).max(0) as u32 * source.width / fw
            });
        let (rw, rh) = self.aspect.reduced();
        let (mut w, mut h) = if source.width * rh > source.height * rw {
            // The source is wider than the target: full height, cropped width.
            ((source.height * rw / rh).min(source.width), source.height)
        } else {
            (source.width, (source.width * rh / rw).min(source.height))
        };
        // Even width, height AND origin: yuv420p has no odd chroma boundary,
        // and an odd `x` shifts the chroma plane against the luma plane.
        w = (w.max(2) / 2) * 2;
        h = (h.max(2) / 2) * 2;
        let x =
            i64::from((anchor_x.saturating_sub(w / 2)).min(source.width.saturating_sub(w)) / 2 * 2);
        let y = i64::from((source.height - h) / 4 * 2);
        Rect { x, y, w, h }
    }

    /// The crop plus the conform to this layout's frame, as one ffmpeg chain.
    ///
    /// The conform is spelled here rather than left to the executor's
    /// `Frames` because **the two cannot both be set**: `composite_args` builds
    /// the caller's chain *and* the conform from the same `[0:v]` label, so a
    /// request with both produces two consumers of one label and ffmpeg refuses
    /// the graph. That is a defect in `render::invoke`, which this lane does not
    /// touch; until it is fixed, a destination variant carries its own conform
    /// and leaves `Frames` unset. [`Self::aspect`]'s
    /// [`frames`](AspectRatio::frames) is still the declaration — it is what an
    /// overlay source must be authored to, and what this chain spells.
    #[must_use]
    pub fn conform_filter(&self, source: SourceSize) -> String {
        let crop = self.crop(source);
        let frames = self.aspect.frames();
        format!(
            "[0:v]crop={}:{}:{}:{},scale={}:{}:flags=bicubic,fps={}/{},format={},settb={}/{}\
             [vout]",
            crop.w,
            crop.h,
            crop.x,
            crop.y,
            frames.width,
            frames.height,
            frames.fps_num,
            frames.fps_den,
            frames.pixel_format,
            frames.timebase_num,
            frames.timebase_den
        )
    }

    /// Every non-gameplay region's transform, in paint order.
    ///
    /// The transform is a size and an offset in output pixels, which is what
    /// the executor's `overlay` expression already takes. The gameplay region
    /// is absent because it is the programme lane, not a layer drawn over it.
    #[must_use]
    pub fn region_transforms(&self) -> Vec<(&str, Transform)> {
        let mut regions: Vec<&Region> = self
            .regions
            .iter()
            .filter(|r| r.role != RegionRole::Gameplay)
            .collect();
        regions.sort_by(|a, b| a.z.cmp(&b.z).then_with(|| a.id.cmp(&b.id)));
        regions
            .into_iter()
            .map(|r| {
                (
                    r.id.as_str(),
                    Transform {
                        scale: Some((r.rect.w, r.rect.h)),
                        offset_px: (r.rect.x, r.rect.y),
                        z: r.z,
                    },
                )
            })
            .collect()
    }

    /// A one-microsecond hold, which is how a *fixed* placement is spelled.
    ///
    /// `render::invoke::segments` builds a piecewise expression from
    /// `points.windows(2)`, so a layer with no keyframes produces no segment
    /// and `piecewise` answers `0.0000` — a static non-zero overlay offset is
    /// silently drawn at the origin. Todo 48's assertions never saw it because
    /// every offset they checked was either zero or keyframed. A hold of two
    /// identical keyframes is the workaround; the defect is in
    /// `render::invoke`, which this lane does not touch, and it should be fixed
    /// there rather than worked around forever.
    fn hold(transform: Transform) -> Vec<Keyframe> {
        let at = MediaTime::from_micros(1).expect("non-negative media time");
        vec![
            Keyframe {
                at: MediaTime::ZERO,
                transform,
                interpolate: Interpolation::Linear,
            },
            Keyframe {
                at,
                transform,
                interpolate: Interpolation::Stepped,
            },
        ]
    }

    /// Fill this layout into a render request: the crop and conform chain, and
    /// the transform on every layer whose id names a region.
    ///
    /// Layers whose id names no region are left alone, so a B-roll layer
    /// composed in the same plan keeps whatever position todo 48 gave it.
    ///
    /// An overlay source must be authored at the declared frame — see
    /// [`Self::conform_filter`] for why nothing conforms it here.
    ///
    /// # Errors
    ///
    /// [`DestinationError::EmptySource`] for a source with no pixels,
    /// [`DestinationError::NoGameplay`] for a layout with no programme region,
    /// and [`DestinationError::CallerVideoGraph`] when the request already
    /// carries a video graph — a branding chain, say. Overwriting it would drop
    /// the brand from a branded render, silently.
    pub fn apply(
        &self,
        request: &mut crate::render::RenderRequest,
        source: SourceSize,
    ) -> Result<(), DestinationError> {
        if source.width == 0 || source.height == 0 {
            return Err(DestinationError::EmptySource);
        }
        self.gameplay()?;
        if !request.video.filter_complex.trim().is_empty() {
            return Err(DestinationError::CallerVideoGraph {
                existing: request.video.filter_complex.clone(),
            });
        }
        let transforms: BTreeMap<&str, Transform> = self.region_transforms().into_iter().collect();
        for layer in &mut request.layers {
            if let Some(transform) = transforms.get(layer.id.as_str()) {
                layer.transform = *transform;
                layer.keyframes = Self::hold(*transform);
            }
        }
        request.video.filter_complex = self.conform_filter(source);
        // See `conform_filter`: the executor cannot compose a caller chain and
        // a `Frames` conform in one graph, and this chain carries both.
        request.video.frames = None;
        Ok(())
    }

    /// Check a duration against an injected capability.
    ///
    /// A no-op when no capability is supplied, which is the state of the tree
    /// until todo 60 lands: a layout is not where a per-account limit lives,
    /// and inventing a constant here would make one platform's rule everyone's.
    fn check_duration(
        &self,
        duration: MediaTime,
        capability: Option<&DestinationCapability>,
    ) -> Result<(), DestinationError> {
        match capability.and_then(|c| c.max_duration) {
            Some(max) if duration > max => {
                Err(DestinationError::DurationExceedsCapability { duration, max })
            }
            _ => Ok(()),
        }
    }
}
