//! The row boundary: a `creator_history` row and the value it becomes.
//!
//! One direction only — this file is the sole place a stored column becomes a
//! typed field, so a malformed id or an unreadable patch is reported on the read
//! that would have used it rather than by whichever verb happened to hit it
//! first. A `parent_id` that no longer resolves is deliberately *not* refused
//! here: it is left as the typed value it is, and the walk in
//! [`strivo_editor::history::History::chain`] is where a dangling parent
//! becomes a reported [`HistoryError::UnknownEntry`].

use super::super::history::{HistoryEntry, TranscriptEdit};
use super::super::HistoryStoreError;
use strivo_editor::history::Entry;

pub(super) fn to_algebra(entry: &HistoryEntry) -> Entry {
    Entry {
        key: entry.id.to_string(),
        parent: entry.parent.map(|p| p.to_string()),
        replaced: entry.replaced.map(|r| r.to_string()),
        label: entry.label.clone(),
        created_at: entry.created_at.clone(),
    }
}

pub(super) fn json_error(e: serde_json::Error) -> HistoryStoreError {
    HistoryStoreError::Storage(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}

/// A stored id that will not parse is a conversion failure at `column`, not a
/// generic error: the column is what a caller needs to know was wrong.
fn bad_id(column: u32, e: crate::ResearchError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column as usize,
        rusqlite::types::Type::Text,
        Box::new(e),
    )
}

/// Column offsets, named because a conversion failure reports one and a bare
/// integer in that report is unreadable.
const COL_ID: u32 = 0;
const COL_PROJECT: u32 = 1;
const COL_PARENT: u32 = 2;
const COL_REPLACED: u32 = 3;
const COL_COMPOSITION: u32 = 4;
const COL_TRANSCRIPT: u32 = 5;
const COL_PATCH: u32 = 6;

pub(super) fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
    let transcript: Option<String> = r.get(5)?;
    let patch: String = r.get(6)?;
    let transcript = match transcript {
        Some(id) => Some(TranscriptEdit {
            revision: id.parse().map_err(|e| bad_id(COL_TRANSCRIPT, e))?,
            patch: serde_json::from_str(&patch).map_err(|e| bad_id(COL_PATCH, e.into()))?,
        }),
        None => None,
    };
    Ok(HistoryEntry {
        id: r
            .get::<_, String>(0)?
            .parse()
            .map_err(|e| bad_id(COL_ID, e))?,
        project_id: r
            .get::<_, String>(1)?
            .parse()
            .map_err(|e| bad_id(COL_PROJECT, e))?,
        parent: r
            .get::<_, Option<String>>(2)?
            .map(|p| p.parse())
            .transpose()
            .map_err(|e| bad_id(COL_PARENT, e))?,
        replaced: r
            .get::<_, Option<String>>(3)?
            .map(|p| p.parse())
            .transpose()
            .map_err(|e| bad_id(COL_REPLACED, e))?,
        composition_revision_id: r
            .get::<_, String>(4)?
            .parse()
            .map_err(|e| bad_id(COL_COMPOSITION, e))?,
        transcript,
        label: r.get(7)?,
        created_at: r.get(8)?,
    })
}
