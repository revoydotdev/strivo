//! Strivo Pro licensing client (Phase 3 foundation).
//!
//! Three responsibilities:
//!
//!   1. Resolve a stable, machine-local identifier the activation server
//!      can bind a licence to. We read the OS-provided machine ID where
//!      available and fall back to a v4 UUID we generate + persist.
//!   2. Read/write the licence cache at
//!      `~/.local/share/strivo/licence.json`. The activation backend
//!      (CF Workers + D1) signs a token + a refreshed-by-72h timestamp;
//!      we cache it so offline boxes stay entitled across reboots.
//!   3. Answer `is_entitled(plugin) -> bool` for the plugin loader,
//!      respecting the dev override (`STRIVO_DEV_UNLOCK_ALL=1`) and the
//!      "internet might be down — don't kill in-flight features" rule.
//!
//! Phase 3b (the CF Worker) plugs in here via the existing
//! `/api/v1/licence/{activate,trial,refresh}` routes in `strivo-web`,
//! which today return 501. When the backend lands, those routes will
//! call into `apply_token()` / `start_trial()` below.

pub mod cache;
pub mod client;
pub mod gate;
pub mod machine_id;
pub mod verify;

pub use cache::{Licence, LicenceCache, Tier};
pub use client::{spawn_refresh_loop, DEFAULT_REFRESH_INTERVAL};
pub use gate::is_entitled;
pub use machine_id::machine_id;
pub use verify::verify_es256;
