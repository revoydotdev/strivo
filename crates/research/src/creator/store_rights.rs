//! Persistence for the source rights notes, and the release gate that reads
//! them.
//!
//! Two statements and two reads, which is the point: a note is written by
//! [`Store::record_note`] and by nothing else, and eligibility is read by
//! [`Store::publication_report`] and nowhere else. There is no
//! `set_publishable`, because there is no stored eligibility to set.
//!
//! ## The upsert replaces, it does not accumulate
//!
//! Re-recording a decision **replaces** the row. Two rows for one source would
//! give "what did this operator decide about this file" two answers, and the
//! wrong one would be whichever the reader happened to find first.
//!
//! ## The fingerprint is stored, not recomputed on read
//!
//! `creator_source_notes.fingerprint` is written with the note and read back
//! with it, so a caller that reviewed a note can compare rather than
//! re-derive. It is still [`SourceNote::fingerprint`]'s output — the column is
//! a copy of a derived value, not a second definition of it, and
//! `a_note_that_says_licensed_coexists_...` style tests read the note back and
//! assert the two agree.

use rusqlite::{params, OptionalExtension};

use super::ids::SourceAssetId;
use super::rights::{
    CalendarDate, Consent, Eligibility, Licence, Ownership, RightsError, SourceNote,
};
use super::store::Store;

const COLUMNS: &str = "source_asset_id,ownership,holder,licence,licence_grant,consent,\
     consent_scope,consent_valid_until,disclosure,fingerprint,recorded_by,recorded_at";

type Result<T> = std::result::Result<T, RightsError>;

impl Store {
    /// Record what a human decided about one source, replacing any earlier
    /// decision about it.
    ///
    /// The note is validated first, so a decision with nothing attached to it
    /// never reaches a row. The fingerprint travels with it.
    ///
    /// A source id nothing was ever inserted under is refused by the foreign
    /// key: a note filed against an identity no other row can reach is a note
    /// that will never be read by anything, and would be indistinguishable
    /// later from a note that was deleted on purpose.
    pub fn record_note(&self, note: &SourceNote) -> Result<()> {
        let note = note.clone().validated()?;
        self.conn.execute(
            "INSERT INTO creator_source_notes
             (source_asset_id,ownership,holder,licence,licence_grant,consent,consent_scope,
              consent_valid_until,disclosure,fingerprint,recorded_by,recorded_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
             ON CONFLICT(source_asset_id) DO UPDATE SET
               ownership=excluded.ownership, holder=excluded.holder,
               licence=excluded.licence, licence_grant=excluded.licence_grant,
               consent=excluded.consent, consent_scope=excluded.consent_scope,
               consent_valid_until=excluded.consent_valid_until,
               disclosure=excluded.disclosure, fingerprint=excluded.fingerprint,
               recorded_by=excluded.recorded_by, recorded_at=excluded.recorded_at",
            params![
                note.source.to_string(),
                note.ownership.as_str(),
                note.ownership.holder(),
                note.licence.as_str(),
                note.licence.grant(),
                note.consent.as_str(),
                note.consent.text(),
                note.consent.valid_until().map(CalendarDate::as_str),
                note.disclosure,
                note.fingerprint(),
                note.recorded_by,
                note.recorded_at,
            ],
        )?;
        Ok(())
    }

    /// The note for one source, or `None` where none has been recorded.
    ///
    /// `None` is a real state, not an error: it is every source imported in the
    /// last second, and [`super::rights::eligibility_of`] is what turns it into
    /// *undecided on all three axes* rather than into a default.
    ///
    /// A row whose stored spelling is unrecognised is a typed refusal rather
    /// than a default, so a corrupt or hand-edited row cannot quietly stop
    /// gating.
    pub fn source_note(&self, source: SourceAssetId) -> Result<Option<SourceNote>> {
        let raw = self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM creator_source_notes WHERE source_asset_id=?1"),
                [source.to_string()],
                row_to_raw,
            )
            .optional()?;
        raw.map(note_from_raw).transpose()
    }

    /// The gate: may a release made of these sources be published, and if not,
    /// which source is the problem.
    ///
    /// One verdict per source, so a caller rendering the refusal names the
    /// blocking *file* rather than "a source". A source that names no row is
    /// undecided on all three axes and therefore blocks — the same fail-closed
    /// reading [`super::rights::eligibility_of`] gives, applied per source
    /// rather than to one.
    pub fn publication_report(
        &self,
        sources: &[SourceAssetId],
        as_of: &CalendarDate,
    ) -> Result<PublicationReport> {
        let verdicts = sources
            .iter()
            .map(|id| {
                let note = self.source_note(*id)?;
                Ok((*id, super::rights::eligibility_of(note.as_ref(), as_of)))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(PublicationReport { verdicts })
    }
}

/// Every source's verdict for one release check.
///
/// A small struct rather than a `bool` because a report that cannot name the
/// blocking source is a report whose only use is the answer, and the plan asks
/// for the reason to be shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationReport {
    verdicts: Vec<(SourceAssetId, Eligibility)>,
}

impl PublicationReport {
    /// Whether every source in it permits publication.
    #[must_use]
    pub fn is_publishable(&self) -> bool {
        self.verdicts.iter().all(|(_, e)| e.is_publishable())
    }

    /// One source's verdict, or `None` for a source that was not in the check.
    #[must_use]
    pub fn verdict_for(&self, source: SourceAssetId) -> Option<&Eligibility> {
        self.verdicts
            .iter()
            .find(|(id, _)| *id == source)
            .map(|(_, e)| e)
    }

    /// The sources that do not permit publication, in the order they were
    /// checked. Empty on a clean report.
    #[must_use]
    pub fn blocking_sources(&self) -> Vec<SourceAssetId> {
        self.verdicts
            .iter()
            .filter(|(_, e)| !e.is_publishable())
            .map(|(id, _)| *id)
            .collect()
    }

    /// Every verdict, for a caller that renders them all.
    #[must_use]
    pub fn verdicts(&self) -> &[(SourceAssetId, Eligibility)] {
        &self.verdicts
    }
}

impl std::fmt::Display for PublicationReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.verdicts.is_empty() {
            return f.write_str("no sources to check");
        }
        for (n, (id, verdict)) in self.verdicts.iter().enumerate() {
            if n > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{id}: {verdict}")?;
        }
        Ok(())
    }
}

/// The row as it came out of SQLite, with no interpretation applied.
///
/// Kept separate from the interpretation so a bad *value* is a typed
/// [`RightsError`] rather than a `rusqlite::Error` wearing its message. Doing
/// the parsing inside the `query_row` closure forces every refusal through
/// `rusqlite::Error::InvalidColumnType`, which is precisely how a caller ends
/// up unable to tell "this row is corrupt" from "the database failed" — and a
/// caller that cannot tell those two apart reports neither.
struct RawNote {
    source: String,
    ownership: String,
    holder: String,
    licence: String,
    licence_grant: String,
    consent: String,
    consent_scope: String,
    consent_valid_until: Option<String>,
    disclosure: String,
    recorded_by: String,
    recorded_at: String,
}

fn row_to_raw(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawNote> {
    Ok(RawNote {
        source: r.get(0)?,
        ownership: r.get(1)?,
        holder: r.get(2)?,
        licence: r.get(3)?,
        licence_grant: r.get(4)?,
        consent: r.get(5)?,
        consent_scope: r.get(6)?,
        consent_valid_until: r.get(7)?,
        disclosure: r.get(8)?,
        recorded_by: r.get(10)?,
        recorded_at: r.get(11)?,
    })
}

fn note_from_raw(raw: RawNote) -> Result<SourceNote> {
    let source: SourceAssetId =
        raw.source
            .parse()
            .map_err(|e: crate::ResearchError| RightsError::MalformedSourceId {
                found: format!("{:?}: {e}", raw.source),
            })?;
    let ownership = Ownership::parse(&raw.ownership, &raw.holder)?;
    let licence = Licence::parse(&raw.licence, &raw.licence_grant)?;
    let consent = Consent::parse(
        &raw.consent,
        &raw.consent_scope,
        raw.consent_valid_until.as_deref(),
    )?;
    Ok(SourceNote {
        source,
        ownership,
        licence,
        consent,
        disclosure: raw.disclosure,
        recorded_by: raw.recorded_by,
        recorded_at: raw.recorded_at,
    })
}
