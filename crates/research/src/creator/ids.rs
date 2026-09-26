//! Typed identity for the Creator edition.
//!
//! Every Creator-owned entity has a distinct newtype over [`Uuid`]. That is
//! the whole point: a `ProjectId` cannot be passed where a `SourceAssetId`
//! is expected, so "one entity's data leaking into another's surface" is a
//! compile error rather than a query that happens to return a wrong row.
//!
//! The research kernel's own [`crate::Project`] keeps its `Uuid` id and is
//! **not** retyped here. It is referenced by [`ResearchProjectId`], a link
//! handle only — see [`crate::creator`].

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ResearchError;

macro_rules! typed_ids {
    ($($(#[$meta:meta])* $name:ident),* $(,)?) => {$(
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Mint a fresh random identity. Named `generate`, not `new`,
            /// so there is no `Default` implying a well-known id.
            #[must_use]
            pub fn generate() -> Self {
                Self(Uuid::new_v4())
            }

            /// Adopt an existing UUID as this kind of identity. Used by
            /// import adapters, never by a caller resolving a row.
            #[must_use]
            pub const fn from_uuid(id: Uuid) -> Self {
                Self(id)
            }

            /// The underlying UUID.
            #[must_use]
            pub const fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl From<Uuid> for $name {
            fn from(id: Uuid) -> Self {
                Self(id)
            }
        }

        impl FromStr for $name {
            type Err = ResearchError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s)
                    .map(Self)
                    .map_err(|e| ResearchError::Validation(format!("not a {}: {e}", stringify!($name))))
            }
        }
    )*};
}

typed_ids!(
    /// A Creator project: the persistent context that holds many sources and
    /// many composition revisions. Distinct from the research kernel's
    /// [`crate::Project`], which it links to rather than replaces.
    ProjectId,
    /// One immutable asset in a project's source collection. Identity is the
    /// id plus the media fingerprint; the filesystem locator is a resolvable
    /// location that may change at any time.
    SourceAssetId,
    /// One capture run: recording parts, wall-clock bounds, finalization.
    CaptureSessionId,
    /// One immutable revision of a source's transcript.
    TranscriptRevisionId,
    /// One immutable revision of an edit. Revisions chain through their
    /// parent, so a project accumulates many without a shared counter.
    CompositionRevisionId,
    /// A destination-shaped derivative of a composition revision.
    VariantId,
    /// A resolved, reproducible render instruction set.
    RenderPlanId,
    /// A produced output, identified by its content hash rather than a path.
    ArtifactId,
    /// One durable creator job: a render, a transcription, a publication.
    ///
    /// A job **is** the lifecycle. The artifacts it produces hang off it, and
    /// its `cache_key` is what lets an unchanged set of inputs reuse an earlier
    /// output instead of running again.
    JobId,
    /// An approved artifact plus its metadata, ready to release.
    ReleasePackageId,
    /// One destination posting of a release package.
    PublicationId,
    /// One provider metric reading about a published output.
    MetricObservationId,
    /// A **link handle** to a research-kernel project id. It carries no
    /// schema of its own and is never a foreign key into the research
    /// tables: the report requires the two project models to be related by
    /// stable ID, not merged.
    ResearchProjectId,
);
