//! Ownership, licence and consent notes, and **publication eligibility derived
//! from them**.
//!
//! Report item C07 (`docs/CREATOR-EDITION-REPORT-2026-09-07.md:286,338`): make
//! personal-archive material and creator-owned/publishable assets
//! distinguishable, and carry provenance and review requirements onto every
//! derivative.
//!
//! It answers the open question at
//! `docs/STRATEGY-NVIVO-RIVERSIDE.md:187-190` — *chat messages are other
//! people's speech, so what does the creator owe a participant before
//! publishing a clip with a chat overlay?* — with a note the operator fills in
//! and a verdict they cannot.
//!
//! ## Three axes, and they are independent
//!
//! | axis | question | values |
//! |---|---|---|
//! | [`Ownership`] | whose material is this? | creator-owned, third-party, archive-only, undecided |
//! | [`Licence`] | may it be redistributed? | granted, absent, undecided |
//! | [`Consent`] | may *other people's* material in it be redistributed? | granted, refused, not-required, undecided |
//!
//! Three axes rather than one "rights" flag because a single flag cannot
//! express the two failure modes that matter: a source that is licence-clear
//! but was never cleared for a chat participant, and a source with every
//! participant's consent but no licence anyone has checked. A combined
//! "rights: ok" collapses both into the one answer a creator most wants to be
//! wrong about.
//!
//! ## Eligibility is derived. There is no column for it.
//!
//! [`Eligibility`] has exactly one producer, [`SourceNote::eligibility`], and
//! the table has no `publishable`/`eligible` column — a test reads
//! `pragma_table_info` to check that rather than trusting this paragraph. A
//! stored flag is a flag somebody can write, and a caller who can write
//! "eligible" has not been gated.
//!
//! ## The truth table
//!
//! [`Eligibility::Publishable`] requires **all three** axes to permit. Anything
//! else is a refusal, and the refusal names which axis and why. The two
//! non-permitting states are kept apart because they are different operational
//! facts:
//!
//! * **Undecided** — nobody has looked. `Undetermined` on any axis is
//!   undecided, and so is a source with **no note at all**, which is the
//!   fail-closed default the todo cares about: a source imported a second ago
//!   resolves to *undecided on every axis*, never to publishable.
//! * **Blocked** — somebody looked and the answer was no. `Licence::Absent`,
//!   `Consent::Refused` and `Ownership::ArchiveOnly` are decisions.
//!
//! **Undecided is not permitted, and the reason is the direction of the
//! default.** An unannotated source is the overwhelmingly common case — every
//! capture lands before anybody has thought about its licence — so permitting
//! it would make "publish" the answer for everything nobody has reviewed yet.
//! That is the fail-open gate `src/licence/gate.rs` documents ("a wiring some
//! process forgets is a fail-open gate"), transposed from entitlements to
//! rights. Undecided blocks, and it blocks *differently* from a refusal so a
//! UI can send the operator to fill in a note rather than to give up.
//!
//! ## A note hangs off the identity, never off a path
//!
//! The note is keyed by [`SourceAssetId`]. Todo 33 established that a path is
//! a locator and content is the identity; a licence note attached to a path
//! would silently re-attach to different bytes when the file moved, which for
//! rights data is the difference between "cleared" and "cleared for something
//! else entirely". Three tests hold that line: a move preserves the note, a
//! different file at the old path does not inherit it, and a re-import of the
//! same bytes keeps it.
//!
//! ## What a note is *not*
//!
//! **It is a record of what a human decided, never a licence oracle.** There is
//! no SPDX database here and no detection: nothing in this module reads a
//! licence identifier, resolves one, or concludes anything from prose. A
//! licence is [`Licence::Granted { grant }`] because a person said so, and the
//! grant text is carried verbatim and never parsed — which is also why a note
//! whose every field reads like an approval still cannot grant anything
//! (`a_note_that_looks_like_a_grant_does_not_grant`). The decision is the typed
//! axis; the text is a human's own words about it.
//!
//! No dependency is added for any of this.

use std::fmt;

use super::ids::SourceAssetId;

/// The largest licence grant, consent scope, refusal reason, holder or
/// disclosure, in bytes.
///
/// Bounded because these are untrusted free text on their way to a UI and into
/// a row. Over the limit is [`RightsError::NoteTooLong`], **never** a
/// truncation: a shortened grant reads exactly like a whole one, and a
/// shortened refusal reason reads like the person consented.
pub const MAX_NOTE_BYTES: usize = 4_096;

/// A calendar date, `YYYY-MM-DD`. No time, no zone, no clock.
///
/// Consent expires on a *day*, and a day is all a person states. The type is
/// here so an unparseable date is a typed refusal at the boundary rather than
/// a string that would compare lexicographically against a real one — which is
/// how a lapsed consent becomes a perpetual one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CalendarDate {
    year: i32,
    month: u32,
    day: u32,
}

impl CalendarDate {
    /// Parse `YYYY-MM-DD`, validating the calendar rather than the shape.
    ///
    /// `2023-02-29` is refused: a date that does not exist is not a date, and
    /// accepting it would let a caller compute an expiry that never arrives.
    pub fn parse(raw: &str) -> Result<Self, RightsError> {
        let bad = || RightsError::MalformedDate {
            found: raw.chars().take(32).collect(),
        };
        let (year, rest) = raw.split_once('-').ok_or_else(bad)?;
        let (month, day) = rest.split_once('-').ok_or_else(bad)?;
        if year.len() != 4 || month.len() != 2 || day.len() != 2 {
            return Err(bad());
        }
        if !raw.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
            return Err(bad());
        }
        let year: i32 = year.parse().map_err(|_| bad())?;
        let month: u32 = month.parse().map_err(|_| bad())?;
        let day: u32 = day.parse().map_err(|_| bad())?;
        if !(1..=12).contains(&month) || day < 1 || day > days_in(year, month) {
            return Err(bad());
        }
        Ok(Self { year, month, day })
    }

    /// The stored spelling, which is the wire spelling.
    #[must_use]
    pub fn as_str(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

impl std::str::FromStr for CalendarDate {
    type Err = RightsError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl fmt::Display for CalendarDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_str())
    }
}

fn days_in(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Whose material this is, and whether it is the creator's to publish.
///
/// The plan's "personal-archive material and creator-owned or publishable
/// assets are distinguishable" is this axis: [`Self::ArchiveOnly`] is the
/// personal archive, and it is *not* a flavour of the licence. A file somebody
/// let the creator keep and a file the creator may republish are different
/// facts, and merging them into one "rights" bit is the conflation this axis
/// exists to prevent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ownership {
    /// The creator made it, or holds it outright.
    CreatorOwned,
    /// Somebody else made it. `holder` names them, because "third party" with
    /// nobody named is not a record of anything.
    ThirdParty { holder: String },
    /// Personal-archive material: kept, never published. A decision, not an
    /// oversight — it is the answer "I have this and that is all I will do
    /// with it", and it blocks publication whatever the other two axes say.
    ArchiveOnly,
    /// Nobody has said.
    Undetermined,
}

impl Ownership {
    /// Parse the stored spelling, with the holder carried beside it.
    ///
    /// An unrecognised spelling is [`RightsError::UnknownOwnership`], never a
    /// default: a row read as "undetermined" because its own value was
    /// unrecognised is a row that quietly stopped gating.
    pub fn parse(raw: &str, holder: &str) -> Result<Self, RightsError> {
        check_note(holder)?;
        match raw {
            "creator_owned" => Ok(Self::CreatorOwned),
            "archive_only" => Ok(Self::ArchiveOnly),
            "undetermined" => Ok(Self::Undetermined),
            "third_party" if holder.trim().is_empty() => Err(RightsError::EmptyHolder),
            "third_party" => Ok(Self::ThirdParty {
                holder: holder.to_string(),
            }),
            other => Err(RightsError::UnknownOwnership {
                found: other.chars().take(64).collect(),
            }),
        }
    }

    /// The stored spelling.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CreatorOwned => "creator_owned",
            Self::ThirdParty { .. } => "third_party",
            Self::ArchiveOnly => "archive_only",
            Self::Undetermined => "undetermined",
        }
    }

    /// The holder, or the empty string for the other two spellings.
    #[must_use]
    pub fn holder(&self) -> &str {
        match self {
            Self::ThirdParty { holder } => holder,
            _ => "",
        }
    }

    fn verdict(&self) -> AxisVerdict {
        match self {
            Self::CreatorOwned => AxisVerdict::Permit,
            Self::ThirdParty { .. } => AxisVerdict::Permit,
            Self::ArchiveOnly => AxisVerdict::Block(
                "this is personal-archive material: kept, never published".to_string(),
            ),
            Self::Undetermined => AxisVerdict::Undecided,
        }
    }
}

/// Whether the material may be redistributed at all.
///
/// The *copyright* axis, deliberately separate from [`Consent`]: a
/// participant's permission is not a grant of somebody's copyright, and a
/// model with one axis for both cannot say which one is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Licence {
    /// A person stated a grant. `grant` is their own words, carried verbatim.
    Granted { grant: String },
    /// Looked, and there is no grant. A decision.
    Absent,
    /// Nobody has looked. The common case, and it blocks.
    Undetermined,
}

impl Licence {
    /// Parse the stored spelling with the grant text beside it.
    ///
    /// An empty grant is [`RightsError::EmptyLicenceGrant`] rather than an
    /// accepted grant with nothing attached, because "I grant permission" with
    /// no words is indistinguishable from a client that sent an empty field.
    pub fn parse(raw: &str, grant: &str) -> Result<Self, RightsError> {
        check_note(grant)?;
        match raw {
            "granted" if grant.trim().is_empty() => Err(RightsError::EmptyLicenceGrant),
            "granted" => Ok(Self::Granted {
                grant: grant.to_string(),
            }),
            "absent" => Ok(Self::Absent),
            "undetermined" => Ok(Self::Undetermined),
            other => Err(RightsError::UnknownLicence {
                found: other.chars().take(64).collect(),
            }),
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Granted { .. } => "granted",
            Self::Absent => "absent",
            Self::Undetermined => "undetermined",
        }
    }

    #[must_use]
    pub fn grant(&self) -> &str {
        match self {
            Self::Granted { grant } => grant,
            _ => "",
        }
    }

    fn verdict(&self) -> AxisVerdict {
        match self {
            Self::Granted { .. } => AxisVerdict::Permit,
            Self::Absent => AxisVerdict::Block("no licence grant covers this material".to_string()),
            Self::Undetermined => AxisVerdict::Undecided,
        }
    }
}

/// Whether the *other people's* material inside it may be redistributed.
///
/// A third axis, because it expires and because it is the one the strategy
/// document is actually asking about. [`Self::NotApplicable`] is a real state
/// and not a synonym for "granted": it is the claim that there is nobody else
/// in the material, which is a different statement from "everyone in it said
/// yes" and is false for any clip carrying a chat overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Consent {
    /// Somebody agreed, for a stated scope, optionally until a date.
    Granted {
        scope: String,
        valid_until: Option<CalendarDate>,
    },
    /// Somebody declined. A decision, and it blocks.
    Refused { reason: String },
    /// Nobody else's material is in here. A decision by the creator.
    NotApplicable,
    /// Nobody has asked. Blocks.
    Undetermined,
}

impl Consent {
    /// Parse the stored spelling, with its text and its optional expiry.
    pub fn parse(raw: &str, scope: &str, valid_until: Option<&str>) -> Result<Self, RightsError> {
        check_note(scope)?;
        let until = valid_until.map(str::parse::<CalendarDate>).transpose()?;
        match raw {
            "granted" if scope.trim().is_empty() => Err(RightsError::EmptyConsentScope),
            "granted" => Ok(Self::Granted {
                scope: scope.to_string(),
                valid_until: until,
            }),
            "refused" if scope.trim().is_empty() => Err(RightsError::EmptyConsentReason),
            "refused" => Ok(Self::Refused {
                reason: scope.to_string(),
            }),
            "not_required" => Ok(Self::NotApplicable),
            "undetermined" => Ok(Self::Undetermined),
            other => Err(RightsError::UnknownConsent {
                found: other.chars().take(64).collect(),
            }),
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Granted { .. } => "granted",
            Self::Refused { .. } => "refused",
            Self::NotApplicable => "not_required",
            Self::Undetermined => "undetermined",
        }
    }

    /// The scope or the reason, whichever this variant carries. The stored
    /// `consent_scope` column holds either, so there is one text field rather
    /// than two that could disagree about what was said.
    #[must_use]
    pub fn text(&self) -> &str {
        match self {
            Self::Granted { scope, .. } => scope,
            Self::Refused { reason } => reason,
            Self::NotApplicable | Self::Undetermined => "",
        }
    }

    #[must_use]
    pub fn valid_until(&self) -> Option<CalendarDate> {
        match self {
            Self::Granted { valid_until, .. } => *valid_until,
            _ => None,
        }
    }

    /// The verdict **at a date the caller supplies**.
    ///
    /// No clock is read here. A lapsed grant is [`AxisVerdict::Undecided`]
    /// rather than a block: nobody refused, the permission simply ran out, and
    /// sending the operator to "the participant declined" would be a lie. It
    /// still does not publish, which is the part that matters.
    fn verdict(&self, as_of: CalendarDate) -> AxisVerdict {
        match self {
            Self::Granted { valid_until, .. } => match valid_until {
                Some(until) if as_of > *until => AxisVerdict::Undecided,
                _ => AxisVerdict::Permit,
            },
            Self::Refused { reason } => AxisVerdict::Block(format!("consent refused: {reason}")),
            Self::NotApplicable => AxisVerdict::Permit,
            Self::Undetermined => AxisVerdict::Undecided,
        }
    }
}

/// One recorded decision about one source: whose material it is, what may be
/// done with it, and what the participants in it agreed.
///
/// **It carries no eligibility.** There is no `publishable` field here and no
/// column for one in the schema, which is what makes the gate a gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceNote {
    /// The identity the note is about. Not a path, and never one.
    pub source: SourceAssetId,
    pub ownership: Ownership,
    pub licence: Licence,
    pub consent: Consent,
    /// What the creator still owes, in their own words. Carried verbatim and
    /// never parsed; it is the disclosure the plan asks to travel with the
    /// release package, and it is text a renderer escapes.
    pub disclosure: String,
    pub recorded_at: String,
    /// Who made this decision. An empty reviewer is
    /// [`RightsError::EmptyReviewer`]: "a licence nobody reviewed" is not the
    /// same fact as "reviewed and permitted".
    pub recorded_by: String,
}

impl SourceNote {
    /// Refuse a note whose decision has no content, before it can be stored.
    ///
    /// Each variant re-parses its own axis rather than trusting the field a
    /// caller built, so a `Licence::Granted { grant: "" }` assembled by hand is
    /// refused the same way one that came from [`Licence::parse` would be.
    pub fn validated(self) -> Result<Self, RightsError> {
        check_note(&self.disclosure)?;
        if self.recorded_by.trim().is_empty() {
            return Err(RightsError::EmptyReviewer);
        }
        let ownership = Ownership::parse(self.ownership.as_str(), self.ownership.holder())?;
        let licence = Licence::parse(self.licence.as_str(), self.licence.grant())?;
        let consent = Consent::parse(
            self.consent.as_str(),
            self.consent.text(),
            self.consent.valid_until().map(|d| d.as_str()).as_deref(),
        )?;
        Ok(Self {
            ownership,
            licence,
            consent,
            ..self
        })
    }

    /// The derived verdict, at a date the caller supplies.
    #[must_use]
    pub fn eligibility(&self, as_of: &CalendarDate) -> Eligibility {
        Eligibility::from_axes([
            (Axis::Ownership, self.ownership.verdict()),
            (Axis::Licence, self.licence.verdict()),
            (Axis::Consent, self.consent.verdict(*as_of)),
        ])
    }

    /// The content fingerprint of the **decision**, not of when it was written.
    ///
    /// `recorded_at` is excluded for the reason
    /// [`super::workspace::revision_fingerprint`] excludes it: re-saving an
    /// unchanged note is not a change to it, and a review that went stale
    /// because somebody pressed save again is a review that gets ignored.
    /// Neither is the reviewer, for the same reason.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let mut digest = <sha2::Sha256 as sha2::Digest>::new();
        let mut feed = |part: &str| {
            sha2::Digest::update(&mut digest, part.as_bytes());
            // A separator, so ("ab","c") and ("a","bc") cannot share a digest.
            sha2::Digest::update(&mut digest, b"\x1f");
        };
        feed(&self.source.to_string());
        feed(self.ownership.as_str());
        feed(self.ownership.holder());
        feed(self.licence.as_str());
        feed(self.licence.grant());
        feed(self.consent.as_str());
        feed(self.consent.text());
        match self.consent.valid_until() {
            Some(d) => feed(&d.as_str()),
            None => feed(""),
        }
        feed(&self.disclosure);
        sha2::Digest::finalize(digest)
            .iter()
            .fold(String::with_capacity(64), |mut hex, byte| {
                use std::fmt::Write as _;
                let _ = write!(hex, "{byte:02x}");
                hex
            })
    }
}

/// Whether a `fingerprint` taken at review time still describes the note.
///
/// The one staleness test, on the same content-comparison rule as
/// [`super::render_plan::is_stale`] and for the same reason: ids and timestamps
/// are not what changed.
#[must_use]
pub fn is_stale(recorded: &str, note: &SourceNote) -> bool {
    recorded != note.fingerprint()
}

/// The verdict for a source that may have no note at all.
///
/// **A missing note is undecided on every axis**, and therefore not
/// publishable. This function exists so that answer is written down in one
/// place: a caller that reaches for `note.eligibility(..)` on an `Option` has
/// to decide what the `None` means, and the fail-open answer is the one a
/// `map_or(true, ..)` would reach for by accident.
#[must_use]
pub fn eligibility_of(note: Option<&SourceNote>, as_of: &CalendarDate) -> Eligibility {
    match note {
        Some(n) => n.eligibility(as_of),
        None => Eligibility::from_axes([
            (Axis::Ownership, AxisVerdict::Undecided),
            (Axis::Licence, AxisVerdict::Undecided),
            (Axis::Consent, AxisVerdict::Undecided),
        ]),
    }
}

/// One of the three axes, for naming which one a verdict is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Ownership,
    Licence,
    Consent,
}

impl Axis {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ownership => "ownership",
            Self::Licence => "licence",
            Self::Consent => "consent",
        }
    }
}

impl fmt::Display for Axis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What one axis says. Internal to the derivation, and the reason
/// [`Eligibility`] can name a reason without the axes agreeing to expose one.
enum AxisVerdict {
    Permit,
    Block(String),
    Undecided,
}

/// One axis's refusal, with the fact that caused it. Named rather than a bare
/// string so a caller can tell "somebody said no" from "nobody has looked"
/// without matching on prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxisBlocker {
    pub axis: Axis,
    pub reason: String,
}

/// The verdict. **Derived, never stored** — see the module doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Eligibility {
    /// Every axis permits. The only state that publishes.
    Publishable,
    /// At least one axis was decided and refuses.
    Refused { blockers: Vec<AxisBlocker> },
    /// Nothing refused it; at least one axis has no decision. Still not
    /// publishable — see the module doc on why undecided is not permitted.
    Undetermined { undecided: Vec<Axis> },
}

impl Eligibility {
    /// The single construction path, from the three axis verdicts.
    ///
    /// Ordered ownership, licence, consent — the order a person reads them in,
    /// and the order a UI lists blockers in.
    fn from_axes<const N: usize>(axes: [(Axis, AxisVerdict); N]) -> Self {
        let mut blockers = Vec::new();
        let mut undecided = Vec::new();
        for (axis, verdict) in axes {
            match verdict {
                AxisVerdict::Permit => {}
                AxisVerdict::Block(reason) => blockers.push(AxisBlocker { axis, reason }),
                AxisVerdict::Undecided => undecided.push(axis),
            }
        }
        // A refusal outranks an undecided axis: if somebody said no, "not
        // looked at yet" would be the wrong thing to tell an operator.
        if !blockers.is_empty() {
            Self::Refused { blockers }
        } else if !undecided.is_empty() {
            Self::Undetermined { undecided }
        } else {
            Self::Publishable
        }
    }

    /// Whether this source may be published. The only boolean, and it is
    /// computed — there is nowhere to store the answer.
    #[must_use]
    pub fn is_publishable(&self) -> bool {
        matches!(self, Self::Publishable)
    }

    /// The axes that refused, in reading order. Empty unless [`Self::Refused`].
    #[must_use]
    pub fn blockers(&self) -> &[AxisBlocker] {
        match self {
            Self::Refused { blockers } => blockers,
            _ => &[],
        }
    }

    /// The axes with no decision, in reading order. Empty unless
    /// [`Self::Undetermined`].
    #[must_use]
    pub fn undecided(&self) -> &[Axis] {
        match self {
            Self::Undetermined { undecided } => undecided,
            _ => &[],
        }
    }

    /// The gate: `Ok(())` only for [`Self::Publishable`], otherwise a typed
    /// refusal naming every axis and why.
    pub fn require_publishable(&self) -> Result<(), Ineligible> {
        if self.is_publishable() {
            return Ok(());
        }
        Err(Ineligible {
            blockers: self.blockers().to_vec(),
            undecided: self.undecided().to_vec(),
        })
    }
}

impl fmt::Display for Eligibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Publishable => f.write_str("publishable"),
            Self::Refused { blockers } => {
                write!(f, "refused: ")?;
                for (n, b) in blockers.iter().enumerate() {
                    if n > 0 {
                        f.write_str("; ")?;
                    }
                    write!(f, "{}: {}", b.axis, b.reason)?;
                }
                Ok(())
            }
            Self::Undetermined { undecided } => {
                f.write_str("undecided: ")?;
                for (n, axis) in undecided.iter().enumerate() {
                    if n > 0 {
                        f.write_str(", ")?;
                    }
                    f.write_str(axis.as_str())?;
                }
                f.write_str(" — nobody has recorded a decision")
            }
        }
    }
}

/// What [`Eligibility::require_publishable`] hands back: every axis, and why.
///
/// Carries **both** lists even though at most one is non-empty, so a caller
/// rendering the refusal does not have to know which variant it came from in
/// order to be complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ineligible {
    pub blockers: Vec<AxisBlocker>,
    pub undecided: Vec<Axis>,
}

impl fmt::Display for Ineligible {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.blockers.is_empty() {
            f.write_str("refused: ")?;
            for (n, b) in self.blockers.iter().enumerate() {
                if n > 0 {
                    f.write_str("; ")?;
                }
                write!(f, "{}: {}", b.axis, b.reason)?;
            }
            return Ok(());
        }
        write!(
            f,
            "undecided: {} — nobody has recorded a decision",
            self.undecided
                .iter()
                .map(|a| a.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

/// Everything this model refuses, each with the fact that caused it.
///
/// `Database` is carried rather than flattened: a store caller's refusal stays
/// a typed state instead of a message to pattern-match, and message-matching is
/// what this type exists to avoid.
#[derive(Debug, PartialEq)]
pub enum RightsError {
    /// A licence a person stated, with nothing attached to it.
    EmptyLicenceGrant,
    /// A participant agreed to nothing in particular.
    EmptyConsentScope,
    /// A participant declined, with no reason given.
    EmptyConsentReason,
    /// Third-party material with nobody named.
    EmptyHolder,
    /// Nobody recorded who made the decision.
    EmptyReviewer,
    /// A NUL byte in free text. See [`check_note`].
    NulInNote,
    /// Free text over [`MAX_NOTE_BYTES`]. A refusal, never a truncation.
    NoteTooLong {
        bytes: usize,
        limit: usize,
    },
    /// A date that is not a calendar date.
    MalformedDate {
        found: String,
    },
    /// A row whose `source_asset_id` is not a UUID.
    MalformedSourceId {
        found: String,
    },
    /// A stored licence spelling this build does not know. Reported, never
    /// defaulted — a corrupt or hand-edited row must not quietly stop gating.
    UnknownLicence {
        found: String,
    },
    UnknownConsent {
        found: String,
    },
    UnknownOwnership {
        found: String,
    },
    Database(rusqlite::Error),
}

impl fmt::Display for RightsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyLicenceGrant => {
                f.write_str("a licence is recorded as granted with no grant stated")
            }
            Self::EmptyConsentScope => {
                f.write_str("consent is recorded as granted with no scope stated")
            }
            Self::EmptyConsentReason => {
                f.write_str("consent is recorded as refused with no reason stated")
            }
            Self::EmptyHolder => {
                f.write_str("this is third-party material with nobody named as the holder")
            }
            Self::EmptyReviewer => f.write_str("nobody is recorded as having made this decision"),
            Self::NulInNote => f.write_str("note contains a NUL byte"),
            Self::NoteTooLong { bytes, limit } => {
                write!(f, "note is {bytes} bytes, over the {limit}-byte limit")
            }
            Self::MalformedDate { found } => write!(f, "not a calendar date: {found:?}"),
            Self::MalformedSourceId { found } => {
                write!(f, "not a source identity: {found:?}")
            }
            Self::UnknownLicence { found } => {
                write!(f, "not a recorded licence decision: {found:?}")
            }
            Self::UnknownConsent { found } => {
                write!(f, "not a recorded consent decision: {found:?}")
            }
            Self::UnknownOwnership { found } => {
                write!(f, "not a recorded ownership decision: {found:?}")
            }
            Self::Database(e) => write!(f, "database: {e}"),
        }
    }
}

impl std::error::Error for RightsError {}

impl From<rusqlite::Error> for RightsError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Database(e)
    }
}

impl From<RightsError> for crate::ResearchError {
    fn from(e: RightsError) -> Self {
        Self::Validation(e.to_string())
    }
}

fn check_note(raw: &str) -> Result<(), RightsError> {
    if raw.len() > MAX_NOTE_BYTES {
        return Err(RightsError::NoteTooLong {
            bytes: raw.len(),
            limit: MAX_NOTE_BYTES,
        });
    }
    // A NUL is refused rather than carried. This is not a purity rule: SQLite's
    // `LENGTH()` — and `TRIM()` — stop at the first NUL, so a note beginning
    // with one measures as zero characters and the table's own
    // "a decision has something attached" `CHECK` rejects the row. Refusing it
    // here with a named reason is strictly better than letting the caller
    // discover it as `CHECK constraint failed`, and it is also the safer
    // default for any C consumer downstream, where a NUL truncates a string.
    if raw.contains('\0') {
        return Err(RightsError::NulInNote);
    }
    Ok(())
}
