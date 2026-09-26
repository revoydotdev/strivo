-- Version 8, reversed: the durable chat record goes away.
--
-- **The drop destroys the chat archive.** Same honest cost as version 5's
-- `down`: there is no earlier version of this state, and keeping it would mean
-- a second place holding messages. This is why
-- `docs/RESEARCH-PLATFORM-ROADMAP.md:72` permits a destructive `down` only
-- rehearsed from a backup, and why the rehearsal copies the file.
--
-- Order is reverse creation, so the messages and deletions that reference a
-- collection go before the collection itself. SQLite does not enforce
-- referential integrity on a `down` unless `PRAGMA foreign_keys` happens to be
-- on for that connection, and the order is what makes this file correct either
-- way rather than dependent on a pragma.
--
-- The version line is NOT stamped here. It belongs to `schema::rollback`,
-- which writes it in the same transaction as this file's statements.

DROP TABLE IF EXISTS creator_chat_gaps;

DROP TABLE IF EXISTS creator_chat_deletions;

DROP TABLE IF EXISTS creator_chat_messages;

DROP TABLE IF EXISTS creator_chat_collections;
