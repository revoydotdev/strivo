//! Lexical search over `transcript.utterance` signals (CE-Fusion F1).
//!
//! Matching uses SQLite's FTS5 virtual-table module, confirmed available in
//! the workspace's bundled rusqlite build by [`tests::fts5_probe`] below —
//! no Cargo feature change was needed. The index itself is built as a
//! **temp** table, scoped to one connection and repopulated from the
//! project's `transcript.utterance` signals on every call, rather than as a
//! persistent table wired into `lib.rs`'s shared `SCHEMA` with insert
//! triggers. `ResearchStore::open` hands out a fresh connection per request
//! (see `strivo-web`'s `research_db()` handlers), so a temp index costs one
//! project-scoped, index-backed scan (`idx_signals_project_kind`) per
//! search — simplest correct thing first; if corpora outgrow that, a
//! persistent FTS5 table with sync triggers is the natural follow-up.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{parse_json, parse_uuid, ResearchError, ResearchStore, Result};

/// One transcript-search hit: a signal resolved to its source and time
/// range, with the matched text and surrounding context.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TranscriptHit {
    pub signal_id: Uuid,
    pub source_id: Uuid,
    pub t_start_ms: u64,
    pub t_end_ms: u64,
    pub snippet: String,
    pub confidence: Option<f64>,
}

impl ResearchStore {
    /// Case-insensitive lexical search over `transcript.utterance` signal
    /// text for one project. The query is matched as an FTS5 phrase (tokens
    /// must appear adjacent, in order) so arbitrary user input can't be
    /// interpreted as FTS5 query syntax. Results are ordered deterministically
    /// by `(source_id, start_ms, id)` — not by FTS5 relevance rank — so
    /// pagination across `limit`/`offset` pages is stable and reproducible.
    /// Cross-project isolation is enforced by scoping both the index build
    /// and the query to `project_id`.
    pub fn search_transcripts(
        &self,
        project_id: Uuid,
        query: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<TranscriptHit>> {
        let query = validate_query(query)?;
        let limit = validate_limit(limit)?;

        rebuild_index(&self.conn, project_id)?;

        let phrase = format!("\"{}\"", query.replace('"', "\"\""));
        let mut stmt = self.conn.prepare(
            "SELECT signals.id, signals.source_id, signals.start_ms, signals.end_ms,
                    signals.confidence,
                    snippet(transcript_search_fts, 0, '', '', '…', 12)
             FROM transcript_search_fts
             JOIN signals ON signals.id = transcript_search_fts.signal_id
             WHERE transcript_search_fts MATCH ?1
             ORDER BY signals.source_id, signals.start_ms, signals.id
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(params![phrase, limit as i64, offset as i64], |row| {
            Ok(TranscriptHit {
                signal_id: parse_uuid(row.get::<_, String>(0)?)?,
                source_id: parse_uuid(row.get::<_, String>(1)?)?,
                t_start_ms: row.get::<_, i64>(2)?.max(0) as u64,
                t_end_ms: row.get::<_, i64>(3)?.max(0) as u64,
                confidence: row.get(4)?,
                snippet: row.get(5)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
}

/// (Re)build the temp FTS5 index for `project_id`'s `transcript.utterance`
/// signals. Idempotent: safe to call once per `search_transcripts` call on
/// the same connection.
fn rebuild_index(conn: &Connection, project_id: Uuid) -> Result<()> {
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS temp.transcript_search_fts
         USING fts5(text, signal_id UNINDEXED, tokenize = 'unicode61');",
    )?;
    conn.execute("DELETE FROM temp.transcript_search_fts", [])?;

    let mut select = conn.prepare(
        "SELECT id, payload_json FROM signals
         WHERE project_id = ?1 AND kind = 'transcript.utterance'",
    )?;
    let mut insert =
        conn.prepare("INSERT INTO temp.transcript_search_fts(text, signal_id) VALUES (?1, ?2)")?;
    let rows = select.query_map(params![project_id.to_string()], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (signal_id, payload_json) = row?;
        let payload: Value = parse_json(payload_json)?;
        let Some(text) = payload.get("text").and_then(Value::as_str) else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        insert.execute(params![text, signal_id])?;
    }
    Ok(())
}

fn validate_query(query: &str) -> Result<&str> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Err(ResearchError::Validation(
            "search query cannot be empty".into(),
        ));
    }
    Ok(trimmed)
}

fn validate_limit(limit: u32) -> Result<u32> {
    if limit == 0 || limit > 200 {
        return Err(ResearchError::Validation(
            "search limit must be between 1 and 200".into(),
        ));
    }
    Ok(limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NewSignal, NewSource, SourceKind};

    /// Probe: does the bundled rusqlite build (workspace `Cargo.toml`, no
    /// `fts5` cargo feature) compile SQLite with the FTS5 virtual-table
    /// module? Confirmed available — `rusqlite 0.33` / bundled
    /// `libsqlite3-sys` compiles FTS5 in by default, so the search module
    /// uses it directly rather than falling back to substring matching.
    #[test]
    fn fts5_probe() {
        let conn = Connection::open_in_memory().unwrap();
        let result = conn.execute_batch("CREATE VIRTUAL TABLE probe USING fts5(text)");
        assert!(result.is_ok(), "FTS5 module not available: {result:?}");
    }

    fn source(project_id: Uuid, title: &str) -> NewSource {
        NewSource {
            id: Uuid::new_v4(),
            project_id,
            recording_id: None,
            kind: SourceKind::Recording,
            title: title.into(),
            uri: None,
            duration_ms: None,
            attributes: serde_json::json!({}),
        }
    }

    fn utterance(project_id: Uuid, source_id: Uuid, start_ms: u64, text: &str) -> NewSignal {
        NewSignal {
            id: Uuid::new_v4(),
            project_id,
            source_id,
            start_ms,
            end_ms: start_ms + 500,
            kind: "transcript.utterance".into(),
            label: "Speaker".into(),
            payload: serde_json::json!({"text": text}),
            confidence: Some(0.9),
            provenance_id: None,
        }
    }

    #[test]
    fn finds_hits_across_multiple_sources_in_one_project() {
        let store = ResearchStore::open(":memory:").unwrap();
        let project = store.create_project("Study", "").unwrap();
        let source_a = source(project.id, "Episode A");
        let source_b = source(project.id, "Episode B");
        store.upsert_source(&source_a).unwrap();
        store.upsert_source(&source_b).unwrap();
        store
            .append_signal(&utterance(
                project.id,
                source_a.id,
                1_000,
                "we discussed the speedrun strategy",
            ))
            .unwrap();
        store
            .append_signal(&utterance(
                project.id,
                source_b.id,
                2_000,
                "chat erupted during the speedrun",
            ))
            .unwrap();

        let hits = store
            .search_transcripts(project.id, "speedrun", 50, 0)
            .unwrap();
        assert_eq!(hits.len(), 2);
        let source_ids: std::collections::HashSet<_> =
            hits.iter().map(|hit| hit.source_id).collect();
        assert!(source_ids.contains(&source_a.id));
        assert!(source_ids.contains(&source_b.id));
    }

    #[test]
    fn matching_is_case_insensitive() {
        let store = ResearchStore::open(":memory:").unwrap();
        let project = store.create_project("Study", "").unwrap();
        let src = source(project.id, "Episode");
        store.upsert_source(&src).unwrap();
        store
            .append_signal(&utterance(
                project.id,
                src.id,
                0,
                "Hello World, this is a Test",
            ))
            .unwrap();

        let hits = store
            .search_transcripts(project.id, "HELLO world", 10, 0)
            .unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn snippet_contains_matched_context() {
        let store = ResearchStore::open(":memory:").unwrap();
        let project = store.create_project("Study", "").unwrap();
        let src = source(project.id, "Episode");
        store.upsert_source(&src).unwrap();
        store
            .append_signal(&utterance(
                project.id,
                src.id,
                0,
                "we built this together as a community",
            ))
            .unwrap();

        let hits = store
            .search_transcripts(project.id, "built", 10, 0)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.to_lowercase().contains("built"));
        assert!(hits[0].snippet.to_lowercase().contains("community"));
    }

    #[test]
    fn pagination_is_bounded_and_deterministic_across_pages() {
        let store = ResearchStore::open(":memory:").unwrap();
        let project = store.create_project("Study", "").unwrap();
        let src = source(project.id, "Episode");
        store.upsert_source(&src).unwrap();
        for i in 0..5u64 {
            store
                .append_signal(&utterance(
                    project.id,
                    src.id,
                    i * 1_000,
                    "recurring keyword appears here",
                ))
                .unwrap();
        }

        let page1 = store
            .search_transcripts(project.id, "keyword", 2, 0)
            .unwrap();
        let page2 = store
            .search_transcripts(project.id, "keyword", 2, 2)
            .unwrap();
        let page3 = store
            .search_transcripts(project.id, "keyword", 2, 4)
            .unwrap();
        assert_eq!(page1.len(), 2);
        assert_eq!(page2.len(), 2);
        assert_eq!(page3.len(), 1);

        let mut all_starts: Vec<u64> = page1
            .iter()
            .chain(page2.iter())
            .chain(page3.iter())
            .map(|hit| hit.t_start_ms)
            .collect();
        let mut sorted = all_starts.clone();
        sorted.sort_unstable();
        assert_eq!(
            all_starts, sorted,
            "pages must be in ascending start_ms order"
        );
        all_starts.dedup();
        assert_eq!(
            all_starts.len(),
            5,
            "no duplicate or missing hits across pages"
        );
    }

    #[test]
    fn rejects_empty_and_whitespace_queries() {
        let store = ResearchStore::open(":memory:").unwrap();
        let project = store.create_project("Study", "").unwrap();
        assert!(store.search_transcripts(project.id, "", 10, 0).is_err());
        assert!(store.search_transcripts(project.id, "   ", 10, 0).is_err());
    }

    #[test]
    fn rejects_out_of_range_limits() {
        let store = ResearchStore::open(":memory:").unwrap();
        let project = store.create_project("Study", "").unwrap();
        assert!(store.search_transcripts(project.id, "hello", 0, 0).is_err());
        assert!(store
            .search_transcripts(project.id, "hello", 201, 0)
            .is_err());
        assert!(store
            .search_transcripts(project.id, "hello", 200, 0)
            .is_ok());
    }

    #[test]
    fn cross_project_results_never_leak() {
        let store = ResearchStore::open(":memory:").unwrap();
        let first = store.create_project("First", "").unwrap();
        let second = store.create_project("Second", "").unwrap();
        let src_first = source(first.id, "Episode");
        let src_second = source(second.id, "Episode");
        store.upsert_source(&src_first).unwrap();
        store.upsert_source(&src_second).unwrap();
        store
            .append_signal(&utterance(
                first.id,
                src_first.id,
                0,
                "shared keyword only in first project",
            ))
            .unwrap();
        store
            .append_signal(&utterance(
                second.id,
                src_second.id,
                0,
                "shared keyword only in second project",
            ))
            .unwrap();

        let first_hits = store
            .search_transcripts(first.id, "keyword", 10, 0)
            .unwrap();
        let second_hits = store
            .search_transcripts(second.id, "keyword", 10, 0)
            .unwrap();
        assert_eq!(first_hits.len(), 1);
        assert_eq!(second_hits.len(), 1);
        assert_eq!(first_hits[0].source_id, src_first.id);
        assert_eq!(second_hits[0].source_id, src_second.id);
    }
}
