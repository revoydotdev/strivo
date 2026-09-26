//! Persistence for the durable chat record, and the coverage figure derived
//! from the acquisition intervals.
//!
//! Six writes and four reads, all through the one `Connection`
//! [`Store`](super::store::Store) already owns. Three properties shape them:
//!
//! * **Deduplication is the schema's, not this file's.** `INSERT .. ON CONFLICT
//!   DO NOTHING` against `UNIQUE(capture_session_id, identity)` is what makes a
//!   redelivered message a duplicate, so a caller cannot reach past it with a
//!   hand-made identity and a re-import of the same log cannot double the
//!   corpus. The `changes()` result *is* the stored/already-present split — the
//!   two numbers are read from the database rather than counted here, so they
//!   cannot disagree with what was written.
//! * **A batch is one transaction, and a refusal writes nothing.** A message
//!   the clock cannot place abandons the whole batch, exactly as a viewer
//!   sample in a reconnect hole does, so the series is never half-written.
//! * **The acquisition interval is the record of a dead process.** A collection
//!   row with no end *is* the witness: [`Store::reap_open_collections`] closes
//!   it and reports the interval nobody was watching. There is no heartbeat, no
//!   counter and no timeout to tune, because the absence of an end is already
//!   the fact.

use rusqlite::{params, Connection};
use strivo_editor::time::ClockMap;

use super::chat::{
    media_of, ChatAppend, ChatCollection, ChatDeletion, ChatDeletionTarget, ChatIdentity,
    ChatMessage, ChatReplay, CollectionEnd, DeletionOutcome, Landmark, RawChatDeletion,
    RawChatMessage, ReplayedChat, MAX_CHAT_BATCH,
};
use super::coverage::{Coverage, CoverageGap, GapReason};
use super::ids::{CaptureSessionId, ChatCollectionId, ProjectId};
use super::observation_error::ObservationError;
use super::observations::{CaptureWindow, SamplePosition};
use super::store::Store;

/// The store's own result type: every chat refusal stays typed all the way out
/// to the caller, so "outside the clock map", "body too long" and "the
/// database said no" are distinguishable without reading a message.
type Result<T> = std::result::Result<T, ObservationError>;

const TABLE_COLLECTIONS: &str = "creator_chat_collections";
const TABLE_MESSAGES: &str = "creator_chat_messages";
const TABLE_DELETIONS: &str = "creator_chat_deletions";
const TABLE_GAPS: &str = "creator_chat_gaps";

/// Run `body` inside one `BEGIN IMMEDIATE`, committing on success and rolling
/// back on any error. The same helper the observation appends use, so a batch
/// is atomic by construction rather than by two copies of a commit/rollback
/// pair staying in step.
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

/// The next sequence number within a collection, or 0 for a new one. Assigned
/// by the store from `MAX(seq) + 1` so a resumed collection continues rather
/// than colliding with a batch it never saw committed.
fn next_seq(conn: &Connection, collection: &ChatCollectionId) -> Result<u32> {
    let sql =
        format!("SELECT COALESCE(MAX(seq), -1) + 1 FROM {TABLE_MESSAGES} WHERE collection_id=?1");
    let next: i64 = conn.query_row(&sql, [collection.to_string()], |r| r.get(0))?;
    Ok(u32::try_from(next).unwrap_or(0))
}

impl Store {
    /// Open a collection for a room: the daemon, not a browser, deciding that
    /// from here on collection happens.
    pub fn open_chat_collection(
        &self,
        session: CaptureSessionId,
        project: ProjectId,
        platform: &str,
        room: &str,
        at_wall_ms: i64,
    ) -> Result<ChatCollection> {
        if platform.is_empty() || platform.len() > 32 {
            return Err(ObservationError::LabelTooLong {
                bytes: platform.len(),
                limit: 32,
            });
        }
        if room.is_empty() || room.len() > 256 {
            return Err(ObservationError::LabelTooLong {
                bytes: room.len(),
                limit: 256,
            });
        }
        let collection = ChatCollection {
            id: ChatCollectionId::generate(),
            project_id: project,
            capture_session_id: session,
            platform: platform.to_string(),
            room: room.to_string(),
            opened_at_wall_ms: at_wall_ms,
            ended_wall_ms: None,
            end_reason: None,
        };
        self.conn.execute(
            "INSERT INTO creator_chat_collections
             (id,project_id,capture_session_id,platform,room,opened_at_wall_ms,ended_wall_ms,end_reason)
             VALUES(?1,?2,?3,?4,?5,?6,NULL,NULL)",
            params![
                collection.id.to_string(),
                project.to_string(),
                session.to_string(),
                collection.platform,
                collection.room,
                collection.opened_at_wall_ms,
            ],
        )?;
        Ok(collection)
    }

    /// Close a collection this process opened.
    pub fn close_chat_collection(
        &self,
        collection: &ChatCollection,
        at_wall_ms: i64,
        reason: super::chat::CollectionEnd,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE creator_chat_collections SET ended_wall_ms=?1, end_reason=?2 WHERE id=?3",
            params![at_wall_ms, reason.as_str(), collection.id.to_string()],
        )?;
        Ok(())
    }

    /// Close every collection a *previous* process left open, and report the
    /// intervals that were therefore uncovered.
    ///
    /// This is the restart path and the only place a
    /// [`GapReason::DaemonDown`] can come from: a row with no end is the fact,
    /// so there is nothing to detect and nothing to tune. The new process's
    /// `now` becomes the end of the hole, which is the one imprecision here and
    /// the conservative direction — the gap is stated as lasting until the new
    /// process *noticed*, so it is never shorter than the truth.
    ///
    /// The caller must have already reaped before it appends, so a message from
    /// the new process cannot be attributed to the dead one's collection.
    pub fn reap_open_collections(
        &self,
        session: CaptureSessionId,
        now_wall_ms: i64,
    ) -> Result<Vec<CoverageGap>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, project_id, platform, room, opened_at_wall_ms
             FROM {TABLE_COLLECTIONS}
             WHERE ended_wall_ms IS NULL AND capture_session_id=?1 AND opened_at_wall_ms <= ?2
             ORDER BY opened_at_wall_ms",
        ))?;
        // Read the raw columns, then convert: a `rusqlite` row closure may only
        // return a `rusqlite::Error`, so validating a typed id inside it would
        // flatten a typed refusal into an opaque conversion failure.
        let raw = stmt
            .query_map(params![session.to_string(), now_wall_ms], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, rusqlite::Error>>()?;
        drop(stmt);
        let mut open: Vec<ChatCollection> = Vec::with_capacity(raw.len());
        for (id, project, platform, room, opened_at_wall_ms) in raw {
            open.push(ChatCollection {
                id: parse_id::<ChatCollectionId>(&id, TABLE_COLLECTIONS)?,
                project_id: parse_id::<ProjectId>(&project, TABLE_COLLECTIONS)?,
                capture_session_id: session,
                platform,
                room,
                opened_at_wall_ms,
                ended_wall_ms: None,
                end_reason: None,
            });
        }

        let mut gaps = Vec::new();
        for collection in open {
            // A collection opened at exactly `now` covers no interval, so
            // there is no hole to record — and leaving it open is correct,
            // because this process is the one collecting into it.
            let Ok(gap) = CoverageGap::new(
                collection.opened_at_wall_ms,
                now_wall_ms,
                GapReason::DaemonDown,
            ) else {
                continue;
            };
            in_transaction(&self.conn, || {
                self.conn.execute(
                    "UPDATE creator_chat_collections SET ended_wall_ms=?1, end_reason='daemon_down'
                     WHERE id=?2",
                    params![now_wall_ms, collection.id.to_string()],
                )?;
                insert_gap(
                    &self.conn,
                    collection.project_id,
                    collection.capture_session_id,
                    Some(&collection.id),
                    &gap,
                    "collection left open by a process that ended",
                )
            })?;
            gaps.push(gap);
        }
        Ok(gaps)
    }

    /// Every collection for a session, in the order they were opened.
    pub fn chat_collections(&self, session: CaptureSessionId) -> Result<Vec<ChatCollection>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, project_id, platform, room, opened_at_wall_ms, ended_wall_ms, end_reason
             FROM {TABLE_COLLECTIONS} WHERE capture_session_id=?1
             ORDER BY opened_at_wall_ms, id"
        ))?;
        let rows = stmt.query_map([session.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, project, platform, room, opened, ended, reason) =
                row.map_err(ObservationError::Database)?;
            out.push(ChatCollection {
                id: parse_id(&id, TABLE_COLLECTIONS)?,
                project_id: parse_id(&project, TABLE_COLLECTIONS)?,
                capture_session_id: session,
                platform,
                room,
                opened_at_wall_ms: opened,
                ended_wall_ms: ended,
                end_reason: reason.as_deref().and_then(CollectionEnd::parse),
            });
        }
        Ok(out)
    }

    /// Only the collections that are still open — the ones a reaper must close.
    pub fn open_chat_collections(&self, session: CaptureSessionId) -> Result<Vec<ChatCollection>> {
        Ok(self
            .chat_collections(session)?
            .into_iter()
            .filter(ChatCollection::is_open)
            .collect())
    }

    /// Append a batch of received messages to a collection.
    ///
    /// Every message is placed on the clock *before* the transaction opens, so
    /// a refusal costs no lock and nothing is half-written. The stored /
    /// already-present split is read back from SQLite's own `changes()`, not
    /// counted here, so it cannot claim a row the database refused.
    pub fn append_chat(
        &self,
        collection: &ChatCollection,
        batch: &[RawChatMessage],
        window: &CaptureWindow,
        clock: &ClockMap,
    ) -> Result<ChatAppend> {
        if batch.len() > MAX_CHAT_BATCH {
            return Err(ObservationError::BatchTooLarge {
                len: batch.len(),
                limit: MAX_CHAT_BATCH,
            });
        }
        let mut seq = next_seq(&self.conn, &collection.id)?;
        let mut placed = Vec::with_capacity(batch.len());
        for raw in batch {
            let mut message = ChatMessage::observe(collection, window, clock, raw)?;
            message.seq = seq;
            seq = seq.saturating_add(1);
            placed.push(message);
        }

        let mut stored = 0usize;
        in_transaction(&self.conn, || {
            for message in &placed {
                let identity = message.identity.as_str();
                let changed = self.conn.execute(
                    "INSERT INTO creator_chat_messages
                     (id,project_id,capture_session_id,collection_id,seq,identity,identity_kind,
                      wall_ms,media_us,uncertainty_us,sender,body,is_action)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
                     ON CONFLICT(capture_session_id, identity) DO NOTHING",
                    params![
                        uuid::Uuid::new_v4().to_string(),
                        collection.project_id.to_string(),
                        collection.capture_session_id.to_string(),
                        collection.id.to_string(),
                        i64::from(message.seq),
                        identity,
                        message.identity.kind_str(),
                        message.wall_ms,
                        message.position.media.micros(),
                        message.position.uncertainty_micros,
                        message.sender,
                        message.text,
                        i64::from(message.is_action),
                    ],
                )?;
                if changed == 1 {
                    stored += 1;
                }
            }
            Ok(())
        })?;
        Ok(ChatAppend {
            stored,
            already_present: batch.len() - stored,
        })
    }

    /// The session's messages, in capture order, each joined to its removal.
    pub fn chat_messages(&self, session: CaptureSessionId) -> Result<Vec<ChatMessage>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT collection_id, seq, identity, wall_ms, media_us, uncertainty_us,
                    sender, body, is_action
             FROM {TABLE_MESSAGES} WHERE capture_session_id=?1 ORDER BY wall_ms, seq"
        ))?;
        let rows = stmt.query_map([session.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, i64>(8)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (collection, seq, identity, wall_ms, media_us, uncertainty, sender, body, action) =
                row.map_err(ObservationError::Database)?;
            let mut message = ChatMessage {
                capture_session_id: session,
                project_id: ProjectId::generate(),
                collection_id: parse_id(&collection, TABLE_MESSAGES)?,
                seq: u32::try_from(seq).map_err(|_| ObservationError::UnreadableRow {
                    table: TABLE_MESSAGES,
                    detail: format!("seq {seq} is negative"),
                })?,
                identity: ChatIdentity::parse(&identity).ok_or_else(|| {
                    ObservationError::UnreadableRow {
                        table: TABLE_MESSAGES,
                        detail: format!("identity {identity:?} is not a spelling this build reads"),
                    }
                })?,
                wall_ms,
                position: SamplePosition {
                    media: media_of(media_us, TABLE_MESSAGES)?,
                    uncertainty_micros: uncertainty,
                },
                sender,
                text: body,
                is_action: action != 0,
                deletion: None,
            };
            message.deletion = self.deletion_for(session, &message.identity)?;
            out.push(message);
        }
        Ok(out)
    }

    /// The removal recorded against one identity, if any.
    fn deletion_for(
        &self,
        session: CaptureSessionId,
        identity: &ChatIdentity,
    ) -> Result<Option<ChatDeletion>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT wall_ms, by_login, target_identity, unresolved FROM {TABLE_DELETIONS}
             WHERE capture_session_id=?1 AND target_identity=?2 ORDER BY wall_ms LIMIT 1"
        ))?;
        let mut rows = stmt.query(params![session.to_string(), identity.as_str()])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let wall_ms: i64 = row.get(0)?;
        let by: String = row.get(1)?;
        let unresolved: i64 = row.get(3)?;
        Ok(Some(ChatDeletion {
            wall_ms,
            by,
            target: Some(identity.clone()),
            unresolved: unresolved != 0,
        }))
    }

    /// Record a batch of moderator removals.
    ///
    /// The removal is persisted whether or not the message is in hand, so a
    /// replay can report the ones it cannot show and a message that arrives
    /// later is marked rather than resurrected live. `marked` and `unresolved`
    /// are both stated: neither is derived by subtraction at the call site, and
    /// there is no third outcome.
    pub fn apply_chat_deletions(
        &self,
        session: CaptureSessionId,
        batch: &[RawChatDeletion],
    ) -> Result<DeletionOutcome> {
        if batch.len() > MAX_CHAT_BATCH {
            return Err(ObservationError::BatchTooLarge {
                len: batch.len(),
                limit: MAX_CHAT_BATCH,
            });
        }
        let mut marked = 0usize;
        let mut unresolved = 0usize;
        in_transaction(&self.conn, || {
            // The project comes from the capture session, not from a
            // collection: a removal can arrive before this daemon has opened
            // one, and refusing to record it would be exactly the silent drop
            // this model exists to prevent.
            let project: String = self.conn.query_row(
                "SELECT project_id FROM creator_capture_sessions WHERE id=?1",
                [session.to_string()],
                |r| r.get(0),
            )?;
            for removal in batch {
                // A removal that named nothing identifiable is stored with a
                // NULL target, which is a different row from one naming a
                // message we happen not to hold. Collapsing them would make
                // "the platform sent a timeout notice" indistinguishable from
                // "we lost that message".
                let identity = match &removal.target {
                    ChatDeletionTarget::ProviderId(id) => {
                        Some(ChatIdentity::ProviderId(id.clone()))
                    }
                    ChatDeletionTarget::Unidentifiable => None,
                };
                // The removal lives in its own table and the message row is
                // left exactly as it is, so nothing here can erase the text.
                // "Marked" therefore only means a message exists under that
                // identity for the join to find on read, and the row's
                // `unresolved` flag is that same fact: a removal is
                // unresolvable either because it named nothing or because it
                // named something this daemon does not hold. The two stay
                // apart in `target_identity`, which is NULL only in the first
                // case.
                let held: i64 = match &identity {
                    Some(id) => self.conn.query_row(
                        "SELECT COUNT(*) FROM creator_chat_messages
                         WHERE capture_session_id=?1 AND identity=?2",
                        params![session.to_string(), id.as_str()],
                        |r| r.get(0),
                    )?,
                    None => 0,
                };
                self.conn.execute(
                    "INSERT INTO creator_chat_deletions
                     (id,project_id,capture_session_id,target_identity,wall_ms,by_login,unresolved)
                     VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![
                        uuid::Uuid::new_v4().to_string(),
                        project,
                        session.to_string(),
                        identity.as_ref().map(ChatIdentity::as_str),
                        removal.wall_ms,
                        removal.by,
                        i64::from(held == 0),
                    ],
                )?;
                if held == 0 {
                    unresolved += 1;
                } else {
                    marked += 1;
                }
            }
            Ok(())
        })?;
        Ok(DeletionOutcome { marked, unresolved })
    }

    /// The session's removals, in capture order.
    pub fn chat_deletions(&self, session: CaptureSessionId) -> Result<Vec<ChatDeletion>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT target_identity, wall_ms, by_login, unresolved FROM {TABLE_DELETIONS}
             WHERE capture_session_id=?1 ORDER BY wall_ms"
        ))?;
        let rows = stmt
            .query_map([session.to_string()], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, rusqlite::Error>>()?;
        rows.into_iter()
            .map(|(target, wall_ms, by, unresolved)| {
                Ok(ChatDeletion {
                    wall_ms,
                    by,
                    // A NULL target is a recorded fact, not a corrupt row, so
                    // it is not an error here; only an unreadable non-NULL one is.
                    target: match target.as_deref() {
                        None => None,
                        Some(raw) => Some(ChatIdentity::parse(raw).ok_or_else(|| {
                            ObservationError::UnreadableRow {
                                table: TABLE_DELETIONS,
                                detail: format!(
                                    "target_identity {raw:?} is not a spelling this build reads"
                                ),
                            }
                        })?),
                    },
                    unresolved: unresolved != 0,
                })
            })
            .collect()
    }

    /// Record an interval nothing was collected in.
    pub fn record_chat_gap(&self, collection: &ChatCollection, gap: &CoverageGap) -> Result<()> {
        insert_gap(
            &self.conn,
            collection.project_id,
            collection.capture_session_id,
            Some(&collection.id),
            gap,
            "",
        )
    }

    /// The session's recorded gaps, in capture order.
    pub fn chat_gaps(&self, session: CaptureSessionId) -> Result<Vec<CoverageGap>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT start_wall_ms, end_wall_ms, reason FROM {TABLE_GAPS}
             WHERE capture_session_id=?1 ORDER BY start_wall_ms, end_wall_ms"
        ))?;
        let rows = stmt
            .query_map([session.to_string()], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, rusqlite::Error>>()?;
        rows.into_iter()
            .map(|(start, end, reason)| {
                Ok(CoverageGap {
                    start_wall_ms: start,
                    end_wall_ms: end,
                    reason: GapReason::parse(&reason).ok_or_else(|| {
                        ObservationError::UnreadableRow {
                            table: TABLE_GAPS,
                            detail: format!("reason {reason:?} is not a reason this build names"),
                        }
                    })?,
                })
            })
            .collect()
    }

    /// How much of `window` the daemon was actually collecting for.
    ///
    /// **The numerator is the union of the open collection intervals, not the
    /// messages.** A quiet room is fully covered; a collector that stopped is
    /// not, whatever it happened to have said before it stopped. The recorded
    /// gaps are subtracted from that union and reported, and
    /// [`Coverage::validated`] refuses a shortfall that names none — so
    /// "we covered 40% of this stream" cannot be printed without saying which
    /// 60% nobody was watching.
    pub fn chat_coverage(
        &self,
        session: CaptureSessionId,
        window: &CaptureWindow,
    ) -> Result<Coverage> {
        let collections = self.chat_collections(session)?;
        let gaps = self.chat_gaps(session)?;

        let mut open: Vec<(i64, i64)> = collections
            .iter()
            .map(|c| {
                (
                    c.opened_at_wall_ms.max(window.start_wall_ms),
                    c.ended_wall_ms
                        .unwrap_or(window.end_wall_ms)
                        .min(window.end_wall_ms),
                )
            })
            .filter(|(lo, hi)| hi > lo)
            .collect();
        if open.is_empty() {
            // Nothing was ever collecting here, so the whole window is a gap
            // with a name. Reporting 1 000 per mille for a room nobody listened
            // to would be the exact "silently empty" failure this model exists
            // to prevent.
            let gap = CoverageGap::new(
                window.start_wall_ms,
                window.end_wall_ms,
                GapReason::NotConnected,
            )?;
            let coverage = Coverage {
                expected_wall_ms: window.span_wall_ms(),
                observed_wall_ms: 0,
                per_mille: 0,
                gaps: vec![gap],
                widest_uncertainty_micros: 0,
            };
            coverage.validated()?;
            return Ok(coverage);
        }
        open.sort_unstable();

        // Subtract the recorded gaps from the open intervals, so a gap recorded
        // inside a live collection (a refused batch, a socket that dropped and
        // came back) is not counted as covered by the collection's own span.
        let covered = subtract(&open, &gaps, window);
        // The complement is a gap too, and it is named rather than left for
        // `validated()` to refuse: a collector that closed early left the rest
        // of the window uncovered, and that interval is `not_connected` —
        // literally true, and distinct from the `disconnected` a collector
        // records for itself when its own socket drops.
        let mut all_gaps = complement(&covered, window);
        all_gaps.extend(gaps.iter().copied());
        all_gaps.sort_by_key(|g| (g.start_wall_ms, g.end_wall_ms));
        all_gaps.dedup();

        let observed_wall_ms: i64 = covered.iter().map(|(lo, hi)| hi - lo).sum();
        let expected_wall_ms = window.span_wall_ms();
        let per_mille = (observed_wall_ms * 1_000 / expected_wall_ms.max(1)).clamp(0, 1_000) as u32;
        let coverage = Coverage {
            expected_wall_ms,
            observed_wall_ms,
            per_mille,
            gaps: all_gaps,
            widest_uncertainty_micros: 0,
        };
        coverage.validated()?;
        Ok(coverage)
    }

    /// The session's chat, aligned to a named landmark on the media timeline.
    ///
    /// The landmark is not optional: "aligned to a capture landmark" is a claim
    /// about a specific instant, and an API that let a caller omit it would let
    /// a report print offsets with nothing to be relative to. Every offset is
    /// `None`-free by construction — a message's position is a `MediaTime` and
    /// a landmark is a `MediaTime`, so the subtraction is total — and a message
    /// *before* the landmark yields `None` from
    /// [`Landmark::offset_of`] rather than a wrapped positive number.
    pub fn replay_chat(
        &self,
        session: CaptureSessionId,
        window: &CaptureWindow,
        landmark: &Landmark,
    ) -> Result<ChatReplay> {
        let messages = self.chat_messages(session)?;
        let deletions = self.chat_deletions(session)?;
        let mut replayed = Vec::with_capacity(messages.len());
        for message in messages {
            // Re-placed through the same refusal a write went through, so a
            // replay cannot show a message the clock would not accept. It also
            // means the window bounds apply on read: a message outside the
            // replayed window is not silently included.
            if !window.contains(message.wall_ms) {
                continue;
            }
            let offset = landmark.offset_of(message.position.media).ok_or_else(|| {
                ObservationError::UnreadableRow {
                    table: TABLE_MESSAGES,
                    detail: format!(
                        "message at media {} precedes landmark {}",
                        message.position.media.micros(),
                        landmark.at.micros()
                    ),
                }
            })?;
            replayed.push(ReplayedChat {
                position: message.position,
                message,
                offset_from_landmark: offset,
            });
        }
        let unresolved_deletions = deletions.iter().filter(|d| d.unresolved).count();
        Ok(ChatReplay {
            landmark: landmark.clone(),
            messages: replayed,
            deletions,
            unresolved_deletions,
            gaps: self.chat_gaps(session)?,
            coverage: self.chat_coverage(session, window)?,
        })
    }
}

/// The open intervals minus the recorded gaps, clipped to the window.
///
/// Subtraction rather than inclusion, because a gap inside a live collection is
/// the interesting case: the collector was open, and something still happened
/// in there that cost the messages.
fn subtract(open: &[(i64, i64)], gaps: &[CoverageGap], window: &CaptureWindow) -> Vec<(i64, i64)> {
    let mut merged_gaps = gaps
        .iter()
        .map(|g| {
            (
                g.start_wall_ms.max(window.start_wall_ms),
                g.end_wall_ms.min(window.end_wall_ms),
            )
        })
        .filter(|(lo, hi)| hi > lo)
        .collect::<Vec<_>>();
    merged_gaps.sort_unstable();

    let mut out = Vec::new();
    for (lo, hi) in open {
        let mut cursor = *lo;
        for (glo, ghi) in &merged_gaps {
            if *ghi <= cursor {
                continue;
            }
            if *glo >= *hi {
                break;
            }
            if *glo > cursor {
                out.push((cursor, (*glo).min(*hi)));
            }
            cursor = cursor.max(*ghi);
            if cursor >= *hi {
                break;
            }
        }
        if cursor < *hi {
            out.push((cursor, *hi));
        }
    }
    out
}

/// The intervals of `window` that `covered` does not cover, each named
/// [`GapReason::NotConnected`] because no collector was open across it.
fn complement(covered: &[(i64, i64)], window: &CaptureWindow) -> Vec<CoverageGap> {
    let mut out = Vec::new();
    let mut cursor = window.start_wall_ms;
    for (lo, hi) in covered {
        if *lo > cursor {
            if let Ok(gap) = CoverageGap::new(cursor, *lo, GapReason::NotConnected) {
                out.push(gap);
            }
        }
        cursor = cursor.max(*hi);
    }
    if cursor < window.end_wall_ms {
        if let Ok(gap) = CoverageGap::new(cursor, window.end_wall_ms, GapReason::NotConnected) {
            out.push(gap);
        }
    }
    out
}

fn insert_gap(
    conn: &Connection,
    project: ProjectId,
    session: CaptureSessionId,
    collection: Option<&ChatCollectionId>,
    gap: &CoverageGap,
    detail: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO creator_chat_gaps
         (id,project_id,capture_session_id,collection_id,start_wall_ms,end_wall_ms,reason,detail)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            uuid::Uuid::new_v4().to_string(),
            project.to_string(),
            session.to_string(),
            collection.map(ToString::to_string),
            gap.start_wall_ms,
            gap.end_wall_ms,
            gap.reason.as_str(),
            detail,
        ],
    )?;
    Ok(())
}

/// Adopt a stored UUID as a typed id, refusing a row whose id this build cannot
/// read rather than minting a fresh one — a minted id would detach the row from
/// the collection that owns it.
fn parse_id<T: From<uuid::Uuid>>(raw: &str, table: &'static str) -> Result<T> {
    uuid::Uuid::parse_str(raw)
        .map(T::from)
        .map_err(|e| ObservationError::UnreadableRow {
            table,
            detail: format!("id {raw:?}: {e}"),
        })
}
