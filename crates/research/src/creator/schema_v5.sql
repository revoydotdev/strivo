-- Version 5: the observation series — viewer samples and stream events —
-- and the refusal that keeps live status out of the outcome table.
--
-- These are two different things and they are not one table. A viewer sample
-- is a *poll*: a periodic reading of what the platform was showing at a
-- wall-clock instant, placed on the source clock with a stated uncertainty.
-- `creator_metric_observations` (version 1) is a *measured outcome* of
-- published work: a provider's number over a closed interval, with a
-- denominator. Conflating them would let a live-status badge value be
-- reported as though a stream's performance had been measured.
--
-- The only viewer field in the daemon is `concurrent_viewers` at
-- `src/platform/youtube.rs:118,520-522`, read from a live status endpoint
-- and never persisted. So version 5 adds a **trigger** refusing those names
-- in the observation table, and it is a trigger rather than a Rust `if`
-- because the observation table is written by import adapters this crate
-- does not own: a constraint a caller can route around is a comment. The
-- enforcement lives where the write happens.
--
-- Time is integer microseconds, the single Creator representation. The poll
-- cadence stays in **milliseconds** deliberately and is the one wall-clock
-- integer here: it is a duration the *provider* states in milliseconds, not
-- a position on a timeline, and converting it to microseconds on the way in
-- would be inventing a precision the provider did not offer — a 250 ms poll
-- is 250 ms, and `250_000` would read as exactly 250 000 µs of accuracy it
-- does not have. `uncertainty_us` is microseconds because it is a derived
-- bound on a position, in the same unit as the position.
--
-- `wall_ms` and `media_us` are BOTH NOT NULL. A row with a NULL wall clock
-- is a row an interrupted writer left, and the reader refuses it rather than
-- defaulting: read as 0 it would place a truncated sample on the first frame
-- of the recording, which is the fabricated-precision defect this whole
-- version exists to prevent.
--
-- `(capture_session_id, seq)` is UNIQUE and the store assigns `seq` from
-- `MAX(seq) + 1`, so a capture that is interrupted and resumed continues the
-- series instead of restarting it and colliding with a batch it never saw
-- committed.
--
-- The third table is the availability state. A metric nobody holds a
-- permission for is a fact with a reason, persisted and readable, so it can
-- never be mistaken for a series that was collected and came back empty.

CREATE TABLE IF NOT EXISTS creator_viewer_samples (
 id TEXT PRIMARY KEY,
 capture_session_id TEXT NOT NULL REFERENCES creator_capture_sessions(id) ON DELETE CASCADE,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 seq INTEGER NOT NULL CHECK(seq >= 0),
 wall_ms INTEGER NOT NULL,
 media_us INTEGER NOT NULL CHECK(media_us >= 0),
 -- Every row carries its own bound. A position without one is the defect.
 uncertainty_us INTEGER NOT NULL CHECK(uncertainty_us >= 0),
 viewer_count INTEGER NOT NULL CHECK(viewer_count >= 0),
 poll_interval_ms INTEGER NOT NULL CHECK(poll_interval_ms > 0),
 dropped INTEGER NOT NULL DEFAULT 0 CHECK(dropped IN (0,1)),
 UNIQUE(capture_session_id, seq)
);

CREATE INDEX IF NOT EXISTS creator_viewer_samples_by_session
 ON creator_viewer_samples(capture_session_id, wall_ms);

CREATE TABLE IF NOT EXISTS creator_stream_events (
 id TEXT PRIMARY KEY,
 capture_session_id TEXT NOT NULL REFERENCES creator_capture_sessions(id) ON DELETE CASCADE,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 seq INTEGER NOT NULL CHECK(seq >= 0),
 kind TEXT NOT NULL CHECK(kind IN ('marker','category','provider_event')),
 wall_ms INTEGER NOT NULL,
 media_us INTEGER NOT NULL CHECK(media_us >= 0),
 uncertainty_us INTEGER NOT NULL CHECK(uncertainty_us >= 0),
 -- Untrusted external text, carried as data and never interpreted. Bounded
 -- so it cannot fill a disk; over the limit is a refusal, not a truncation.
 label TEXT NOT NULL CHECK(LENGTH(label) <= 4096),
 category TEXT CHECK(category IS NULL OR LENGTH(category) <= 4096),
 payload_json TEXT NOT NULL DEFAULT '{}',
 UNIQUE(capture_session_id, seq)
);

CREATE INDEX IF NOT EXISTS creator_stream_events_by_session
 ON creator_stream_events(capture_session_id, wall_ms);

CREATE TABLE IF NOT EXISTS creator_metric_availability (
 capture_session_id TEXT NOT NULL REFERENCES creator_capture_sessions(id) ON DELETE CASCADE,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 metric TEXT NOT NULL CHECK(LENGTH(metric) BETWEEN 1 AND 256),
 state TEXT NOT NULL CHECK(state IN ('observed','unavailable')),
 reason TEXT NOT NULL,
 detail TEXT NOT NULL DEFAULT '',
 declared_at TEXT NOT NULL,
 -- One declared state per metric per session. A second declaration replaces
 -- the first rather than accumulating: two rows would give "is it
 -- available?" two answers.
 PRIMARY KEY(capture_session_id, metric)
);

CREATE TRIGGER IF NOT EXISTS creator_metric_observations_reject_live_status
 BEFORE INSERT ON creator_metric_observations
 WHEN NEW.metric IN ('concurrent_viewers','viewer_count','live_viewers',
                     'is_live','live_status','followers')
BEGIN
 SELECT RAISE(ABORT,
   'live-status/live-status: this metric is a live-status reading, not a measured outcome. '
   || 'It belongs in creator_viewer_samples, which carries it with its own uncertainty bound.');
END;
