//! What a preview cannot show, derived from the plan.
//!
//! Its own file because the property that matters is negative: nothing in here
//! may end up **on** the plan. An approximation field on the plan would be a
//! second answer to "what does this render as" for preview and export to
//! disagree about, and this module is where that is stated and tested.

use super::plan::CompiledPlan;
use super::request::CaptionMode;

/// One thing a preview shows that is not the render, and why.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Approximation {
    /// Which stage of the plan is approximate.
    pub stage: String,
    /// Why, in one sentence.
    pub reason: String,
}

/// What a non-executing preview cannot show about this plan.
///
/// **Derived from the plan, never stored in it.** An approximation list *on*
/// the plan would give the two callers a field to disagree on, which is the
/// exact failure this module makes impossible. Both callers read this
/// function; neither can set an approximation, only read the ones the plan
/// implies.
#[must_use]
pub fn approximations(plan: &CompiledPlan) -> Vec<Approximation> {
    let mut out = Vec::new();
    if plan.caption_mode == CaptionMode::BurnedIn {
        // Burned-in glyphs raster with whatever fonts the host has, so a
        // preview cannot promise the render's pixels. Labelled, not silently
        // different.
        out.push(Approximation {
            stage: "captions".into(),
            reason: "burned-in glyphs raster with the preview host's fonts".into(),
        });
    }
    if plan.loudness.is_some() {
        // Two-pass loudnorm reads the finished signal; a preview that has not
        // rendered it cannot report the achieved value.
        out.push(Approximation {
            stage: "loudness".into(),
            reason: "loudnorm measures the finished signal, which a preview has not produced"
                .into(),
        });
    }
    if plan.container.stream_copy {
        out.push(Approximation {
            stage: "container".into(),
            reason: "the plan stream-copies, so the preview shows the source's own quality".into(),
        });
    }
    out
}
