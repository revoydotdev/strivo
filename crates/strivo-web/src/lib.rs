//! StriVo web UI — *arr-style HTTP frontend over the existing daemon IPC.
//!
//! The web server is stateless: every request reads from the running daemon
//! over the Unix socket defined in `strivo_core::ipc`. The daemon is the
//! single source of truth, shared with the TUI.

pub mod assets;
pub mod auth;
pub mod csrf;
pub mod ipc_client;
pub mod problem;
pub mod ratelimit;
pub mod routes;
pub mod server;
pub mod telemetry;

pub use server::{serve, ServeConfig};
