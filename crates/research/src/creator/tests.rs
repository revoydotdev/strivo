//! Tests for the Creator identity model.
//!
//! Split by the class of thing under test: the identity invariants in
//! [`identity`], wrong-kind and malformed inputs in [`malformed`], and the
//! migration and rollback contract in [`migration`] and [`rollback`].

mod fixtures;
mod identity;
mod malformed;
mod migration;
mod rollback;
