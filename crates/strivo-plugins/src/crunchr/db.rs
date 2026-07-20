use std::path::Path;

use anyhow::Result;
use rusqlite::Connection;

use super::types::SearchResult;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS videos (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    recording_id    TEXT UNIQUE NOT NULL,
    channel_name    TEXT NOT NULL,
    title           TEXT NOT NULL,
    video_path      TEXT,
    audio_path      TEXT,
    transcript_text TEXT,
    status          TEXT DEFAULT 'pending'
                    CHECK(status IN ('pending','extracting_audio','transcribing','chunking','analyzing','complete','failed')),
    error_message   TEXT,
    created_at      TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS segments (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    video_id        INTEGER REFERENCES videos(id) ON DELETE CASCADE,
    segment_index   INTEGER NOT NULL,
    start_sec       REAL NOT NULL,
    end_sec         REAL NOT NULL,
    text            TEXT NOT NULL,
    speaker         TEXT,
    confidence      REAL,
    UNIQUE(video_id, segment_index)
);

CREATE TABLE IF NOT EXISTS chunks (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    video_id        INTEGER REFERENCES videos(id) ON DELETE CASCADE,
    chunk_index     INTEGER NOT NULL,
    text            TEXT NOT NULL,
    start_sec       REAL,
    end_sec         REAL,
    token_count     INTEGER,
    embedding       BLOB,
    UNIQUE(video_id, chunk_index)
);

CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
    text,
    content=chunks,
    content_rowid=id,
    tokenize='porter unicode61'
);

CREATE TRIGGER IF NOT EXISTS chunks_fts_ai AFTER INSERT ON chunks BEGIN
    INSERT INTO chunks_fts(rowid, text) VALUES (new.id, new.text);
END;

CREATE TRIGGER IF NOT EXISTS chunks_fts_ad AFTER DELETE ON chunks BEGIN
    INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;

CREATE TRIGGER IF NOT EXISTS chunks_fts_au AFTER UPDATE ON chunks BEGIN
    INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES ('delete', old.id, old.text);
    INSERT INTO chunks_fts(rowid, text) VALUES (new.id, new.text);
END;

CREATE TABLE IF NOT EXISTS word_frequency (
    video_id        INTEGER REFERENCES videos(id) ON DELETE CASCADE,
    word            TEXT NOT NULL,
    count           INTEGER NOT NULL,
    PRIMARY KEY(video_id, word)
);

CREATE TABLE IF NOT EXISTS tfidf_vocabulary (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    term            TEXT UNIQUE NOT NULL,
    doc_frequency   INTEGER DEFAULT 0,
    idf             REAL
);

CREATE TABLE IF NOT EXISTS video_analysis (
    video_id        INTEGER PRIMARY KEY REFERENCES videos(id) ON DELETE CASCADE,
    summary         TEXT,
    topics          TEXT,
    sentiment       TEXT,
    analyzed_at     TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_segments_video ON segments(video_id);
CREATE INDEX IF NOT EXISTS idx_chunks_video ON chunks(video_id);
CREATE INDEX IF NOT EXISTS idx_wordfreq_word ON word_frequency(word);
"#;

/// Open database connection and run schema migration.
pub fn open_and_init(db_path: &Path) -> Result<Connection> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(db_path)?;
    // WAL + a generous busy timeout so concurrent transcription jobs (e.g.
    // a whole playlist transcribing at once) wait for the write lock instead
    // of failing with "database is locked".
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=30000;",
    )?;
    conn.execute_batch(SCHEMA)?;
    // M5.6 — additive migration for cost tracking. ALTER TABLE ADD
    // COLUMN is idempotent only if we check first; rusqlite returns
    // an error on the second run, which we swallow because there's
    // no DROP/cleanup state to roll back. Default 0 keeps existing
    // rows valid.
    for stmt in [
        "ALTER TABLE videos ADD COLUMN prompt_tokens INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE videos ADD COLUMN completion_tokens INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE videos ADD COLUMN cost_cents INTEGER NOT NULL DEFAULT 0",
        // C5 — word-level timings from whisperx / voxtral preserved as
        // a JSON-array sidecar on the segment row. Format:
        // `[{"w":"hello","s":1.234,"e":1.380,"c":0.97}, ...]`
        // The Editor plugin reads this to render the transcript-as-
        // timeline with word-accurate in/out marks; Insights uses it
        // to compute speaker airtime by word count.
        "ALTER TABLE segments ADD COLUMN word_timings TEXT",
    ] {
        let _ = conn.execute(stmt, []);
    }
    Ok(conn)
}

/// Read the word-timings JSON for a segment. Returns `None` when the
/// transcribe backend didn't emit per-word timings (whisper-cli for
/// example) — callers fall back to segment-level start/end in that case.
#[allow(dead_code)] // consumed by Editor plugin (E1)
pub fn segment_word_timings(
    conn: &Connection,
    video_id: i64,
    segment_index: i64,
) -> Result<Option<String>> {
    let row: Option<Option<String>> = conn
        .query_row(
            "SELECT word_timings FROM segments WHERE video_id = ?1 AND segment_index = ?2",
            rusqlite::params![video_id, segment_index],
            |row| row.get(0),
        )
        .ok();
    Ok(row.flatten())
}

/// Persist a JSON-encoded word-timings array on the segment. Idempotent
/// — overwrites prior contents.
#[allow(dead_code)] // populated by transcribe backends in subsequent commit
pub fn set_segment_word_timings(
    conn: &Connection,
    video_id: i64,
    segment_index: i64,
    json: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE segments SET word_timings = ?1 WHERE video_id = ?2 AND segment_index = ?3",
        rusqlite::params![json, video_id, segment_index],
    )?;
    Ok(())
}

pub fn insert_video(
    conn: &Connection,
    recording_id: &str,
    channel_name: &str,
    title: &str,
    video_path: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT OR IGNORE INTO videos (recording_id, channel_name, title, video_path) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![recording_id, channel_name, title, video_path],
    )?;
    let id = conn.query_row(
        "SELECT id FROM videos WHERE recording_id = ?1",
        [recording_id],
        |row| row.get(0),
    )?;
    Ok(id)
}

pub fn update_video_status(
    conn: &Connection,
    recording_id: &str,
    status: &str,
    error: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE videos SET status = ?1, error_message = ?2 WHERE recording_id = ?3",
        rusqlite::params![status, error, recording_id],
    )?;
    Ok(())
}

pub fn update_video_audio_path(
    conn: &Connection,
    recording_id: &str,
    audio_path: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE videos SET audio_path = ?1 WHERE recording_id = ?2",
        rusqlite::params![audio_path, recording_id],
    )?;
    Ok(())
}

pub fn update_video_transcript(
    conn: &Connection,
    recording_id: &str,
    transcript: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE videos SET transcript_text = ?1 WHERE recording_id = ?2",
        rusqlite::params![transcript, recording_id],
    )?;
    Ok(())
}

pub fn insert_segments(
    conn: &Connection,
    video_id: i64,
    segments: &[(usize, f64, f64, &str, Option<&str>, Option<f64>)],
) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT OR REPLACE INTO segments (video_id, segment_index, start_sec, end_sec, text, speaker, confidence) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for (idx, start, end, text, speaker, confidence) in segments {
        stmt.execute(rusqlite::params![
            video_id, idx, start, end, text, speaker, confidence
        ])?;
    }
    Ok(())
}

pub fn insert_chunks(
    conn: &Connection,
    video_id: i64,
    chunks: &[(usize, &str, f64, f64, usize)],
) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT OR REPLACE INTO chunks (video_id, chunk_index, text, start_sec, end_sec, token_count) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for (idx, text, start, end, tokens) in chunks {
        stmt.execute(rusqlite::params![video_id, idx, text, start, end, tokens])?;
    }
    Ok(())
}

/// Chunk `(id, text)` rows for a video, in chunk order — the input to the
/// local embedding pass (vectorization).
pub fn chunks_for_embedding(conn: &Connection, video_id: i64) -> Result<Vec<(i64, String)>> {
    let mut stmt =
        conn.prepare("SELECT id, text FROM chunks WHERE video_id = ?1 ORDER BY chunk_index")?;
    let rows = stmt
        .query_map([video_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<(i64, String)>>>()?;
    Ok(rows)
}

/// Persist one chunk's embedding as a little-endian `f32` BLOB.
pub fn set_chunk_embedding(conn: &Connection, chunk_id: i64, blob: &[u8]) -> Result<()> {
    conn.execute(
        "UPDATE chunks SET embedding = ?1 WHERE id = ?2",
        rusqlite::params![blob, chunk_id],
    )?;
    Ok(())
}

pub fn insert_word_frequencies(
    conn: &Connection,
    video_id: i64,
    frequencies: &[(String, usize)],
) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT OR REPLACE INTO word_frequency (video_id, word, count) VALUES (?1, ?2, ?3)",
    )?;
    for (word, count) in frequencies {
        stmt.execute(rusqlite::params![video_id, word, count])?;
    }
    Ok(())
}

/// Sanitize a user query for FTS5 MATCH: wrap in double quotes to treat as literal phrase.
fn sanitize_fts_query(query: &str) -> String {
    // Escape internal double quotes and wrap in quotes for literal matching
    let escaped = query.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

pub fn fts_search(conn: &Connection, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
    fts_search_with_facets(conn, query, &SearchFacets::default(), limit)
}

/// Filters that narrow an FTS search post-hoc. Empty/None fields mean
/// "no filter on that field". (C4.)
#[derive(Debug, Clone, Default)]
pub struct SearchFacets {
    /// Restrict to chunks where any speaker matches one of these labels.
    pub speakers: Vec<String>,
    /// Channel-name substring match (case-insensitive).
    pub channel: Option<String>,
    /// `videos.created_at >= this RFC3339 timestamp` when set.
    pub since: Option<String>,
    /// `videos.created_at < this RFC3339 timestamp` when set.
    pub until: Option<String>,
    /// `video_analysis.sentiment LIKE 'positive%'` (or 'negative' /
    /// 'neutral'); maps to a case-insensitive prefix match.
    pub sentiment: Option<String>,
    /// Minimum video duration in seconds.
    pub min_duration_secs: Option<f64>,
    /// Maximum video duration in seconds.
    pub max_duration_secs: Option<f64>,
}

/// Faceted FTS search. Same shape as [`fts_search`] but the WHERE clause
/// also enforces channel / date / speaker / sentiment / duration
/// constraints. (C4 M4 stretch.)
pub fn fts_search_with_facets(
    conn: &Connection,
    query: &str,
    facets: &SearchFacets,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let safe_query = sanitize_fts_query(query);

    let mut sql = String::from(
        "SELECT c.id, v.title, v.channel_name, snippet(chunks_fts, 0, '>>>', '<<<', '...', 40), c.start_sec, c.end_sec, rank, v.video_path
         FROM chunks_fts
         JOIN chunks c ON c.id = chunks_fts.rowid
         JOIN videos v ON v.id = c.video_id",
    );
    let mut joins = Vec::new();
    let mut conds: Vec<String> = vec!["chunks_fts MATCH :q".into()];
    let mut params: Vec<(&str, Box<dyn rusqlite::ToSql>)> = vec![
        (":q", Box::new(safe_query)),
        (":lim", Box::new(limit as i64)),
    ];

    if let Some(ch) = &facets.channel {
        conds.push("LOWER(v.channel_name) LIKE :ch".into());
        params.push((":ch", Box::new(format!("%{}%", ch.to_lowercase()))));
    }
    if let Some(since) = &facets.since {
        conds.push("v.created_at >= :since".into());
        params.push((":since", Box::new(since.clone())));
    }
    if let Some(until) = &facets.until {
        conds.push("v.created_at < :until".into());
        params.push((":until", Box::new(until.clone())));
    }
    if let Some(sent) = &facets.sentiment {
        joins.push("LEFT JOIN video_analysis va ON va.video_id = v.id".to_string());
        conds.push("LOWER(va.sentiment) LIKE :sent".into());
        params.push((":sent", Box::new(format!("{}%", sent.to_lowercase()))));
    }
    if !facets.speakers.is_empty() {
        joins.push(
            "INNER JOIN segments seg ON seg.video_id = v.id \
             AND seg.start_sec <= c.end_sec \
             AND seg.end_sec   >= c.start_sec"
                .to_string(),
        );
        // SQLite parameter list expansion via JSON array — robust against
        // arbitrary speaker counts without dynamic placeholder generation.
        let json = serde_json::to_string(&facets.speakers)?;
        conds.push("seg.speaker IN (SELECT value FROM json_each(:spk))".into());
        params.push((":spk", Box::new(json)));
    }
    if let Some(min) = facets.min_duration_secs {
        // Recordings table stores duration via segments; approximate
        // using the MAX(end_sec) per video.
        joins.push(
            "INNER JOIN (SELECT video_id, MAX(end_sec) AS dur FROM segments GROUP BY video_id) dseg \
             ON dseg.video_id = v.id"
                .to_string(),
        );
        conds.push("dseg.dur >= :mind".into());
        params.push((":mind", Box::new(min)));
    }
    if let Some(max) = facets.max_duration_secs {
        if !joins.iter().any(|j| j.contains("dseg")) {
            joins.push(
                "INNER JOIN (SELECT video_id, MAX(end_sec) AS dur FROM segments GROUP BY video_id) dseg \
                 ON dseg.video_id = v.id"
                    .to_string(),
            );
        }
        conds.push("dseg.dur <= :maxd".into());
        params.push((":maxd", Box::new(max)));
    }

    for j in &joins {
        sql.push(' ');
        sql.push_str(j);
    }
    sql.push_str(" WHERE ");
    sql.push_str(&conds.join(" AND "));
    sql.push_str(" ORDER BY rank LIMIT :lim");

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<(&str, &dyn rusqlite::ToSql)> =
        params.iter().map(|(k, v)| (*k, v.as_ref())).collect();
    let results = stmt
        .query_map(param_refs.as_slice(), |row| {
            Ok(SearchResult {
                chunk_id: row.get(0)?,
                video_title: row.get(1)?,
                channel_name: row.get(2)?,
                snippet: row.get(3)?,
                start_sec: row.get(4)?,
                end_sec: row.get(5)?,
                score: row.get::<_, f64>(6)?.abs(),
                video_path: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(results)
}

pub fn get_top_words(conn: &Connection, limit: usize) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT word, SUM(count) as total FROM word_frequency GROUP BY word ORDER BY total DESC LIMIT ?1",
    )?;
    let results = stmt
        .query_map([limit], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(results)
}

/// Get analysis data for a video that owns the given chunk.
pub fn get_analysis_for_chunk(
    conn: &Connection,
    chunk_id: i64,
) -> Result<Option<super::types::AnalysisData>> {
    let result = conn.query_row(
        "SELECT va.summary, va.topics, va.sentiment
         FROM video_analysis va
         JOIN chunks c ON c.video_id = va.video_id
         WHERE c.id = ?1",
        [chunk_id],
        |row| {
            let summary: String = row.get(0)?;
            let topics_json: String = row.get(1)?;
            let sentiment: String = row.get(2)?;
            Ok((summary, topics_json, sentiment))
        },
    );
    match result {
        Ok((summary, topics_json, sentiment)) => {
            let topics: Vec<String> = serde_json::from_str(&topics_json).unwrap_or_default();
            Ok(Some(super::types::AnalysisData {
                summary,
                topics,
                sentiment,
            }))
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Get speaker label for a chunk's time range.
pub fn get_speaker_for_chunk(conn: &Connection, chunk_id: i64) -> Result<Option<String>> {
    let result = conn.query_row(
        "SELECT s.speaker
         FROM segments s
         JOIN chunks c ON c.video_id = s.video_id
         WHERE c.id = ?1 AND s.start_sec <= c.start_sec AND s.end_sec >= c.start_sec AND s.speaker IS NOT NULL
         ORDER BY s.start_sec
         LIMIT 1",
        [chunk_id],
        |row| row.get(0),
    );
    match result {
        Ok(speaker) => Ok(Some(speaker)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn get_video_id_by_recording(conn: &Connection, recording_id: &str) -> Result<Option<i64>> {
    let result = conn.query_row(
        "SELECT id FROM videos WHERE recording_id = ?1",
        [recording_id],
        |row| row.get(0),
    );
    match result {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn get_segments_for_video(
    conn: &Connection,
    video_id: i64,
) -> Result<Vec<(usize, f64, f64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT segment_index, start_sec, end_sec, text FROM segments WHERE video_id = ?1 ORDER BY segment_index",
    )?;
    let results = stmt
        .query_map([video_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(results)
}

/// Look up `(video_id, recording_id)` for a recording stored at the given
/// `.mkv` path. Returns None when there's no row (e.g. recording hasn't gone
/// through CrunchR yet).
pub fn lookup_video_by_path(conn: &Connection, video_path: &str) -> Result<Option<(i64, String)>> {
    let r = conn.query_row(
        "SELECT id, recording_id FROM videos WHERE video_path = ?1",
        [video_path],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
    );
    match r {
        Ok(v) => Ok(Some(v)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// One row per distinct speaker label on a given video, with line counts and
/// total speaking time. Used by the Speaker Editor modal.
#[derive(Debug, Clone)]
pub struct SpeakerStat {
    pub speaker: String,
    pub segment_count: i64,
    pub total_secs: f64,
}

pub fn load_speakers(conn: &Connection, video_id: i64) -> Result<Vec<SpeakerStat>> {
    let mut stmt = conn.prepare(
        "SELECT speaker, COUNT(*), COALESCE(SUM(end_sec - start_sec), 0.0)
         FROM segments
         WHERE video_id = ?1 AND speaker IS NOT NULL AND speaker != ''
         GROUP BY speaker
         ORDER BY SUM(end_sec - start_sec) DESC",
    )?;
    let rows = stmt
        .query_map([video_id], |row| {
            Ok(SpeakerStat {
                speaker: row.get(0)?,
                segment_count: row.get(1)?,
                total_secs: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Rename one speaker label across all segments of a video. Returns the
/// number of rows updated.
pub fn rewrite_speaker_label(
    conn: &Connection,
    video_id: i64,
    old_label: &str,
    new_label: &str,
) -> Result<usize> {
    let n = conn.execute(
        "UPDATE segments SET speaker = ?1 WHERE video_id = ?2 AND speaker = ?3",
        rusqlite::params![new_label, video_id, old_label],
    )?;
    Ok(n)
}

/// Load every segment for a video, with speaker + confidence, in transcript
/// order. Returned shape mirrors [`types::Segment`] so callers can rebuild
/// sidecar files after a label rewrite without re-running the pipeline.
pub fn load_full_segments(conn: &Connection, video_id: i64) -> Result<Vec<super::types::Segment>> {
    let mut stmt = conn.prepare(
        "SELECT segment_index, start_sec, end_sec, text, speaker, confidence
         FROM segments
         WHERE video_id = ?1
         ORDER BY segment_index",
    )?;
    let rows = stmt
        .query_map([video_id], |row| {
            Ok(super::types::Segment {
                index: row.get::<_, i64>(0)? as usize,
                start_sec: row.get(1)?,
                end_sec: row.get(2)?,
                text: row.get(3)?,
                speaker: row.get(4)?,
                confidence: row.get(5)?,
                words: None, // hydrate via segment_word_timings() on demand
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// One row in the web/TUI "transcribed recordings" list. Read-only summary
/// joining segment + analysis counts so a list view needs a single query.
#[derive(Debug, Clone)]
pub struct VideoSummary {
    pub recording_id: String,
    pub channel_name: String,
    pub title: String,
    pub status: String,
    pub segment_count: i64,
    pub has_analysis: bool,
    pub created_at: String,
}

/// Every video Crunchr has touched, newest first. Used by the webui's
/// Crunchr page to list transcribed recordings.
pub fn list_videos(conn: &Connection) -> Result<Vec<VideoSummary>> {
    let mut stmt = conn.prepare(
        "SELECT v.recording_id, v.channel_name, v.title, v.status, \
                (SELECT COUNT(*) FROM segments s WHERE s.video_id = v.id) AS segs, \
                (SELECT COUNT(*) FROM video_analysis a WHERE a.video_id = v.id) AS has_an, \
                COALESCE(v.created_at, '') \
         FROM videos v ORDER BY v.created_at DESC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(VideoSummary {
                recording_id: row.get(0)?,
                channel_name: row.get(1)?,
                title: row.get(2)?,
                status: row.get(3)?,
                segment_count: row.get(4)?,
                has_analysis: row.get::<_, i64>(5)? > 0,
                created_at: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Full transcript + analysis for one recording, in a single read. `segments`
/// is empty when the recording hasn't been transcribed yet; analysis fields
/// are None until the LLM pass completes. Returns None when Crunchr has never
/// seen the recording.
#[derive(Debug, Clone)]
pub struct RecordingDetail {
    pub recording_id: String,
    pub channel_name: String,
    pub title: String,
    pub status: String,
    pub segments: Vec<super::types::Segment>,
    pub summary: Option<String>,
    pub topics: Vec<String>,
    pub sentiment: Option<String>,
}

pub fn recording_detail(conn: &Connection, recording_id: &str) -> Result<Option<RecordingDetail>> {
    let head = conn.query_row(
        "SELECT id, channel_name, title, status FROM videos WHERE recording_id = ?1",
        [recording_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        },
    );
    let (video_id, channel_name, title, status) = match head {
        Ok(v) => v,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(e) => return Err(e.into()),
    };

    let segments = load_full_segments(conn, video_id)?;

    let analysis: Option<(Option<String>, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT summary, topics, sentiment FROM video_analysis WHERE video_id = ?1",
            [video_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .ok();

    let (summary, topics, sentiment) = match analysis {
        Some((s, t, sent)) => (s, parse_topics_field(t.as_deref()), sent),
        None => (None, Vec::new(), None),
    };

    Ok(Some(RecordingDetail {
        recording_id: recording_id.to_string(),
        channel_name,
        title,
        status,
        segments,
        summary,
        topics,
        sentiment,
    }))
}

/// `video_analysis.topics` is either a JSON array of strings or (from older
/// analysis runs) a comma-separated string. Normalize both to a Vec.
fn parse_topics_field(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw else { return Vec::new() };
    let raw = raw.trim();
    if raw.is_empty() {
        return Vec::new();
    }
    if let Ok(serde_json::Value::Array(arr)) = serde_json::from_str::<serde_json::Value>(raw) {
        return arr
            .into_iter()
            .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect();
    }
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn
    }

    fn seed_video(conn: &Connection, recording_id: &str) -> i64 {
        insert_video(conn, recording_id, "Channel", "Title", "/tmp/video.mkv").unwrap()
    }

    #[test]
    fn rewrite_speaker_label_updates_only_matching_rows() {
        let conn = fresh_db();
        let video_id = seed_video(&conn, "rec-1");

        // Three segments: two by "Speaker 0", one by "Speaker 1".
        let segs: Vec<(usize, f64, f64, &str, Option<&str>, Option<f64>)> = vec![
            (0, 0.0, 1.0, "hi", Some("Speaker 0"), None),
            (1, 1.0, 2.0, "yo", Some("Speaker 1"), None),
            (2, 2.0, 3.0, "back", Some("Speaker 0"), None),
        ];
        insert_segments(&conn, video_id, &segs).unwrap();

        // Sanity: load_speakers groups + counts correctly.
        let speakers = load_speakers(&conn, video_id).unwrap();
        assert_eq!(speakers.len(), 2);
        let s0 = speakers.iter().find(|s| s.speaker == "Speaker 0").unwrap();
        assert_eq!(s0.segment_count, 2);

        // Rename Speaker 0 -> Alice; only those two rows change.
        let changed = rewrite_speaker_label(&conn, video_id, "Speaker 0", "Alice").unwrap();
        assert_eq!(changed, 2);

        let after = load_speakers(&conn, video_id).unwrap();
        assert!(after.iter().any(|s| s.speaker == "Alice"));
        assert!(after.iter().any(|s| s.speaker == "Speaker 1"));
        assert!(!after.iter().any(|s| s.speaker == "Speaker 0"));
    }

    #[test]
    fn lookup_video_by_path_roundtrip() {
        let conn = fresh_db();
        let id = seed_video(&conn, "rec-2");
        let got = lookup_video_by_path(&conn, "/tmp/video.mkv").unwrap();
        assert_eq!(got, Some((id, "rec-2".to_string())));
        assert_eq!(lookup_video_by_path(&conn, "/nope").unwrap(), None);
    }

    #[test]
    fn load_full_segments_preserves_speakers_and_order() {
        let conn = fresh_db();
        let id = seed_video(&conn, "rec-3");
        let segs: Vec<(usize, f64, f64, &str, Option<&str>, Option<f64>)> = vec![
            (0, 0.0, 1.0, "one", Some("A"), Some(-0.1)),
            (1, 1.0, 2.0, "two", None, None),
            (2, 2.0, 3.0, "three", Some("B"), None),
        ];
        insert_segments(&conn, id, &segs).unwrap();
        let loaded = load_full_segments(&conn, id).unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].speaker.as_deref(), Some("A"));
        assert_eq!(loaded[1].speaker, None);
        assert_eq!(loaded[2].speaker.as_deref(), Some("B"));
    }

    #[test]
    fn list_videos_reports_counts_and_analysis_flag() {
        let conn = fresh_db();
        let v1 = insert_video(&conn, "rec-a", "ChanA", "First", "/tmp/a.mkv").unwrap();
        insert_video(&conn, "rec-b", "ChanB", "Second", "/tmp/b.mkv").unwrap();
        let segs: Vec<(usize, f64, f64, &str, Option<&str>, Option<f64>)> = vec![
            (0, 0.0, 1.0, "hi", None, None),
            (1, 1.0, 2.0, "yo", None, None),
        ];
        insert_segments(&conn, v1, &segs).unwrap();
        conn.execute(
            "INSERT INTO video_analysis (video_id, summary, topics, sentiment) VALUES (?1, 'sum', '[\"x\"]', 'positive')",
            [v1],
        )
        .unwrap();

        let vids = list_videos(&conn).unwrap();
        assert_eq!(vids.len(), 2);
        let a = vids.iter().find(|v| v.recording_id == "rec-a").unwrap();
        assert_eq!(a.segment_count, 2);
        assert!(a.has_analysis);
        let b = vids.iter().find(|v| v.recording_id == "rec-b").unwrap();
        assert_eq!(b.segment_count, 0);
        assert!(!b.has_analysis);
    }

    #[test]
    fn recording_detail_composes_transcript_and_analysis() {
        let conn = fresh_db();
        let id = insert_video(&conn, "rec-d", "Chan", "Talk", "/tmp/d.mkv").unwrap();
        let segs: Vec<(usize, f64, f64, &str, Option<&str>, Option<f64>)> =
            vec![(0, 0.0, 2.0, "hello world", Some("Alice"), None)];
        insert_segments(&conn, id, &segs).unwrap();
        conn.execute(
            "INSERT INTO video_analysis (video_id, summary, topics, sentiment) VALUES (?1, 'A chat', '[\"news\",\"sports\"]', 'neutral')",
            [id],
        )
        .unwrap();

        let d = recording_detail(&conn, "rec-d").unwrap().unwrap();
        assert_eq!(d.title, "Talk");
        assert_eq!(d.segments.len(), 1);
        assert_eq!(d.segments[0].speaker.as_deref(), Some("Alice"));
        assert_eq!(d.summary.as_deref(), Some("A chat"));
        assert_eq!(d.topics, vec!["news".to_string(), "sports".to_string()]);
        assert_eq!(d.sentiment.as_deref(), Some("neutral"));

        assert!(recording_detail(&conn, "missing").unwrap().is_none());
    }

    #[test]
    fn parse_topics_field_handles_json_and_csv() {
        assert_eq!(parse_topics_field(Some("[\"a\", \"b\"]")), vec!["a", "b"]);
        assert_eq!(parse_topics_field(Some("a, b ,c")), vec!["a", "b", "c"]);
        assert!(parse_topics_field(Some("")).is_empty());
        assert!(parse_topics_field(None).is_empty());
    }
}
