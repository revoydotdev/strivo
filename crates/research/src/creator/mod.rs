//! Creator-owned identity: the canonical project, source, composition,
//! variant and artifact model.
//!
//! From `docs/CREATOR-EDITION-REPORT-2026-09-07.md:179-188` (F04, F08). The
//! report proposes these contracts; this module is where they stop being
//! proposals.
//!
//! **Two identity rules carry the whole model.**
//!
//! 1. A path is a resolvable location, never an identity. [`SourceAsset`]
//!    and [`Artifact`] keep a `locator`, but the key is the typed id and the
//!    content identity is the fingerprint. Nothing is ever looked up *by*
//!    its path, and no unique constraint exists on one, so moving a file
//!    cannot change or duplicate an entity.
//! 2. The research kernel's [`crate::Project`] is **linked, not redefined**.
//!    A Creator project and a research project are separate rows in separate
//!    models, related many-to-many by stable ID through
//!    [`Store::link_research_project`]. There is no foreign key between them
//!    and no cardinality constraint, because a research project spanning two
//!    creator projects (a cross-recording compilation) and one creator
//!    project linking several research projects are both legitimate.
//!
//! **The edition boundary is the dependency, not a `cfg` in this crate.**
//! `strivo-research` is an optional dependency of `strivo-web` that only the
//! `creator` feature activates, so a PVR build does not compile this module
//! at all — strictly stronger than compiling it and hiding it. The
//! `#[cfg(feature = "creator")]` that creates the tables lives on the one
//! call site, in `strivo_web::creator_schema`, and a PVR install has no code
//! path to any statement in [`schema`]. A default-on `creator` feature here
//! would have been a knob nobody could turn off, and a default-off one would
//! have silently removed these tests from `cargo test --workspace`.
//!
//! **One time model, and it is not defined here.** [`MediaTime`] is
//! `strivo_editor::time::MediaTime` — integer microseconds, the single
//! representation every Creator timestamp uses. It is re-exported rather
//! than re-wrapped, because a second newtype over the same integer is a
//! second time model wearing a different name. Schema version 2 replaced
//! version 1's `duration_ms` columns with `duration_us` and **dropped** the
//! old ones: a database that carried both would have two answers to "how
//! long is this source". The `f32` seconds in the legacy edit decision list
//! are read at exactly one named boundary, `strivo_editor::time::Cut::span`.
//!
//! The render compiler (todo 23) is here, in [`render_plan`], and its plan is
//! `strivo_editor::render::CompiledPlan` — a value, not a document. This module
//! owns the one function that stores one as a [`RenderPlan`] row, so there is
//! one definition of "the plan" and one of "the row that holds it". Where a
//! revision field still belongs to the compiler it is stored as an opaque
//! document (`timeline_json`), and it is said so at the field.
//!
//! **The job and artifact lifecycle (todo 24) is here, in [`jobs`], and it is
//! the pipeline's state machine rather than a second one.** Its state names
//! are `strivo_core::pipeline::stage::StageState`'s, re-declared because this
//! crate sits *below* the daemon in the dependency graph and cannot import
//! them; the correspondence is pinned by a test that drives the real
//! `PipelineRegistry`. See [`jobs`]'s module doc for the full argument.

mod entities;
mod ids;
mod jobs;
mod render_plan;
mod schema;
mod store;
mod store_delivery;
mod store_jobs;
mod store_sources;
mod store_time;

#[cfg(test)]
mod tests;

pub use entities::{
    Artifact, Availability, CaptureSession, CompositionRevision, MetricObservation, Project,
    Publication, ReleasePackage, RenderPlan, SourceAsset, TranscriptRevision, Variant,
};
pub use ids::{
    ArtifactId, CaptureSessionId, CompositionRevisionId, JobId, MetricObservationId, ProjectId,
    PublicationId, ReleasePackageId, RenderPlanId, ResearchProjectId, SourceAssetId,
    TranscriptRevisionId, VariantId,
};
pub use jobs::{
    immutable_output_path, is_legal, CacheKey, Job, JobInputs, JobState, JobTransition,
    MAX_ATTEMPTS, TRANSITIONS,
};
pub use render_plan::{compile_render_plan, content_hash, is_stale, read_plan};
pub use schema::{latest_version, rollback, Migration, MIGRATIONS, TABLES, VERSION_KEY};
pub use store::Store;
pub use store_jobs::{MoveJob, NewJob};
pub use store_time::ClockSegmentRow;

use std::path::Path;

pub use strivo_editor::time::MediaTime;

use crate::Result;

/// Bring the Creator schema up to date in `db_path`, creating the file and
/// its parent directory if needed. Idempotent: opening an already-current
/// database applies nothing.
///
/// This is the only entry point a running daemon uses, and the only reason
/// a Creator install has these tables at all.
pub fn ensure_schema(db_path: &Path) -> Result<i64> {
    Store::open(db_path)?.schema_version()
}
