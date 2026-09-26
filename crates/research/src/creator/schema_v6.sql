-- Version 6: source ownership, licence and consent notes — the review
-- requirement, recorded once per source identity.
--
-- **One row per source, and no eligibility column.** The whole point of this
-- version is that publication eligibility is *derived* from the three axes
-- rather than stored, so there is deliberately no `publishable`, `eligible`
-- or `cleared` column here. A test reads `pragma_table_info` on this table and
-- fails if one appears: a stored flag is a flag a caller can write, and a
-- caller who can write "eligible" has not been gated. Todo 27's design note —
-- "a wiring some process forgets is a fail-open gate" — is why.
--
-- The primary key is `source_asset_id` and nothing else. That is todo 33's
-- identity rule applied to licence data: a path is a locator, and a licence
-- note attached to a path would silently re-attach to different bytes when the
-- file moves. There is no unique constraint anywhere else in this table
-- either, and `locator` is not in it at all — there is nothing here that could
-- be looked up by where the bytes happen to be.
--
-- The three axes are separate columns with separate `CHECK`s, because they are
-- separate decisions. A `rights_ok` column would collapse "licence-clear but
-- no consent for the chat overlay" and "every participant consented but
-- nobody checked the licence" into the single answer a creator most wants to
-- be wrong about.
--
-- `ownership` carries the plan's distinction directly: `archive_only` is
-- personal-archive material — kept, never published — and it is not a flavour
-- of the licence. A file somebody let the creator keep and a file the creator
-- may republish are different facts.
--
-- `consent_scope` holds the scope of a grant *or* the reason for a refusal, so
-- a consent decision has one text field rather than two that could disagree
-- about what was said. `consent_valid_until` is a `YYYY-MM-DD` **calendar
-- date** and is deliberately not a timestamp: a consent expires on a day, and
-- that is all a person states. It is `NULL` for a grant that does not expire.
--
-- The `CHECK`s are the enforcement, not a Rust `if`. The three "a decision
-- with nothing attached" guards matter most: a granted licence with no grant
-- text, a granted consent with no scope and a refused consent with no reason
-- are all refused by the database, so the constraint holds for a row written by
-- a tool this crate does not own. Text is length-bounded because it is
-- untrusted free text bound for a UI; over the limit is a refusal at the Rust
-- boundary too, never a truncation, because a shortened grant reads exactly
-- like a whole one.
--
-- The `fingerprint` is the derived content hash of the *decision* (the three
-- axes plus the disclosure, never `recorded_at` or `recorded_by`), so a caller
-- that reviewed a note can detect that it has since been edited — the same rule
-- `creator_workspace.selected_fingerprint` and
-- `creator_render_plans.content_hash` already use.

CREATE TABLE IF NOT EXISTS creator_source_notes (
 source_asset_id TEXT PRIMARY KEY
   REFERENCES creator_source_assets(id) ON DELETE CASCADE,
 ownership TEXT NOT NULL
   CHECK(ownership IN ('creator_owned','third_party','archive_only','undetermined')),
 holder TEXT NOT NULL DEFAULT '' CHECK(LENGTH(holder) <= 4096),
 licence TEXT NOT NULL CHECK(licence IN ('granted','absent','undetermined')),
 licence_grant TEXT NOT NULL DEFAULT '' CHECK(LENGTH(licence_grant) <= 4096),
 consent TEXT NOT NULL
   CHECK(consent IN ('granted','refused','not_required','undetermined')),
 consent_scope TEXT NOT NULL DEFAULT '' CHECK(LENGTH(consent_scope) <= 4096),
 consent_valid_until TEXT
   CHECK(consent_valid_until IS NULL
         OR (LENGTH(consent_valid_until) = 10 AND consent_valid_until NOT GLOB '*[^0-9-]*')),
 disclosure TEXT NOT NULL DEFAULT '' CHECK(LENGTH(disclosure) <= 4096),
 fingerprint TEXT NOT NULL CHECK(LENGTH(fingerprint) = 64),
 recorded_by TEXT NOT NULL CHECK(LENGTH(recorded_by) BETWEEN 1 AND 256),
 recorded_at TEXT NOT NULL,
 -- A decision with nothing attached to it is refused here rather than in Rust,
 -- so the rule holds for a row written by any tool.
 CHECK(licence <> 'granted' OR LENGTH(TRIM(licence_grant)) > 0),
 CHECK(consent <> 'granted' OR LENGTH(TRIM(consent_scope)) > 0),
 CHECK(consent <> 'refused' OR LENGTH(TRIM(consent_scope)) > 0),
 CHECK(ownership <> 'third_party' OR LENGTH(TRIM(holder)) > 0)
);
