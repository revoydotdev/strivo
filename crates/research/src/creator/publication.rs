//! The release package and the publication lifecycle, from
//! `docs/CREATOR-EDITION-REPORT-2026-09-07.md` §4.6 and §7.4.
//!
//! **The ten-stage `creator_publish` pipeline is preparation, not publication.**
//! It ends at a rendered artifact and a draft; it does not render for a
//! destination, upload, reconcile a processing state, schedule, capture a
//! remote id or read a metric. Calling that "publish" was a lie about what the
//! product could do, and §4.6 requires the user-facing name to say
//! *preparation* until the contracts below exist. They now exist, which is why
//! the name is being corrected here rather than left to a later todo.
//!
//! **What a release package is.** An *immutable set of approved inputs* — one
//! approved composition revision, one destination variant, the one rendered
//! artifact produced from them, the metadata, and the recorded source set the
//! rights gate was run over. It **refers back to** identities owned by todos
//! 21/23/24 (`CompositionRevisionId`, `VariantId`, `ArtifactId`) and
//! re-derives none of them: the package is the approval record over somebody
//! else's work, not a second copy of it.
//!
//! [`PackageContent::fingerprint`] is the identity of that set. It hashes the
//! six content fields and deliberately **not** `created_at`, for
//! [`super::rights::SourceNote::fingerprint`]'s reason: re-saving an unchanged
//! package is not a change to it, and an approval that goes stale because
//! somebody pressed save again is an approval that gets ignored. Approval
//! stores the fingerprint it approved, so "is this still the thing a person
//! signed off on" is a string comparison rather than a judgement call.
//!
//! **Immutability is structural, in two places.** The mutator
//! [`Store::edit_release_package`](super::Store::edit_release_package) is the
//! only writer of package content, and it refuses a package that any
//! publication has already carried out of `approved` — so a *scheduled*
//! package cannot be edited, at all, by any caller. The alternative (a
//! convention, or a database trigger nobody remembers) was rejected: a caller
//! that could do it, could do it. Where an edit *is* allowed on an approved
//! package the approval is **revoked in the same transaction** — the row goes
//! back to `draft` with `approved_by`/`approved_at`/`approved_fingerprint`
//! cleared — so a later publication cannot be authorised by an approval that
//! described different content.
//!
//! **The rights verdict is never stored.** `Eligibility` is derived, per todo
//! 38, and a package that carried a verdict would carry a stale one. The gate
//! re-runs [`super::Store::publication_report`] over the package's recorded
//! source set at approval and again at every transition that hands bytes to a
//! provider. **Known residual, not closed here:** the recorded source set is
//! declared by the caller that drafts the package, because
//! `creator_composition_revisions.timeline_json` is an opaque document and a
//! compiled `RenderPlan` carries cut *indices* rather than source identities —
//! there is no machine-readable path from an artifact to its sources yet. So a
//! package drafted from an incomplete list passes a gate that list did not
//! satisfy. What this module does guarantee is that the list cannot change
//! under the gate, and that a note which is missing, undetermined or later
//! withdrawn blocks.
//!
//! **No exactly-once promise, anywhere.** The lifecycle has `uploading →
//! processing → published` *and* `needs_reauthorization` *and*
//! `unknown_remote_state` precisely because a provider does not offer
//! exactly-once posting: a request can leave and its answer be lost, and then
//! the honest state is "I do not know", not "failed" and not "published". The
//! invariant this module actually offers is **at-least-once attempt with an
//! explicit ambiguous state and an append-only action log** — a caller can
//! always see what was tried, and a duplicate is a reconciliation decision a
//! person makes, not a silent property of the code.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::ids::{ArtifactId, CompositionRevisionId, SourceAssetId, VariantId};
use crate::ResearchError;

/// The states of one destination posting. Fourteen, all of them listed here
/// and none invented at a call site — a state machine whose states are
/// unlisted is not a state machine.
///
/// The first nine are the report's lifecycle (`draft → needs_review →
/// approved → rendering → ready → uploading → processing → scheduled/
/// published`); the last five are the ways that lifecycle can honestly end or
/// stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationState {
    /// Built locally, never shown to a provider. The only state a caller may
    /// create one in.
    Draft,
    /// Waiting for a person. Distinct from `draft` because "not looked at" and
    /// "being looked at" are different facts and a queue reads the difference.
    NeedsReview,
    /// A person approved it and the package behind it is `approved`.
    Approved,
    /// Producing the destination-shaped bytes. Local; the artifact the package
    /// names may be re-derived here for a different destination.
    Rendering,
    /// The bytes exist and validate. Not yet anywhere a provider can see.
    Ready,
    /// Bytes are in flight. **Not a claim the provider received them.**
    Uploading,
    /// The provider acknowledged receipt and reports it is transcoding. Also
    /// not a claim anyone can watch.
    Processing,
    /// Accepted with a future publish time. The local promise, not the
    /// provider's.
    Scheduled,
    /// The provider reports it live. Only this, and only with a `remote_id`,
    /// means published.
    Published,
    /// The work was attempted and the outcome is known to be a failure.
    /// Carries the provider's own words in the event log.
    Failed,
    /// Stopped on purpose, locally. Never resumed implicitly.
    Cancelled,
    /// The provider's credential is gone, expired or revoked. Recoverable by
    /// re-authorising, and **not** a failure of the content.
    NeedsReauthorization,
    /// The truth is unknowable: a request left and its answer did not come
    /// back, or the provider will not say. **This is the honesty state**, and
    /// it is deliberately distinguishable from both [`Self::Failed`] (known
    /// bad) and [`Self::Published`] (known good). Collapsing it into either is
    /// how a duplicate gets created silently.
    UnknownRemoteState,
    /// Confirmed gone from the destination. The end of the line.
    Removed,
}

impl PublicationState {
    /// Every state, in lifecycle order. The array length is the number of
    /// states, so adding a variant without changing it is a compile error —
    /// which is the point: a state that exists but is not in here is a state
    /// the exhaustive tests never drive.
    pub const ALL: [PublicationState; 14] = [
        PublicationState::Draft,
        PublicationState::NeedsReview,
        PublicationState::Approved,
        PublicationState::Rendering,
        PublicationState::Ready,
        PublicationState::Uploading,
        PublicationState::Processing,
        PublicationState::Scheduled,
        PublicationState::Published,
        PublicationState::Failed,
        PublicationState::Cancelled,
        PublicationState::NeedsReauthorization,
        PublicationState::UnknownRemoteState,
        PublicationState::Removed,
    ];

    /// The stable lowercase name, which is also the stored spelling and the
    /// one a UI reads. One function so the three cannot drift.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::NeedsReview => "needs_review",
            Self::Approved => "approved",
            Self::Rendering => "rendering",
            Self::Ready => "ready",
            Self::Uploading => "uploading",
            Self::Processing => "processing",
            Self::Scheduled => "scheduled",
            Self::Published => "published",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::NeedsReauthorization => "needs_reauthorization",
            Self::UnknownRemoteState => "unknown_remote_state",
            Self::Removed => "removed",
        }
    }

    /// Whether the provider has been involved at all. Everything below this
    /// line is a local fact and needs no reconciliation; everything above it
    /// does.
    #[must_use]
    pub const fn is_remote(self) -> bool {
        matches!(
            self,
            Self::Uploading
                | Self::Processing
                | Self::Scheduled
                | Self::Published
                | Self::NeedsReauthorization
                | Self::UnknownRemoteState
                | Self::Removed
        )
    }

    /// Whether nothing can move this state again. Only `removed` and
    /// `cancelled` are final, and that is the invariant the transition table
    /// exists to keep.
    #[must_use]
    pub const fn is_final(self) -> bool {
        matches!(self, Self::Removed | Self::Cancelled)
    }

    /// Whether a package edit could still reach this publication. A package
    /// that any publication has carried this far is frozen.
    #[must_use]
    pub const fn locks_package(self) -> bool {
        matches!(
            self,
            Self::Approved
                | Self::Rendering
                | Self::Ready
                | Self::Uploading
                | Self::Processing
                | Self::Scheduled
                | Self::Published
        )
    }
}

impl fmt::Display for PublicationState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PublicationState {
    type Err = ResearchError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|state| state.as_str() == s)
            .ok_or_else(|| ResearchError::Validation(format!("not a publication state: {s}")))
    }
}

/// The legal edges, each naming why it exists. Every writer consults
/// [`is_legal`], so an unlisted transition cannot be written by any of them.
///
/// The shape worth noticing: **every remote-facing state has an edge to
/// [`PublicationState::UnknownRemoteState`]**, and `unknown_remote_state` has
/// an edge back to each of the states reconciliation can establish. There is no
/// edge from `uploading` straight to `published`, because a provider that
/// accepted bytes has not published them yet, and skipping `processing` is how
/// a "live" claim gets made about a video that is still transcoding.
pub const TRANSITIONS: &[(PublicationState, PublicationState)] = &[
    // draft: submitted for a person's eyes, or abandoned before anybody looked.
    (PublicationState::Draft, PublicationState::NeedsReview),
    (PublicationState::Draft, PublicationState::Cancelled),
    // needs_review: approved, sent back for more work, or abandoned.
    (PublicationState::NeedsReview, PublicationState::Approved),
    (PublicationState::NeedsReview, PublicationState::Draft),
    (PublicationState::NeedsReview, PublicationState::Cancelled),
    // approved: render for the destination, or abandoned before the bytes move.
    (PublicationState::Approved, PublicationState::Rendering),
    (PublicationState::Approved, PublicationState::Cancelled),
    // rendering: bytes exist, the render failed, or it was called off.
    (PublicationState::Rendering, PublicationState::Ready),
    (PublicationState::Rendering, PublicationState::Failed),
    (PublicationState::Rendering, PublicationState::Cancelled),
    // ready: upload, re-render for a different destination, or call off.
    (PublicationState::Ready, PublicationState::Uploading),
    (PublicationState::Ready, PublicationState::Rendering),
    (PublicationState::Ready, PublicationState::Cancelled),
    // uploading: the four ways an upload ends, and the fifth way it does not
    // end at all.
    (PublicationState::Uploading, PublicationState::Processing),
    (PublicationState::Uploading, PublicationState::Failed),
    (
        PublicationState::Uploading,
        PublicationState::NeedsReauthorization,
    ),
    (
        PublicationState::Uploading,
        PublicationState::UnknownRemoteState,
    ),
    (PublicationState::Uploading, PublicationState::Cancelled),
    // processing: the provider is transcoding. It may yet schedule, publish,
    // refuse, lose the credential, go quiet, or take the video down.
    (PublicationState::Processing, PublicationState::Scheduled),
    (PublicationState::Processing, PublicationState::Published),
    (PublicationState::Processing, PublicationState::Failed),
    (
        PublicationState::Processing,
        PublicationState::NeedsReauthorization,
    ),
    (
        PublicationState::Processing,
        PublicationState::UnknownRemoteState,
    ),
    (PublicationState::Processing, PublicationState::Removed),
    // scheduled: the local promise comes due, or the world disagrees with it.
    (PublicationState::Scheduled, PublicationState::Published),
    (PublicationState::Scheduled, PublicationState::Failed),
    (
        PublicationState::Scheduled,
        PublicationState::NeedsReauthorization,
    ),
    (
        PublicationState::Scheduled,
        PublicationState::UnknownRemoteState,
    ),
    (PublicationState::Scheduled, PublicationState::Cancelled),
    (PublicationState::Scheduled, PublicationState::Removed),
    // published: a takedown, or the provider ceasing to answer about it.
    (PublicationState::Published, PublicationState::Removed),
    (
        PublicationState::Published,
        PublicationState::UnknownRemoteState,
    ),
    // needs_reauthorization: resume the upload with a fresh credential, give
    // up, or admit the remote answer is unknown.
    (
        PublicationState::NeedsReauthorization,
        PublicationState::Uploading,
    ),
    (
        PublicationState::NeedsReauthorization,
        PublicationState::Cancelled,
    ),
    (
        PublicationState::NeedsReauthorization,
        PublicationState::UnknownRemoteState,
    ),
    // unknown_remote_state: the ONLY exit is a reconciliation that established
    // something. Retrying the upload is deliberately absent — the request may
    // well have succeeded, and that is the whole reason this state exists.
    (
        PublicationState::UnknownRemoteState,
        PublicationState::Processing,
    ),
    (
        PublicationState::UnknownRemoteState,
        PublicationState::Published,
    ),
    (
        PublicationState::UnknownRemoteState,
        PublicationState::Failed,
    ),
    (
        PublicationState::UnknownRemoteState,
        PublicationState::NeedsReauthorization,
    ),
    (
        PublicationState::UnknownRemoteState,
        PublicationState::Removed,
    ),
    // failed: retry the render, re-authorise, give up, or accept a takedown.
    (PublicationState::Failed, PublicationState::Rendering),
    (
        PublicationState::Failed,
        PublicationState::NeedsReauthorization,
    ),
    (
        PublicationState::Failed,
        PublicationState::UnknownRemoteState,
    ),
    (PublicationState::Failed, PublicationState::Cancelled),
    (PublicationState::Failed, PublicationState::Removed),
    // cancelled: the local record is retired. Never resumed implicitly.
    (PublicationState::Cancelled, PublicationState::Removed),
];

/// Whether `from -> to` is one of the legal edges. The single answer every
/// writer consults.
#[must_use]
pub fn is_legal(from: PublicationState, to: PublicationState) -> bool {
    TRANSITIONS.contains(&(from, to))
}

/// The immutable input set of a release package.
///
/// A value, not a row: the row is [`super::entities::ReleasePackage`] and this
/// is what its content columns mean. `sources` is the set the rights gate is
/// run over — see the module doc for why it is declared rather than derived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageContent {
    pub composition_revision_id: CompositionRevisionId,
    pub variant_id: VariantId,
    pub artifact_id: ArtifactId,
    /// The destination-facing metadata, as an opaque JSON document. Opaque
    /// because todo 20's capability descriptor is the only place a field's
    /// meaning is decided; a package that re-declared the fields would be a
    /// second capability list.
    pub metadata_json: String,
    /// The sources this release was made from, sorted by the store on the way
    /// in so the fingerprint cannot depend on the caller's ordering.
    pub sources: Vec<SourceAssetId>,
}

impl PackageContent {
    /// The identity of this content set. Hashes ids and metadata only, and is
    /// stable under re-saving unchanged content.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let mut digest = Sha256::new();
        let mut feed = |part: &str| {
            digest.update(part.as_bytes());
            // A separator, so ("ab","c") and ("a","bc") cannot share a digest.
            digest.update(b"\x1f");
        };
        feed(&self.composition_revision_id.to_string());
        feed(&self.variant_id.to_string());
        feed(&self.artifact_id.to_string());
        feed(&self.metadata_json);
        let mut sources: Vec<String> = self.sources.iter().map(ToString::to_string).collect();
        sources.sort();
        sources.dedup();
        feed(&sources.join("\x1e"));
        digest.update(b"release-package-v1");
        digest
            .finalize()
            .iter()
            .fold(String::with_capacity(64), |mut hex, byte| {
                use std::fmt::Write as _;
                let _ = write!(hex, "{byte:02x}");
                hex
            })
    }
}

/// The package review vocabulary. Three states, and they are version 1's
/// exactly: `creator_release_packages` shipped with this CHECK and a fourth
/// review state would have needed the table rebuilt for no gain. The full
/// fourteen-state lifecycle belongs to the **publication**, which is the thing
/// that actually reaches a destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageState {
    /// Editable. Not shown to a provider.
    Draft,
    /// A person approved this exact [`PackageContent::fingerprint`].
    Approved,
    /// Replaced by a later package. Its publications are untouched: a
    /// supersession is a statement about the *draft*, never about something
    /// already scheduled.
    Superseded,
}

impl PackageState {
    pub const ALL: [PackageState; 3] = [
        PackageState::Draft,
        PackageState::Approved,
        PackageState::Superseded,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Approved => "approved",
            Self::Superseded => "superseded",
        }
    }
}

impl fmt::Display for PackageState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PackageState {
    type Err = ResearchError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|state| state.as_str() == s)
            .ok_or_else(|| ResearchError::Validation(format!("not a package state: {s}")))
    }
}

/// One row of `creator_publication_events`: what was attempted, from which
/// state, and what the provider said.
///
/// Append-only, written in the same transaction as the state change, so a
/// publication can never be observed in a state its own log does not record.
/// This is the P01 acceptance criterion "failed or ambiguous remote actions
/// retain recoverable state and an action log" — the log is what makes an
/// ambiguous action *recoverable*, since the alternative to re-checking the
/// provider is guessing from local state alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationEvent {
    pub id: i64,
    pub publication_id: super::ids::PublicationId,
    /// Monotonic per publication, assigned by the store from `MAX(seq) + 1`.
    pub seq: i64,
    pub from: Option<PublicationState>,
    pub to: PublicationState,
    /// What was attempted, in the caller's words. Untrusted remote text is
    /// carried here verbatim and never interpreted.
    pub detail: String,
    pub recorded_at: String,
}

/// A typed refusal from the publication store.
///
/// Not `Clone`: [`Self::Rights`] owns a report whose whole purpose is to name
/// the blocking sources, and a caller that loses it is left with "no".
#[derive(Debug, PartialEq, Eq)]
pub enum PublishError {
    /// A transition that is not in [`TRANSITIONS`], or one whose precondition
    /// the destination did not meet. Both are reported with both states, so a
    /// caller can see what it asked for and what it was holding.
    IllegalTransition {
        from: PublicationState,
        to: PublicationState,
    },
    /// The package does not exist, or the caller named a package outside its
    /// project.
    UnknownPackage { id: String },
    /// The publication does not exist.
    UnknownPublication { id: String },
    /// The package is not `draft`, so its content is not this module's to
    /// change. Editing an approved package is legal — it revokes the approval —
    /// but only a *locked* package is refused outright.
    NotEditable { state: PackageState },
    /// A publication has already carried this package out of `approved`, so
    /// the package's content is frozen. **This is what makes a scheduled
    /// package impossible to alter**, in the store rather than in a
    /// convention.
    PackageLocked { state: PublicationState },
    /// The rights gate refused, or could not clear, this package. Carries the
    /// whole report so the UI can name the source rather than say "no".
    Rights(super::store_rights::PublicationReport),
    /// `published` requires a remote id: without one there is no posting to
    /// point at, and claiming otherwise is the exact failure this module
    /// exists to prevent.
    MissingRemoteId,
    /// `scheduled` requires a time to come due at.
    MissingSchedule,
    /// The package is not `approved`, so a publication cannot leave `draft`.
    PackageNotApproved { state: PackageState },
    /// The rights gate refused to run: a stored note is unreadable, or the
    /// package's recorded source set is not valid JSON. Distinct from
    /// [`Self::Rights`] — nothing was *refused*, the check could not be made.
    GateUnavailable { reason: String },
    /// The database said no, in the store's own words.
    Storage { reason: String },
}

impl fmt::Display for PublishError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IllegalTransition { from, to } => {
                write!(f, "no publication transition {from} -> {to}")
            }
            Self::UnknownPackage { id } => write!(f, "no release package {id}"),
            Self::UnknownPublication { id } => write!(f, "no publication {id}"),
            Self::NotEditable { state } => {
                write!(f, "a {state} release package is not editable")
            }
            Self::PackageLocked { state } => write!(
                f,
                "a publication has carried this package to {state}, so its content is frozen"
            ),
            Self::Rights(report) => write!(f, "rights gate: {report}"),
            Self::MissingRemoteId => {
                f.write_str("published needs the provider's own id for the posting")
            }
            Self::MissingSchedule => f.write_str("scheduled needs a time to come due at"),
            Self::PackageNotApproved { state } => {
                write!(f, "the release package is {state}, not approved")
            }
            Self::GateUnavailable { reason } => {
                write!(f, "the rights gate could not be run: {reason}")
            }
            Self::Storage { reason } => write!(f, "publication store: {reason}"),
        }
    }
}

impl std::error::Error for PublishError {}

/// A note the gate could not read is [`Self::GateUnavailable`], not
/// [`Self::Rights`]: nothing was *refused*, the check could not be made, and a
/// caller that cannot tell those two apart reports neither.
impl From<super::rights::RightsError> for PublishError {
    fn from(e: super::rights::RightsError) -> Self {
        Self::GateUnavailable {
            reason: e.to_string(),
        }
    }
}

impl From<ResearchError> for PublishError {
    fn from(e: ResearchError) -> Self {
        Self::Storage {
            reason: e.to_string(),
        }
    }
}

impl From<rusqlite::Error> for PublishError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Storage {
            reason: e.to_string(),
        }
    }
}
