//! Licence cache — the on-disk record of the user's entitlement.
//!
//! File: `~/.local/share/strivo/licence.json`.
//!
//! Written by `/api/v1/licence/{activate,trial,refresh}` (Phase 3b).
//! Read by `gate::is_entitled` on every plugin-load decision and by
//! the `/api/v1/licence/status` route.
//!
//! Schema: see [`Licence`]. The activation server signs `token` with
//! ES256; the entitlement gate verifies it on every read.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Free,
    Trial,
    Pro,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Licence {
    pub tier: Tier,
    /// SHA-256 of the machine_id the activation was bound to. The
    /// gate function refuses to honour a cache whose hash doesn't
    /// match this machine — prevents lifting `licence.json` off one
    /// box and dropping it on another.
    pub machine_hash: String,
    /// ISO-8601. For trial: trial expiry; for pro: licence expiry
    /// (today: "never" represented as `None`).
    pub expires_at: Option<String>,
    /// ISO-8601 of the last successful server refresh. The 72h
    /// refresh rule is applied on top of this; if the server has
    /// been unreachable for >72h the cache is still honoured.
    pub last_refreshed: String,
    /// JWT ES256 token from the activation server.
    #[serde(default)]
    pub token: String,
    /// Original Lemon Squeezy licence key used to activate. Empty
    /// for trials. Needed for `/refresh` — the backend looks up the
    /// row by (licence_key, machine_hash).
    #[serde(default)]
    pub licence_key: String,
}

impl Licence {
    /// Whether the *record* claims a paid tier that has not expired.
    ///
    /// **Not the gate's answer.** This reads two editable fields of an
    /// ordinary file and trusts both, so a hand-written
    /// `{"tier":"pro","expires_at":null,"token":""}` returns `true`. It
    /// survives as a record-shape predicate; entitlement is decided by
    /// [`crate::licence::reconnect::classify`], which also verifies the
    /// signature.
    pub fn is_entitled(&self) -> bool {
        matches!(self.tier, Tier::Pro | Tier::Trial) && !self.is_expired()
    }

    pub fn is_expired(&self) -> bool {
        match &self.expires_at {
            None => false, // unlimited
            Some(iso) => {
                // Lazy parse — we only need to compare to "now".
                // RFC3339 sorts lexically when in UTC Z form, but
                // we'd rather be explicit.
                match chrono::DateTime::parse_from_rfc3339(iso) {
                    Ok(dt) => dt < chrono::Utc::now(),
                    // Treat unparsable as expired so a corrupted
                    // cache fails closed (free tier).
                    Err(_) => true,
                }
            }
        }
    }
}

pub struct LicenceCache;

impl LicenceCache {
    pub fn path() -> PathBuf {
        crate::config::AppConfig::state_dir().join("licence.json")
    }

    pub fn load() -> Result<Option<Licence>> {
        let path = Self::path();
        Ok(Self::load_at(&path))
    }

    /// [`load`](Self::load) against an explicit path, collapsing the two
    /// failure modes to `None`. A caller that must tell "no file" from
    /// "unparsable file" — the reconnection classifier, which refuses both
    /// but reports them differently — reads the path itself.
    pub fn load_at(path: &Path) -> Option<Licence> {
        if !path.exists() {
            return None;
        }
        let raw = fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    pub fn save(lic: &Licence) -> Result<()> {
        Self::save_at(&Self::path(), lic)
    }

    /// [`save`](Self::save) against an explicit path, written atomically:
    /// a sibling temp file is written and then renamed over the target, so
    /// an interruption leaves either the previous complete cache or the new
    /// one — never a truncated file that the next read has to guess about.
    pub fn save_at(path: &Path, lic: &Licence) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok();
        }
        let raw = serde_json::to_vec_pretty(lic)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, raw).with_context(|| format!("write {}", tmp.display()))?;
        // `rename` within a directory is atomic on every platform this
        // ships to; a failure here leaves the old file intact because the
        // temp file was never the target.
        fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))?;
        Ok(())
    }

    /// Remove the cache (deactivation / "log out" flow). Idempotent.
    pub fn clear() -> Result<()> {
        Self::clear_at(&Self::path())
    }

    /// [`clear`](Self::clear) against an explicit path.
    pub fn clear_at(path: &Path) -> Result<()> {
        if path.exists() {
            fs::remove_file(path).with_context(|| format!("rm {}", path.display()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_licence_is_never_expired() {
        let l = Licence {
            tier: Tier::Pro,
            machine_hash: "x".into(),
            expires_at: None,
            last_refreshed: chrono::Utc::now().to_rfc3339(),
            token: String::new(),
            licence_key: String::new(),
        };
        assert!(l.is_entitled());
        assert!(!l.is_expired());
    }

    #[test]
    fn past_expiry_is_not_entitled() {
        let l = Licence {
            tier: Tier::Trial,
            machine_hash: "x".into(),
            expires_at: Some("2000-01-01T00:00:00Z".into()),
            last_refreshed: chrono::Utc::now().to_rfc3339(),
            token: String::new(),
            licence_key: String::new(),
        };
        assert!(!l.is_entitled());
        assert!(l.is_expired());
    }

    #[test]
    fn free_tier_is_never_entitled() {
        let l = Licence {
            tier: Tier::Free,
            machine_hash: "x".into(),
            expires_at: None,
            last_refreshed: chrono::Utc::now().to_rfc3339(),
            token: String::new(),
            licence_key: String::new(),
        };
        assert!(!l.is_entitled());
    }
}
