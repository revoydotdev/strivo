//! SQLite persistence for viewguard.
//!
//! Schema lives at `<data_dir>/viewguard.db`, where `data_dir` is already
//! scoped to `<base>/plugins/viewguard` by the plugin registry. Three
//! tables: `samples` (the time series), `signals` (per-detector firings),
//! `verdicts` (the audit trail — final score per stream session).

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::path::Path;

pub struct ViewguardStore {
    conn: Connection,
}

#[derive(Debug, Clone)]
pub struct VerdictRow {
    pub channel_id: String,
    pub stream_started_at: DateTime<Utc>,
    pub stream_ended_at: Option<DateTime<Utc>>,
    pub final_score: f32,
    pub band: String,
    pub contributors_json: String,
}

impl ViewguardStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS samples (
                channel_id TEXT NOT NULL,
                platform   TEXT NOT NULL,
                ts         TEXT NOT NULL,
                viewers    INTEGER,
                PRIMARY KEY (channel_id, ts)
             );
             CREATE INDEX IF NOT EXISTS samples_recent
                ON samples(channel_id, ts DESC);

             CREATE TABLE IF NOT EXISTS signals (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                channel_id  TEXT NOT NULL,
                detector    TEXT NOT NULL,
                score       REAL NOT NULL,
                confidence  REAL NOT NULL,
                ts          TEXT NOT NULL,
                evidence    TEXT
             );
             CREATE INDEX IF NOT EXISTS signals_channel_ts
                ON signals(channel_id, ts DESC);

             CREATE TABLE IF NOT EXISTS verdicts (
                channel_id        TEXT NOT NULL,
                stream_started_at TEXT NOT NULL,
                stream_ended_at   TEXT,
                final_score       REAL NOT NULL,
                band              TEXT NOT NULL,
                contributors      TEXT NOT NULL,
                PRIMARY KEY (channel_id, stream_started_at)
             );",
        )?;
        Ok(Self { conn })
    }

    pub fn insert_sample(
        &self,
        channel_id: &str,
        platform: &str,
        ts: DateTime<Utc>,
        viewers: u32,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO samples(channel_id, platform, ts, viewers)
             VALUES (?1, ?2, ?3, ?4)",
            params![channel_id, platform, ts.to_rfc3339(), viewers as i64],
        )?;
        Ok(())
    }

    pub fn insert_signal(
        &self,
        channel_id: &str,
        detector: &str,
        score: f32,
        confidence: f32,
        ts: DateTime<Utc>,
        evidence_json: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO signals(channel_id, detector, score, confidence, ts, evidence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                channel_id,
                detector,
                score as f64,
                confidence as f64,
                ts.to_rfc3339(),
                evidence_json
            ],
        )?;
        Ok(())
    }

    pub fn upsert_verdict(&self, v: &VerdictRow) -> Result<()> {
        self.conn.execute(
            "INSERT INTO verdicts(channel_id, stream_started_at, stream_ended_at, final_score, band, contributors)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(channel_id, stream_started_at) DO UPDATE SET
               stream_ended_at = excluded.stream_ended_at,
               final_score     = excluded.final_score,
               band            = excluded.band,
               contributors    = excluded.contributors",
            params![
                v.channel_id,
                v.stream_started_at.to_rfc3339(),
                v.stream_ended_at.as_ref().map(|t| t.to_rfc3339()),
                v.final_score as f64,
                v.band,
                v.contributors_json,
            ],
        )?;
        Ok(())
    }

    /// Most recent verdict for a channel — used by the recording
    /// properties pane.
    pub fn latest_verdict(&self, channel_id: &str) -> Result<Option<VerdictRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT channel_id, stream_started_at, stream_ended_at, final_score, band, contributors
             FROM verdicts
             WHERE channel_id = ?1
             ORDER BY stream_started_at DESC
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![channel_id])?;
        if let Some(row) = rows.next()? {
            let started: String = row.get(1)?;
            let ended: Option<String> = row.get(2)?;
            Ok(Some(VerdictRow {
                channel_id: row.get(0)?,
                stream_started_at: DateTime::parse_from_rfc3339(&started)?.with_timezone(&Utc),
                stream_ended_at: ended
                    .map(|s| DateTime::parse_from_rfc3339(&s).map(|t| t.with_timezone(&Utc)))
                    .transpose()?,
                final_score: row.get::<_, f64>(3)? as f32,
                band: row.get(4)?,
                contributors_json: row.get(5)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Trim samples older than `days`. Verdicts kept forever.
    pub fn sweep_old_samples(&self, days: i64) -> Result<usize> {
        let cutoff = Utc::now() - chrono::Duration::days(days);
        let n = self.conn.execute(
            "DELETE FROM samples WHERE ts < ?1",
            params![cutoff.to_rfc3339()],
        )?;
        Ok(n)
    }
}

/// One point in a channel's viewer time series.
#[derive(Debug, Clone)]
pub struct SamplePoint {
    pub ts: String,
    pub viewers: i64,
}

/// Read a [`VerdictRow`] from a result row laid out as
/// `(channel_id, started, ended, score, band, contributors)`.
fn verdict_from_row(row: &rusqlite::Row) -> Result<VerdictRow> {
    let started: String = row.get(1)?;
    let ended: Option<String> = row.get(2)?;
    Ok(VerdictRow {
        channel_id: row.get(0)?,
        stream_started_at: DateTime::parse_from_rfc3339(&started)?.with_timezone(&Utc),
        stream_ended_at: ended
            .map(|s| DateTime::parse_from_rfc3339(&s).map(|t| t.with_timezone(&Utc)))
            .transpose()?,
        final_score: row.get::<_, f64>(3)? as f32,
        band: row.get(4)?,
        contributors_json: row.get(5)?,
    })
}

/// Latest verdict per channel across the whole store, newest first. Read-only
/// — takes a borrowed [`Connection`] so the webui can open the DB read-only.
pub fn all_verdicts(conn: &Connection) -> Result<Vec<VerdictRow>> {
    let mut stmt = conn.prepare(
        "SELECT v.channel_id, v.stream_started_at, v.stream_ended_at, \
                v.final_score, v.band, v.contributors \
         FROM verdicts v \
         JOIN ( \
            SELECT channel_id, MAX(stream_started_at) AS latest \
            FROM verdicts GROUP BY channel_id \
         ) m ON m.channel_id = v.channel_id AND m.latest = v.stream_started_at \
         ORDER BY v.final_score DESC, v.stream_started_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        verdict_from_row(row).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, e.into())
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// The most recent `limit` viewer samples for a channel, oldest-first so the
/// caller can plot a left-to-right sparkline.
pub fn samples_for(conn: &Connection, channel_id: &str, limit: usize) -> Result<Vec<SamplePoint>> {
    let mut stmt = conn.prepare(
        "SELECT ts, COALESCE(viewers, 0) FROM samples \
         WHERE channel_id = ?1 ORDER BY ts DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![channel_id, limit as i64], |row| {
        Ok(SamplePoint {
            ts: row.get(0)?,
            viewers: row.get(1)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    out.reverse();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh() -> (TempDir, ViewguardStore) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("viewguard.db");
        let store = ViewguardStore::open(&path).unwrap();
        (dir, store)
    }

    #[test]
    fn migrations_idempotent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("v.db");
        let _ = ViewguardStore::open(&path).unwrap();
        // Reopen — should not fail.
        let _ = ViewguardStore::open(&path).unwrap();
    }

    #[test]
    fn sample_dedupe_via_pkey() {
        let (_d, s) = fresh();
        let t = Utc::now();
        s.insert_sample("c1", "twitch", t, 100).unwrap();
        s.insert_sample("c1", "twitch", t, 150).unwrap(); // REPLACE
        let count: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM samples WHERE channel_id='c1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn verdict_upsert_roundtrip() {
        let (_d, s) = fresh();
        let started = Utc::now();
        let v = VerdictRow {
            channel_id: "c1".into(),
            stream_started_at: started,
            stream_ended_at: None,
            final_score: 0.6,
            band: "suspect".into(),
            contributors_json: "[]".into(),
        };
        s.upsert_verdict(&v).unwrap();
        let v2 = VerdictRow {
            stream_ended_at: Some(started + chrono::Duration::hours(2)),
            final_score: 0.7,
            band: "suspect".into(),
            ..v.clone()
        };
        s.upsert_verdict(&v2).unwrap();
        let got = s.latest_verdict("c1").unwrap().unwrap();
        assert_eq!(got.band, "suspect");
        assert!((got.final_score - 0.7).abs() < 1e-5);
        assert!(got.stream_ended_at.is_some());
    }

    #[test]
    fn retention_sweep_drops_old() {
        let (_d, s) = fresh();
        let old = Utc::now() - chrono::Duration::days(45);
        let recent = Utc::now() - chrono::Duration::days(2);
        s.insert_sample("c1", "twitch", old, 1).unwrap();
        s.insert_sample("c1", "twitch", recent, 2).unwrap();
        let dropped = s.sweep_old_samples(30).unwrap();
        assert_eq!(dropped, 1);
    }

    #[test]
    fn all_verdicts_returns_latest_per_channel_sorted_by_score() {
        let (_d, s) = fresh();
        let t0 = Utc::now() - chrono::Duration::hours(3);
        let t1 = Utc::now();
        // c1 has two sessions; only the latest should surface.
        s.upsert_verdict(&VerdictRow {
            channel_id: "c1".into(),
            stream_started_at: t0,
            stream_ended_at: None,
            final_score: 0.2,
            band: "clean".into(),
            contributors_json: "[]".into(),
        })
        .unwrap();
        s.upsert_verdict(&VerdictRow {
            channel_id: "c1".into(),
            stream_started_at: t1,
            stream_ended_at: None,
            final_score: 0.9,
            band: "fraudulent".into(),
            contributors_json: "[]".into(),
        })
        .unwrap();
        s.upsert_verdict(&VerdictRow {
            channel_id: "c2".into(),
            stream_started_at: t1,
            stream_ended_at: None,
            final_score: 0.5,
            band: "suspect".into(),
            contributors_json: "[]".into(),
        })
        .unwrap();

        let v = all_verdicts(&s.conn).unwrap();
        assert_eq!(v.len(), 2);
        // Highest score first.
        assert_eq!(v[0].channel_id, "c1");
        assert_eq!(v[0].band, "fraudulent");
        assert_eq!(v[1].channel_id, "c2");
    }

    #[test]
    fn samples_for_returns_oldest_first_capped() {
        let (_d, s) = fresh();
        let base = Utc::now() - chrono::Duration::minutes(10);
        for i in 0..5 {
            s.insert_sample(
                "c1",
                "twitch",
                base + chrono::Duration::minutes(i),
                (i * 10) as u32,
            )
            .unwrap();
        }
        let pts = samples_for(&s.conn, "c1", 3).unwrap();
        assert_eq!(pts.len(), 3);
        // Capped to the 3 most-recent, returned oldest-first for plotting.
        assert!(pts[0].viewers < pts[2].viewers);
        assert_eq!(pts[2].viewers, 40);
    }
}
