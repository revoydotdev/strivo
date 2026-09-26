//! Tests for the Creator identity model.
//!
//! Split by the class of thing under test: the identity invariants in
//! [`identity`], wrong-kind and malformed inputs in [`malformed`], the
//! migration and rollback contract in [`migration`] and [`rollback`], the
//! durable job and artifact lifecycle in [`jobs_lifecycle`], and the the
//! time model in three: its data migration in [`time_migration`], its shape
//! and its anti-drift check in [`time_schema`], and the clock segments it
//! stores in [`time_clock`]. The project workspace — one project, many
//! recordings and edits, one selected revision every view reads — is in
//! [`workspace`], with what it refuses in [`workspace_malformed`] and what
//! reversing it costs in [`workspace_rollback`]. The OBS multitrack
//! import — track identification and the sample alignment that keeps the stems
//! together — is in [`multitrack`].

mod fixtures;
mod identity;
mod import;
mod jobs_lifecycle;
mod malformed;
mod migration;
mod multitrack;
mod rollback;
mod time_clock;
mod time_migration;
mod time_schema;
mod time_shared;
mod workspace;
mod workspace_malformed;
mod workspace_rollback;
