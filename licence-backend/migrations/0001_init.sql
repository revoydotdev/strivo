-- strivo-licence — D1 schema.
--
-- One row per (licence_key, machine_hash). Trials store an empty
-- licence_key so they don't collide with paid activations. Refunds
-- flip `revoked_at` so the next /refresh denies.
--
-- Indices: licence_key lookups (activate), machine_hash lookups
-- (trial allow-list), revoked_at for cleanup jobs.

CREATE TABLE IF NOT EXISTS licences (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    licence_key      TEXT NOT NULL,           -- Lemon Squeezy key, '' for trials
    machine_hash     TEXT NOT NULL,           -- SHA-256 of OS machine_id
    tier             TEXT NOT NULL CHECK (tier IN ('trial', 'pro')),
    email            TEXT,                    -- from LS purchase
    activated_at     TEXT NOT NULL,           -- ISO-8601 UTC
    last_refreshed   TEXT NOT NULL,           -- ISO-8601 UTC
    expires_at       TEXT,                    -- NULL for unlimited (paid)
    revoked_at       TEXT,                    -- non-NULL = refunded/disputed
    UNIQUE (licence_key, machine_hash)
);

CREATE INDEX IF NOT EXISTS idx_licences_licence_key ON licences(licence_key);
CREATE INDEX IF NOT EXISTS idx_licences_machine_hash ON licences(machine_hash);
CREATE INDEX IF NOT EXISTS idx_licences_revoked ON licences(revoked_at);

-- Trials are one-shot per machine. This view is used by /trial to
-- decide whether to issue or reject.
CREATE TABLE IF NOT EXISTS trial_claims (
    machine_hash     TEXT PRIMARY KEY,
    claimed_at       TEXT NOT NULL
);

-- Webhook events from Lemon Squeezy — kept for audit, replay-protected
-- by event_id uniqueness.
CREATE TABLE IF NOT EXISTS webhook_events (
    event_id         TEXT PRIMARY KEY,
    kind             TEXT NOT NULL,
    payload          TEXT NOT NULL,
    received_at      TEXT NOT NULL
);

-- Rate-limit log (IP-based). The Workers Rate Limiting binding does
-- the hot-path counting; this is for after-the-fact analysis.
CREATE TABLE IF NOT EXISTS denied_attempts (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    ip               TEXT NOT NULL,
    path             TEXT NOT NULL,
    reason           TEXT NOT NULL,
    at               TEXT NOT NULL
);
