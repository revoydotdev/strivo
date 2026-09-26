-- Version 5, reversed: the observation series, the availability states and
-- the live-status trigger all go away.
--
-- **The drop destroys the recorded series.** That is the honest cost of
-- reversing the version that introduced it: there is no earlier version of
-- this state to fall back to, and keeping it would mean a second place
-- recording observations. It is why
-- `docs/RESEARCH-PLATFORM-ROADMAP.md:72` permits a destructive `down` only
-- when rehearsed from a backup, and why the rehearsal copies the file.
--
-- Order is reverse creation: the two indexes go with the tables that own
-- them, so they are not named separately — SQLite drops a table's own
-- indexes with it, and naming them here would be a `down` that can fail on
-- an index a later build already dropped.
--
-- The trigger is dropped explicitly, and it has to be: a trigger survives
-- the table it was defined on in the sense that dropping that table does not
-- remove a trigger defined on a *different* table. This one is defined on
-- `creator_metric_observations`, which version 5 does NOT drop, so leaving
-- it would leave a live refusal in place on a table the rolled-back schema
-- believes nothing about.
--
-- The version line is NOT stamped here. It belongs to `schema::rollback`,
-- which writes it in the same transaction as this file's statements.
--
-- `SELECT` nothing. A `down` that reported what it removed would be a
-- second place summarising the schema.

DROP TRIGGER IF EXISTS creator_metric_observations_reject_live_status;

DROP TABLE IF EXISTS creator_metric_availability;

DROP TABLE IF EXISTS creator_stream_events;

DROP TABLE IF EXISTS creator_viewer_samples;
