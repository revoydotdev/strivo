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
//! What is deliberately **not** here yet: the time model (todo 22 owns
//! `ClockMap` and the interval map), the render compiler (todo 23), and the
//! job/artifact lifecycle columns (todo 24). Where a field belongs to one of
//! those it is either stored as an opaque document or left out, and it is
//! said so at the field.

mod entities;
mod ids;
mod schema;
mod store;
mod store_delivery;
mod store_sources;

#[cfg(test)]
mod tests;

pub use entities::{
    Artifact, Availability, CaptureSession, CompositionRevision, MetricObservation, Project,
    Publication, ReleasePackage, RenderPlan, SourceAsset, TranscriptRevision, Variant,
};
pub use ids::{
    ArtifactId, CaptureSessionId, CompositionRevisionId, MetricObservationId, ProjectId,
    PublicationId, ReleasePackageId, RenderPlanId, ResearchProjectId, SourceAssetId,
    TranscriptRevisionId, VariantId,
};
pub use schema::{latest_version, rollback, Migration, MIGRATIONS, TABLES, VERSION_KEY};
pub use store::Store;

use std::path::Path;

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
