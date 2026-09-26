-- Version 8: daemon-owned durable chat — collections, messages, deletions and
-- the recorded acquisition gaps.
--
-- Before this version, chat was a browser WebSocket
-- (`crates/strivo-web/src/routes/chat.rs:1-9`): the record existed only while a
-- tab was open, nothing was persisted, and no message had a media coordinate.
-- Four tables, because four different facts are stored and collapsing them
-- loses one:
--
--   creator_chat_collections — one row per stretch of time the daemon was
--     collecting a room. Its interval is the acquisition record. `ended_wall_ms`
--     NULL means "still open", and that NULL is the whole restart mechanism:
--     a process that dies leaves the row open, so the next process can name the
--     interval nobody was watching instead of assuming it was covered.
--
--   creator_chat_messages — one row per message, placed on the source clock.
--
--   creator_chat_deletions — a moderator's removal, recorded as a fact. It is
--     NOT applied as a delete: erasing the row would leave a hole with no
--     witness, which is the exact failure this whole todo exists to close.
--
--   creator_chat_gaps — an interval of the expected window nothing was
--     collected in, with its cause. There is no dropped-message counter
--     anywhere in this schema, deliberately: a number nobody reads is how a
--     gap becomes invisible.
--
-- **Identity is content, and it is UNIQUE in the schema rather than checked in
-- Rust.** A message the platform gave an id is identified by that id; one it
-- did not is identified by a sha256 over (room, sender, wall instant, body).
-- Both spellings live in one TEXT column with a distinguishing prefix, because
-- a fingerprint and a platform id are both opaque strings and conflating them
-- would let a platform id be mistaken for a content hash. `UNIQUE
-- (capture_session_id, identity)` is what makes deduplication a property of the
-- database rather than a convention a re-import can route around — the same
-- rule todo 33 established for sources and todo 38 for rights notes.
--
-- `wall_ms` is the **provider's** own timestamp (`tmi-sent-ts`) when it sent
-- one, because that is when the message happened, and the daemon's receive
-- instant otherwise. `media_us` and `uncertainty_us` are derived from the
-- canonical `ClockMap` at write time; a message no segment covers is refused
-- whole rather than clamped to an edge, so an unplaceable row cannot exist.
--
-- `seq` is assigned by the store from `MAX(seq) + 1` within a collection, so a
-- resumed collection continues rather than colliding with a batch it never saw
-- committed.

CREATE TABLE IF NOT EXISTS creator_chat_collections (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 capture_session_id TEXT NOT NULL REFERENCES creator_capture_sessions(id) ON DELETE CASCADE,
 platform TEXT NOT NULL CHECK(LENGTH(platform) BETWEEN 1 AND 32),
 room TEXT NOT NULL CHECK(LENGTH(room) BETWEEN 1 AND 256),
 opened_at_wall_ms INTEGER NOT NULL,
 -- NULL is the record of a collector that is still open, and of a process that
 -- died holding one. The CHECK keeps a closed row's two ends in order.
 ended_wall_ms INTEGER,
 end_reason TEXT CHECK(end_reason IS NULL OR end_reason IN ('closed','disconnected','daemon_down')),
 CHECK(ended_wall_ms IS NULL OR ended_wall_ms >= opened_at_wall_ms)
);

CREATE INDEX IF NOT EXISTS creator_chat_collections_by_session
 ON creator_chat_collections(capture_session_id, room, opened_at_wall_ms);

CREATE TABLE IF NOT EXISTS creator_chat_messages (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 capture_session_id TEXT NOT NULL REFERENCES creator_capture_sessions(id) ON DELETE CASCADE,
 collection_id TEXT NOT NULL REFERENCES creator_chat_collections(id) ON DELETE CASCADE,
 seq INTEGER NOT NULL CHECK(seq >= 0),
 -- 'id:<platform id>' or 'fp:<64 hex sha256>'. See the module note: one
 -- column, two spellings, never conflated.
 identity TEXT NOT NULL CHECK(LENGTH(identity) > 0),
 identity_kind TEXT NOT NULL CHECK(identity_kind IN ('provider_id','fingerprint')),
 wall_ms INTEGER NOT NULL,
 media_us INTEGER NOT NULL CHECK(media_us >= 0),
 uncertainty_us INTEGER NOT NULL CHECK(uncertainty_us >= 0),
 sender TEXT NOT NULL CHECK(LENGTH(sender) BETWEEN 1 AND 256),
 -- Untrusted external text, carried verbatim and never interpreted. Bounded
 -- so it cannot fill a disk; over the limit is a refusal, not a truncation,
 -- because a shortened body reads exactly like a whole one.
 body TEXT NOT NULL CHECK(LENGTH(body) <= 4096),
 is_action INTEGER NOT NULL DEFAULT 0 CHECK(is_action IN (0,1)),
 UNIQUE(capture_session_id, identity),
 UNIQUE(collection_id, seq)
);

CREATE INDEX IF NOT EXISTS creator_chat_messages_by_session
 ON creator_chat_messages(capture_session_id, wall_ms);

CREATE TABLE IF NOT EXISTS creator_chat_deletions (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 capture_session_id TEXT NOT NULL REFERENCES creator_capture_sessions(id) ON DELETE CASCADE,
 -- NULL when the removal named no message we could identify, which is a
 -- different fact from naming one we do not hold and both are reportable.
 target_identity TEXT CHECK(target_identity IS NULL OR LENGTH(target_identity) > 0),
 wall_ms INTEGER NOT NULL,
 by_login TEXT NOT NULL CHECK(LENGTH(by_login) BETWEEN 1 AND 256),
 unresolved INTEGER NOT NULL CHECK(unresolved IN (0,1))
);

CREATE INDEX IF NOT EXISTS creator_chat_deletions_by_session
 ON creator_chat_deletions(capture_session_id, wall_ms);

CREATE TABLE IF NOT EXISTS creator_chat_gaps (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 capture_session_id TEXT NOT NULL REFERENCES creator_capture_sessions(id) ON DELETE CASCADE,
 -- The collection that was open when the gap was noticed, or NULL when the
 -- gap is about there having been no collection at all.
 collection_id TEXT REFERENCES creator_chat_collections(id) ON DELETE SET NULL,
 start_wall_ms INTEGER NOT NULL,
 end_wall_ms INTEGER NOT NULL,
 reason TEXT NOT NULL CHECK(reason IN ('not_connected','disconnected','daemon_down',
                                        'reconnect_gap','dropped_sample','no_samples')),
 detail TEXT NOT NULL DEFAULT '',
 -- A gap is an interval. A zero-length one would be a point, and a point gap
 -- cannot be drawn on a timeline, so the schema refuses it rather than letting
 -- a rounding artefact into the record. Table-level, and last: SQLite rejects
 -- a table constraint declared before the remaining column definitions.
 CHECK(end_wall_ms > start_wall_ms)
);

CREATE INDEX IF NOT EXISTS creator_chat_gaps_by_session
 ON creator_chat_gaps(capture_session_id, start_wall_ms);
