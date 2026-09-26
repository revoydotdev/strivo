//! The eleven Creator-owned records, as they are stored.
//!
//! Each is a row of the same name in [`crate::creator::schema`]. Document
//! bodies (a timeline, a resolved render input set, a transcript) are stored
//! as JSON text: they are versioned documents, not queryable relations, and
//! todo 22/23 replace the ones they own with typed columns and a compiler.
//!
//! Fields that are **derivable are not stored** — see
//! [`SourceAsset::availability`], which is computed from the locator at read
//! time so a file moved on disk cannot leave a stale "available" flag behind.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use strivo_editor::time::MediaTime;

use super::ids::*;

/// Where a source's bytes currently are. Derived, never persisted: the row
/// keeps the asset id and the media fingerprint, and this is the answer to
/// "can I still open it right now".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    /// The locator resolves to something that exists.
    Resolved(PathBuf),
    /// No locator, or nothing at the locator. The asset identity and every
    /// link to it are intact; only the bytes are unreachable.
    Unresolved,
}

/// The persistent creative context. Holds many sources and many composition
/// revisions; see [`links`](crate::creator::Store::link_research_project) for
/// the stable-ID relationship to the research kernel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub description: String,
    /// Channel or series context. Free text: the report asks for the context,
    /// not a channel registry.
    pub channel: String,
    pub series: String,
    pub settings_json: String,
    pub created_at: String,
    pub updated_at: String,
}

/// One immutable asset in a project's source collection.
///
/// `locator` is a **resolvable location, never an identity**: it carries no
/// unique constraint, is never a key, and may be rewritten at any time
/// without touching `id` or `media_fingerprint`. Two assets may share a
/// locator (a copy), and a locator may be reused by a different asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceAsset {
    pub id: SourceAssetId,
    pub project_id: ProjectId,
    /// `recording`, `import`, `broll`, `music`, …
    pub kind: String,
    pub title: String,
    /// Content identity, stable across a move.
    pub media_fingerprint: String,
    pub locator: Option<PathBuf>,
    /// Acquisition and rights metadata, opaque JSON.
    pub origin: String,
    pub rights_json: String,
    /// `video`, `audio`, `chat`, `document`, …
    pub track_type: String,
    pub codec: String,
    /// Integer microseconds on the source's own timeline. The single time
    /// representation for the whole Creator edition; the millisecond column
    /// version 1 wrote was replaced by schema version 2 rather than kept
    /// beside it.
    pub duration: Option<MediaTime>,
    pub created_at: String,
}

impl SourceAsset {
    /// Whether the locator currently resolves. Never cached, never stored.
    #[must_use]
    pub fn availability(&self) -> Availability {
        match &self.locator {
            Some(path) if path.exists() => Availability::Resolved(path.clone()),
            _ => Availability::Unresolved,
        }
    }
}

/// One capture run. The wall-clock bounds below are the run's own; the
/// relationship between them and the media timeline is
/// [`ClockSegment`](crate::creator::ClockSegment) rows, one per capture
/// part, because a reconnect leaves a hole in the wall clock that a single
/// rate cannot express.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureSession {
    pub id: CaptureSessionId,
    pub project_id: ProjectId,
    pub source_asset_id: Option<SourceAssetId>,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub finalized: bool,
    /// Recording parts and event clocks until the `ClockMap` lands.
    pub attributes_json: String,
    pub created_at: String,
}

/// One immutable transcript revision for one source. Word ids, timings and
/// human corrections live in `body_json` for now; the columns the report
/// names — provider, model version, author, confidence and provenance — are
/// first-class so a revision can be attributed without parsing a document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptRevision {
    pub id: TranscriptRevisionId,
    pub project_id: ProjectId,
    pub source_asset_id: SourceAssetId,
    pub revision: u32,
    pub provider: String,
    pub model_version: String,
    pub author: String,
    pub body_json: String,
    pub created_at: String,
}

/// One immutable edit. `parent_revision_id` chains the history, so a project
/// accumulates many revisions without a shared counter — `revision` is a
/// position in its own chain and is deliberately not unique per project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompositionRevision {
    pub id: CompositionRevisionId,
    pub project_id: ProjectId,
    pub parent_revision_id: Option<CompositionRevisionId>,
    pub revision: u32,
    pub author: String,
    /// Clips, tracks, source spans, transforms, captions and overlays.
    pub timeline_json: String,
    pub output_profile: String,
    pub created_at: String,
}

/// A destination-shaped derivative of a composition revision. The compiler
/// that turns these into a [`RenderPlan`] is todo 23; this is its parent row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    pub id: VariantId,
    pub project_id: ProjectId,
    pub composition_revision_id: CompositionRevisionId,
    pub name: String,
    /// Explicit overrides on top of the parent edit.
    pub format_overrides_json: String,
    pub created_at: String,
}

/// A resolved render instruction set. `content_hash` covers the resolved
/// inputs, so the same revision with the same tools reuses the plan and a
/// changed revision cannot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderPlan {
    pub id: RenderPlanId,
    pub project_id: ProjectId,
    pub variant_id: VariantId,
    pub renderer: String,
    pub renderer_version: String,
    pub resolved_inputs_json: String,
    pub content_hash: String,
    pub created_at: String,
}

/// A produced output. Identity is the content hash; `locator` is where the
/// bytes landed, on the same terms as a source's — a location, not a key.
/// QC verdicts and job dependencies are todo 24's lifecycle columns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub id: ArtifactId,
    pub project_id: ProjectId,
    pub variant_id: Option<VariantId>,
    pub render_plan_id: Option<RenderPlanId>,
    /// `render`, `thumbnail`, `captions`, `audio`, …
    pub kind: String,
    pub content_hash: String,
    pub locator: Option<PathBuf>,
    pub bytes: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Integer microseconds of the produced output, the same unit and the
    /// same source as a [`SourceAsset`]'s.
    pub duration: Option<MediaTime>,
    pub created_at: String,
}

/// An approved artifact plus its metadata, ready to release. A draft is not
/// a release: `state` is the only thing that authorises a publication.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleasePackage {
    pub id: ReleasePackageId,
    pub project_id: ProjectId,
    pub artifact_id: ArtifactId,
    pub metadata_json: String,
    pub approved_by: String,
    pub approved_at: Option<String>,
    pub state: String,
    pub created_at: String,
}

/// One destination posting. `state` distinguishes uploading, uploaded,
/// processing and published — they are different facts, not one "done".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Publication {
    pub id: PublicationId,
    pub project_id: ProjectId,
    pub release_package_id: ReleasePackageId,
    pub destination: String,
    pub account: String,
    pub state: String,
    pub scheduled_at: Option<String>,
    pub timezone: String,
    pub remote_id: Option<String>,
    pub uploaded_at: Option<String>,
    pub published_at: Option<String>,
    pub last_error: Option<String>,
    pub created_at: String,
}

/// One provider metric reading about a published output. It links to the
/// publication, the package, the artifact and the composition revision, so a
/// chart range can be walked back to the exact edit that produced it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricObservation {
    pub id: MetricObservationId,
    pub project_id: ProjectId,
    pub publication_id: Option<PublicationId>,
    pub release_package_id: Option<ReleasePackageId>,
    pub artifact_id: Option<ArtifactId>,
    pub composition_revision_id: Option<CompositionRevisionId>,
    pub provider: String,
    pub metric: String,
    /// The provider's own definition version — a metric whose meaning
    /// changed is a different metric, not a different number.
    pub definition_version: String,
    pub scope: String,
    pub unit: String,
    pub denominator: Option<String>,
    pub interval_start: String,
    pub interval_end: String,
    pub timezone: String,
    pub value: Option<f64>,
    /// Fraction of the interval actually covered, 0.0–1.0.
    pub coverage: Option<f64>,
    pub retrieved_at: String,
    pub source_query_json: String,
    pub transform_json: String,
    pub chart_json: String,
}
