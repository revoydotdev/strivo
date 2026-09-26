//! Relinking: pointing a source at where its bytes are now, after proving they
//! are the same bytes.
//!
//! Split out of [`super::import`] because relinking is a different operation
//! with different failure modes, not a variant of importing. Import decides
//! *whether a source should exist*; a relink asserts *this source is still
//! here*. Conflating the two is how a library ends up with a source whose id,
//! whose transcript and whose edits all say one thing while its bytes say
//! another.
//!
//! ## The content check is the whole function
//!
//! Without it, relinking redefines what a source is while every transcript and
//! edit naming it keeps pointing at the same id — the exact failure the identity
//! model exists to prevent, reached through the one operation that is *about*
//! identity. Nothing here writes an id or a fingerprint: the only column
//! [`relink`] touches is the locator, which is a location.
//!
//! The locator installed here is canonical (the admission gate canonicalised it
//! on the way in), so "does it resolve" and "is it the same file" are the same
//! question, and [`fingerprint`] is the one that answers it.

use super::entities::{Availability, SourceAsset};
use super::ids::{ProjectId, SourceAssetId};
use super::import::{fingerprint, AdmittedFile};
use super::store::Store;
use crate::Result;

/// Why a relink was refused. Each is one decision the caller can act on
/// differently: the id was wrong, the file could not be read, or the file is a
/// different recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelinkRejection {
    /// The id names no source. Refused rather than creating one, because a
    /// relink that silently created a source would be an import wearing a
    /// relink's name, and would leave the caller believing a missing file had
    /// been recovered when nothing was.
    NoSuchSource(SourceAssetId),
    /// The file could not be read to be hashed, or the row could not be
    /// rewritten. Distinct from a mismatch: this is an I/O failure, and the
    /// content may well be the right content.
    Unreadable { locator: String, detail: String },
    /// The bytes at the new location are not the bytes this source is. A relink
    /// is a claim about the same recording, so pointing a source at different
    /// content is refused — the caller may want that, but it is an import, not
    /// a relink.
    ContentMismatch { expected: String, found: String },
}

impl RelinkRejection {
    /// Stable machine code, the key a caller branches on.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoSuchSource(_) => "import/no-such-source",
            Self::Unreadable { .. } => "import/unreadable",
            Self::ContentMismatch { .. } => "import/content-mismatch",
        }
    }
}

impl std::fmt::Display for RelinkRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchSource(id) => write!(f, "no source asset {id}"),
            Self::Unreadable { locator, detail } => {
                write!(f, "{locator}: cannot be used to relink: {detail}")
            }
            Self::ContentMismatch { expected, found } => write!(
                f,
                "that file is {found} and this source is {expected}; relinking one \
                 to the other would change what the source is"
            ),
        }
    }
}

/// Point a source at where its bytes are now, after proving they are the same
/// bytes.
///
/// The content check is the whole function. Without it, relinking is a way to
/// redefine what a source is while every transcript and edit naming it keeps
/// pointing at the same id — the exact failure the identity model exists to
/// prevent, reached through the one operation that is *about* identity. Nothing
/// here writes an id or a fingerprint.
pub fn relink(
    store: &Store,
    source: SourceAssetId,
    file: &AdmittedFile,
) -> std::result::Result<SourceAsset, RelinkRejection> {
    let unreadable = |e: crate::ResearchError| RelinkRejection::Unreadable {
        locator: file.locator().display().to_string(),
        detail: e.to_string(),
    };
    let existing = store
        .source_asset(source)
        .map_err(unreadable)?
        .ok_or(RelinkRejection::NoSuchSource(source))?;
    let found = fingerprint(file.locator()).map_err(unreadable)?;
    if found != existing.media_fingerprint {
        return Err(RelinkRejection::ContentMismatch {
            expected: existing.media_fingerprint,
            found,
        });
    }
    store
        .set_source_locator(source, Some(file.locator()))
        .map_err(unreadable)?;
    store
        .source_asset(source)
        .map_err(unreadable)?
        .ok_or(RelinkRejection::NoSuchSource(source))
}

/// The sources of a project whose bytes are not reachable right now.
///
/// Derived, never stored: a row is not "missing", the bytes are. This is the
/// list a relink surface is built from, and answering it by asking each row
/// rather than reading a flag is what stops a deleted file being reported as
/// present.
pub fn unresolved_sources(store: &Store, project: ProjectId) -> Result<Vec<SourceAsset>> {
    Ok(store
        .sources_of(project)?
        .into_iter()
        .filter(|s| s.availability() == Availability::Unresolved)
        .collect())
}
