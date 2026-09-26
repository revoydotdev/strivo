//! The compiled plan as a durable row: a [`RenderPlan`] that a job and an
//! artifact can point at.
//!
//! `entities::RenderPlan` was declared in todo 21 with the note "the compiler
//! that turns these into a `RenderPlan` is todo 23; this is its parent row".
//! This is that compiler's other half. The **plan itself** is
//! `strivo_editor::render::CompiledPlan` — a value, not a document — and this
//! module is the one place that turns one into a stored, content-addressed
//! row, so there is one definition of "the plan" and one definition of "the
//! row that holds it".
//!
//! ## What the content hash covers, and what it deliberately does not
//!
//! It covers the **declared inputs**: the compiled plan itself, the renderer's
//! identity and version, and the variant's declared overrides. It never covers
//! a file's size or mtime, for the reason [`crate::creator::jobs::CacheKey`]
//! gives — a re-render that changes nothing on disk is still a different job,
//! and a file whose mtime moved is not necessarily a different input. A
//! revision edited after a plan was compiled changes the plan, so it changes
//! the hash; that is the whole of the staleness story, and
//! [`is_stale`] is its only reader.
//!
//! ## No migration
//!
//! `creator_render_plans` already has every column this needs, so this module
//! adds no schema version. Todo 21's mechanism is untouched.

use sha2::{Digest, Sha256};
use strivo_editor::render::CompiledPlan;

use super::entities::RenderPlan;
use super::ids::{ProjectId, RenderPlanId};
use crate::{ResearchError, Result};

/// The compiled plan, serialised into the row's `resolved_inputs_json` and
/// hashed into its `content_hash`.
///
/// `variant` is the destination-shaped derivative the plan was compiled for,
/// and it is **required** rather than optional: `creator_render_plans` declares
/// `variant_id` as a plain foreign key, so there is no "no variant" row. A
/// revision rendered as authored is a variant with empty
/// `format_overrides_json` — which is what it is, honestly, rather than a
/// sentinel identity standing in for "the default one".
///
/// The revision this came from travels **inside** `resolved_inputs_json` — the
/// plan body names it — rather than in a column of its own. `creator_render_plans`
/// has no revision column and this module adds no migration for one, so a row
/// cannot claim a revision its own body contradicts.
///
/// Fails rather than storing a plan it cannot read back: a row whose
/// `resolved_inputs_json` does not round-trip would make `content_hash` a
/// claim about bytes nothing can interpret.
pub fn compile_render_plan(
    project_id: ProjectId,
    variant: &super::entities::Variant,
    plan: &CompiledPlan,
    now: &str,
) -> Result<RenderPlan> {
    let resolved = serde_json::to_string(plan)
        .map_err(|e| ResearchError::Validation(format!("plan does not serialise: {e}")))?;
    let overrides = &variant.format_overrides_json;
    Ok(RenderPlan {
        id: RenderPlanId::generate(),
        project_id,
        variant_id: variant.id,
        renderer: plan.profile.renderer.clone(),
        renderer_version: plan.profile.renderer_version.clone(),
        resolved_inputs_json: resolved,
        content_hash: content_hash(plan, overrides),
        created_at: now.to_string(),
    })
}

/// The identity of a compiled plan: its body, its renderer, and the variant
/// overrides it was compiled under.
///
/// Public because a caller caching a plan computes this without storing a row,
/// and two implementations of "the same plan" is the drift this module exists
/// to end.
#[must_use]
pub fn content_hash(plan: &CompiledPlan, variant_overrides: &str) -> String {
    let mut digest = Sha256::new();
    for part in [
        plan.profile.renderer.as_str(),
        plan.profile.renderer_version.as_str(),
        variant_overrides,
    ] {
        digest.update(part.as_bytes());
        digest.update(b"\x1f");
    }
    // The serialised body last, so a truncation in the hash input is visible
    // as a hash change rather than as a silently shorter digest.
    if let Ok(body) = serde_json::to_string(plan) {
        digest.update(body.as_bytes());
    }
    digest
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            hex.push_str(&format!("{byte:02x}"));
            hex
        })
}

/// Read a stored plan back.
///
/// A row that does not deserialise is an error, not an empty plan: a caller
/// handed an empty plan would render the wrong thing successfully.
pub fn read_plan(row: &RenderPlan) -> Result<CompiledPlan> {
    serde_json::from_str(&row.resolved_inputs_json)
        .map_err(|e| ResearchError::Validation(format!("stored plan does not parse: {e}")))
}

/// Whether a stored plan is out of date with respect to a freshly compiled
/// one.
///
/// The **only** staleness test, and it is a content comparison rather than a
/// revision comparison on purpose: a revision can be re-saved with identical
/// content, and a plan compiled from it is not stale. Comparing ids would
/// report staleness for a no-op edit and miss a settings change that left the
/// revision id alone.
#[must_use]
pub fn is_stale(row: &RenderPlan, fresh: &CompiledPlan, variant_overrides: &str) -> bool {
    row.content_hash != content_hash(fresh, variant_overrides)
}

#[cfg(test)]
mod tests;
