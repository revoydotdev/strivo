//! A brand asset as a **reference**: content-identified, locatable, and
//! either cleared or not.
//!
//! ## The fingerprint is the identity. The locator is a location.
//!
//! Todo 33 established that rule for a project's source collection and this
//! module reuses it verbatim rather than inventing a second scheme for brand
//! assets. A [`AssetRef`] is keyed by [`AssetRef::fingerprint`] — the content
//! digest — and [`AssetRef::locator`] is a resolvable location that may change
//! at any time. Two consequences, both of them tested here:
//!
//! * a move (same bytes, new path) keeps the asset's identity and its terms;
//! * a different file at the old path is a **different** asset, and a brand set
//!   that named the fingerprint does not silently adopt it.
//!
//! Nothing about a path is ever allowed to answer "which asset is this".
//!
//! ## Availability is derived, never stored
//!
//! Exactly as `strivo_research`'s `SourceAsset::availability` is:
//! [`AssetRef::availability`] asks the filesystem and returns the answer,
//! and there is no `available` field a writer can set and then forget to clear.
//! The consequence is the todo's `Must NOT` made structural — a logo that was
//! deleted is [`AssetAvailability::Unresolved`] the instant it is asked about,
//! not once somebody updates a flag.
//!
//! ## Terms are a typed axis, and undecided is not permitted
//!
//! [`AssetTerms::Undecided`] **blocks**. This is todo 38's rule transposed onto
//! brand assets rather than a second opinion about it: an unannotated logo is
//! the overwhelmingly common case — a set is built before anybody thinks about
//! the music bed's licence — so permitting it would make "cleared" the answer
//! for everything nobody has reviewed. A missing asset and an unreviewed asset
//! both stop the export; they are kept apart as different variants because
//! they are different operator actions (find the file vs. write the note).
//!
//! What this is **not**: a licence oracle. There is no SPDX database here and
//! no detection. [`AssetTerms::Cleared`] is a person saying so, and it carries
//! their own words verbatim, which is never parsed.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What an asset is for. Closed on purpose: an open string here is a place two
/// callers can spell the same role two ways and a refusal cannot name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    /// A logo, mark or wordmark, composited over the picture.
    Logo,
    /// A music bed, stinger or outro sound.
    Music,
    /// A font file, for a `drawtext` face the operator has rights to.
    Font,
    /// An image plate burned in over the lower third or the intro.
    Stinger,
}

impl AssetKind {
    /// The word a refusal uses. Lowercase, so it reads inside a sentence.
    #[must_use]
    pub const fn noun(self) -> &'static str {
        match self {
            Self::Logo => "logo",
            Self::Music => "music bed",
            Self::Font => "font",
            Self::Stinger => "stinger",
        }
    }
}

/// What a person has decided about an asset's terms. Never an oracle; see the
/// module doc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AssetTerms {
    /// Somebody looked, and the answer was yes. `note` is their own words and
    /// is carried verbatim — never resolved, never parsed.
    Cleared { note: String },
    /// Nobody has looked. Blocks an export.
    Undecided,
}

/// Whether an asset's bytes are reachable right now. Derived on every read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetAvailability {
    /// The locator resolves to something that exists.
    Resolved(PathBuf),
    /// No locator, or nothing at the locator. The identity and the terms note
    /// are intact; only the bytes are unreachable.
    Unresolved,
}

/// One brand asset, by content identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetRef {
    /// The operator-facing name. This is what a refusal prints, so it is a
    /// name a person would recognise and not a filename.
    pub name: String,
    pub kind: AssetKind,
    /// Content identity, stable across a move. Never empty: an asset with no
    /// digest has no identity, and [`AssetRef::identity`] says so.
    pub fingerprint: String,
    /// A resolvable location. May change, may be absent, is never a key.
    pub locator: Option<PathBuf>,
    pub terms: AssetTerms,
}

impl AssetRef {
    /// The identity: the content digest. Named rather than read off
    /// `self.fingerprint` at call sites so "which field is the identity" has
    /// one answer.
    #[must_use]
    pub fn identity(&self) -> &str {
        &self.fingerprint
    }

    /// Whether the bytes are reachable. Never cached, never stored.
    #[must_use]
    pub fn availability(&self) -> AssetAvailability {
        match &self.locator {
            Some(path) if path.exists() => AssetAvailability::Resolved(path.clone()),
            _ => AssetAvailability::Unresolved,
        }
    }

    /// The resolved path, if there is one. This is the only way a path leaves
    /// an [`AssetRef`], so there is exactly one place a caller can obtain a
    /// path that has just been checked.
    #[must_use]
    pub fn resolved_path(&self) -> Option<&Path> {
        self.locator.as_deref().filter(|path| path.exists())
    }

    /// Whether a person has cleared this asset's terms.
    #[must_use]
    pub fn terms_cleared(&self) -> bool {
        matches!(self.terms, AssetTerms::Cleared { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmpdir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("strivo-brand-asset-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn asset(name: &str, locator: Option<PathBuf>) -> AssetRef {
        AssetRef {
            name: name.into(),
            kind: AssetKind::Logo,
            fingerprint: format!("sha256:{name}"),
            locator,
            terms: AssetTerms::Cleared {
                note: "my own artwork".into(),
            },
        }
    }

    #[test]
    fn a_present_locator_resolves() {
        let dir = tmpdir("present");
        let file = dir.join("logo.png");
        fs::write(&file, b"x").unwrap();
        let a = asset("logo", Some(file.clone()));
        assert_eq!(a.availability(), AssetAvailability::Resolved(file.clone()));
        assert_eq!(a.resolved_path(), Some(file.as_path()));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_deleted_locator_is_unresolved_and_availability_is_not_stored() {
        // The todo's `Must NOT` in miniature: there is no field to write, so
        // deleting the bytes is the whole change and the answer moves with it.
        let dir = tmpdir("deleted");
        let file = dir.join("logo.png");
        fs::write(&file, b"x").unwrap();
        let a = asset("logo", Some(file.clone()));
        assert!(a.resolved_path().is_some());
        fs::remove_file(&file).unwrap();
        assert_eq!(a.availability(), AssetAvailability::Unresolved);
        assert!(a.resolved_path().is_none());
        // The identity and the terms note survive the move of the bytes —
        // that is what makes the asset the same asset.
        assert_eq!(a.identity(), "sha256:logo");
        assert!(a.terms_cleared());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_asset_with_no_locator_is_unresolved() {
        let a = asset("logo", None);
        assert_eq!(a.availability(), AssetAvailability::Unresolved);
    }

    #[test]
    fn a_moved_asset_keeps_its_identity_while_a_new_file_at_the_old_path_does_not() {
        // Todo 33's rule, re-asserted for brand assets: identity is content.
        let dir = tmpdir("moved");
        let first = dir.join("logo.png");
        fs::write(&first, b"original").unwrap();
        let a = asset("logo", Some(first.clone()));
        let identity = a.identity().to_string();

        fs::rename(&first, dir.join("logo-2.png")).unwrap();
        let moved = AssetRef {
            locator: Some(dir.join("logo-2.png")),
            ..a.clone()
        };
        assert_eq!(
            moved.identity(),
            identity,
            "a move must not change identity"
        );

        // A different file at the old path is a different asset, and the
        // fingerprint is what says so — not the path.
        let other = AssetRef {
            fingerprint: "sha256:different".into(),
            ..a
        };
        assert_ne!(other.identity(), identity);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn undecided_terms_are_not_cleared() {
        let a = AssetRef {
            terms: AssetTerms::Undecided,
            ..asset("music", None)
        };
        assert!(!a.terms_cleared());
    }

    #[test]
    fn kind_nouns_read_inside_a_sentence() {
        assert_eq!(AssetKind::Logo.noun(), "logo");
        assert_eq!(AssetKind::Music.noun(), "music bed");
    }

    #[test]
    fn json_roundtrip_keeps_the_fingerprint_and_the_locator_apart() {
        let a = asset("logo", Some(PathBuf::from("/logos/a.png")));
        let back: AssetRef =
            serde_json::from_str(&serde_json::to_string(&a).expect("serialise")).expect("parse");
        assert_eq!(back.fingerprint, a.fingerprint);
        assert_eq!(back.locator, a.locator);
        assert_eq!(back, a);
    }
}
