//! Tests for the Creator identity model.
//!
//! Split by the class of thing under test: the identity invariants in
//! [`identity`], wrong-kind and malformed inputs in [`malformed`], the
//! migration and rollback contract in [`migration`] and [`rollback`], and the
//! time model in three: its data migration in [`time_migration`], its shape
//! and its anti-drift check in [`time_schema`], and the clock segments it
//! stores in [`time_clock`].

mod fixtures;
mod identity;
mod malformed;
mod migration;
mod rollback;
mod time_clock;
mod time_migration;
mod time_schema;
mod time_shared;
