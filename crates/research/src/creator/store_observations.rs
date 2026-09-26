//! Persistence for the observation series, and the coverage figure derived
//! from them.
//!
//! Three writes and four reads, all through the one `Connection`
//! [`Store`](super::store::Store) already owns. Two properties are the
//! reason the writes are shaped the way they are:
//!
//! * **A batch is one transaction.** A reader sees either the whole batch or
//!   none of it, so an append interrupted part-way leaves a consistent
//!   series rather than a hole that looks like a dropped poll. The sequence
//!   numbers are assigned *by the store* from `MAX(seq) + 1`, which is what
//!   makes a resumed capture unable to reuse one it never saw committed —
//!   the caller does not get to choose.
//! * **Coverage is computed, not stored.** A cached coverage figure is a
//!   second answer to "how much did we observe" and the first writer to
//!   append a sample without updating it is undetectable. The union is
//!   derived on read from the rows that are actually there.

use rusqlite::{params, Connection};
use strivo_editor::time::ClockMap;

use super::ids::{CaptureSessionId, ProjectId};
use super::observation_error::ObservationError;
use super::observations::{CaptureWindow, MAX_BATCH};
use super::store::Store;
use super::stream_event::{EventRow, RawStreamEvent, StreamEvent};
use super::viewer_sample::{RawViewerSample, SampleRow, ViewerSample};

/// The store's own result type: every observation refusal stays typed all
/// the way out to the caller, so "unavailable", "outside the clock map" and
/// "the database said no" are distinguishable without reading a message.
type Result<T> = std::result::Result<T, ObservationError>;

/// The Creator tables this model owns, as a closed set.
///
/// A name never reaches SQL from a caller: a table-count helper that took a
/// `&str` would be a way to run arbitrary SQL against a Creator install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountedTable {
    ViewerSamples,
    StreamEvents,
    MetricAvailability,
    MetricObservations,
}

impl CountedTable {
    const fn sql(self) -> &'static str {
        match self {
            Self::ViewerSamples => "SELECT COUNT(*) FROM creator_viewer_samples",
            Self::StreamEvents => "SELECT COUNT(*) FROM creator_stream_events",
            Self::MetricAvailability => "SELECT COUNT(*) FROM creator_metric_availability",
            Self::MetricObservations => "SELECT COUNT(*) FROM creator_metric_observations",
        }
    }
}

/// Run `body` inside one `BEGIN IMMEDIATE`, committing on success and
/// rolling back on any error.
///
/// Shared by both appends so a batch is atomic by construction rather than by
/// three copies of the same commit/rollback pair staying in step. An
/// interrupted append therefore leaves the series untouched, and the next
/// append takes the next sequence number — which is what makes a resumed
/// capture unable to reuse one it never saw committed.
fn in_transaction(conn: &Connection, body: impl FnOnce() -> Result<()>) -> Result<()> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    match body() {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// Refuse a batch whose readings are not in capture order.
///
/// Checked before a single row is written and before any sequence number is
/// assigned, so a reversed batch costs nothing and leaves the series
/// untouched. Non-decreasing rather than strictly increasing: two readings
/// may legitimately share a wall millisecond.
fn require_capture_order(walls: impl Iterator<Item = i64>) -> Result<()> {
    let mut previous: Option<i64> = None;
    for (i, wall_ms) in walls.enumerate() {
        if let Some(prev) = previous {
            if wall_ms < prev {
                return Err(ObservationError::OutOfOrderBatch {
                    at_seq: u32::try_from(i).unwrap_or(u32::MAX),
                    previous_wall_ms: prev,
                    wall_ms,
                });
            }
        }
        previous = Some(wall_ms);
    }
    Ok(())
}

/// The next sequence number for a session's series, or 0 for a new one.
fn next_seq(conn: &Connection, table: &str, session: &CaptureSessionId) -> Result<u32> {
    let sql = format!("SELECT COALESCE(MAX(seq), -1) + 1 FROM {table} WHERE capture_session_id=?1");
    let next: i64 = conn.query_row(&sql, [session.to_string()], |r| r.get(0))?;
    Ok(u32::try_from(next).unwrap_or(0))
}

impl Store {
    /// The raw connection, for the migration and rollback rehearsals.
    #[must_use]
    pub fn raw_connection(&self) -> &Connection {
        &self.conn
    }

    /// How many rows a Creator table holds.
    pub fn row_count(&self, table: CountedTable) -> Result<i64> {
        Ok(self.conn.query_row(table.sql(), [], |r| r.get(0))?)
    }

    /// Append a batch of provider polls as viewer samples.
    ///
    /// The whole batch is one transaction: any refusal inside it — a poll
    /// inside a reconnect hole, a negative count, a timestamp outside the
    /// window — leaves the series untouched rather than half-written.
    pub fn append_viewer_samples(
        &self,
        session: CaptureSessionId,
        project: ProjectId,
        batch: &[RawViewerSample],
        window: &CaptureWindow,
        clock: &ClockMap,
    ) -> Result<Vec<ViewerSample>> {
        if batch.len() > MAX_BATCH {
            return Err(ObservationError::BatchTooLarge {
                len: batch.len(),
                limit: MAX_BATCH,
            });
        }
        // Every sample is derived *before* the transaction opens, so a refusal
        // costs no lock and a long batch cannot hold the write lock while it
        // is being validated.
        require_capture_order(batch.iter().map(|r| r.wall_ms))?;
        let mut seq = next_seq(&self.conn, "creator_viewer_samples", &session)?;
        let mut placed = Vec::with_capacity(batch.len());
        for raw in batch {
            let mut sample = ViewerSample::observe(window, clock, raw.clone())?;
            sample.capture_session_id = session;
            sample.project_id = project;
            sample.seq = seq;
            seq = seq.saturating_add(1);
            placed.push(sample);
        }

        in_transaction(&self.conn, || {
            for s in &placed {
                self.conn.execute(
                    "INSERT INTO creator_viewer_samples
                     (id,capture_session_id,project_id,seq,wall_ms,media_us,uncertainty_us,
                      viewer_count,poll_interval_ms,dropped)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![
                        uuid::Uuid::new_v4().to_string(),
                        session.to_string(),
                        project.to_string(),
                        i64::from(s.seq),
                        s.wall_ms,
                        s.position.media.micros(),
                        s.position.uncertainty_micros,
                        i64::from(s.value),
                        s.poll_interval_ms,
                        i64::from(s.dropped),
                    ],
                )?;
            }
            Ok(())
        })?;
        Ok(placed)
    }

    /// The session's viewer samples, in capture order.
    ///
    /// A row this model cannot make sense of is a refusal, not a repaired
    /// default: a NULL wall clock read as 0 would place a truncated sample
    /// on the first frame, and a negative count read as a `u32` would become
    /// four billion viewers. Both are reachable only by a hand-edited or
    /// corrupt database, and a corrupt row is a finding.
    pub fn viewer_samples(&self, session: CaptureSessionId) -> Result<Vec<ViewerSample>> {
        // Read the raw columns first, then convert. A rusqlite row closure
        // may only return a `rusqlite::Error`, so validating inside it would
        // either flatten a typed refusal into an opaque conversion failure or
        // smuggle the value past. Two steps keeps the refusal typed.
        let mut stmt = self.conn.prepare(
            "SELECT seq,wall_ms,media_us,uncertainty_us,viewer_count,poll_interval_ms,dropped
             FROM creator_viewer_samples WHERE capture_session_id=?1 ORDER BY seq",
        )?;
        let raw: Vec<SampleRow> = stmt
            .query_map([session.to_string()], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })?
            .collect::<std::result::Result<_, rusqlite::Error>>()?;
        raw.into_iter()
            .map(|r| ViewerSample::from_row(session, r))
            .collect()
    }

    /// Append a batch of markers, categories and provider events.
    pub fn append_stream_events(
        &self,
        session: CaptureSessionId,
        project: ProjectId,
        batch: &[RawStreamEvent],
        window: &CaptureWindow,
        clock: &ClockMap,
    ) -> Result<Vec<StreamEvent>> {
        if batch.len() > MAX_BATCH {
            return Err(ObservationError::BatchTooLarge {
                len: batch.len(),
                limit: MAX_BATCH,
            });
        }
        require_capture_order(batch.iter().map(|r| r.wall_ms))?;
        let mut seq = next_seq(&self.conn, "creator_stream_events", &session)?;
        let mut placed = Vec::with_capacity(batch.len());
        for raw in batch {
            let mut event = StreamEvent::observe(window, clock, raw.clone())?;
            event.capture_session_id = session;
            event.project_id = project;
            event.seq = seq;
            seq = seq.saturating_add(1);
            placed.push(event);
        }

        in_transaction(&self.conn, || {
            for e in &placed {
                self.conn.execute(
                    "INSERT INTO creator_stream_events
                     (id,capture_session_id,project_id,seq,kind,wall_ms,media_us,
                      uncertainty_us,label,category,payload_json)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                    params![
                        uuid::Uuid::new_v4().to_string(),
                        session.to_string(),
                        project.to_string(),
                        i64::from(e.seq),
                        e.kind.as_str(),
                        e.wall_ms,
                        e.position.media.micros(),
                        e.position.uncertainty_micros,
                        e.label,
                        e.category,
                        e.payload_json,
                    ],
                )?;
            }
            Ok(())
        })?;
        Ok(placed)
    }

    /// The session's events, in capture order.
    pub fn stream_events(&self, session: CaptureSessionId) -> Result<Vec<StreamEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq,kind,wall_ms,media_us,uncertainty_us,label,category,payload_json
             FROM creator_stream_events WHERE capture_session_id=?1 ORDER BY seq",
        )?;
        let raw: Vec<EventRow> = stmt
            .query_map([session.to_string()], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            })?
            .collect::<std::result::Result<_, rusqlite::Error>>()?;
        raw.into_iter()
            .map(|r| StreamEvent::from_row(session, r))
            .collect()
    }
}
