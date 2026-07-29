//! Entitlement gate — the single read path for "should this Pro
//! feature unlock?". Everything that gates on a paid plugin calls
//! this. Lives outside the plugin loader so the same function can
//! gate UI surfaces (the upgrade card) and runtime loads alike.
//!
//! Decision order:
//!
//!   1. **Dev override** — `STRIVO_DEV_UNLOCK_ALL=1` short-circuits to
//!      entitled. Mirrors the env the licence-status route already
//!      reads so the SPA and the gate agree.
//!   2. **Cache** — read `~/.local/share/strivo/licence.json`. If
//!      present, entitled, AND bound to this machine (hash match),
//!      return true. Cache survives reboots and offline use — the
//!      72h refresh is enforced server-side at the next contact, not
//!      by killing entitlement client-side.
//!   3. **Default** — free tier, no Pro features.
//!
//! The set of "Pro" plugin names is hard-coded for now — it's a tiny
//! list and won't churn. When we ship a third-party plugin SDK
//! (post-1.0) this becomes a manifest lookup.

use super::cache::{Licence, LicenceCache};
use super::machine_id::hashed_machine_id;

/// First-party Pro plugins. Anything not in this list is treated as
/// free and ungated.
pub const PRO_PLUGINS: &[&str] = &["crunchr", "archiver", "viewguard", "insights"];

pub fn is_pro_plugin(name: &str) -> bool {
    PRO_PLUGINS.iter().any(|p| p.eq_ignore_ascii_case(name))
}

/// Returns true if `plugin` should be allowed to load / be exposed in
/// the UI. Free plugins are always allowed; Pro plugins require a
/// valid licence cache or the dev override.
pub fn is_entitled(plugin: &str) -> bool {
    if !is_pro_plugin(plugin) {
        return true;
    }
    if dev_unlock() {
        return true;
    }
    entitled_from_cache()
}

/// Whole-app entitlement (used by the upgrade card, the licence
/// status route, etc.) — true iff *any* Pro feature is unlocked on
/// this machine right now.
pub fn entitled() -> bool {
    dev_unlock() || entitled_from_cache()
}

fn dev_unlock() -> bool {
    std::env::var("STRIVO_DEV_UNLOCK_ALL")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn entitled_from_cache() -> bool {
    match LicenceCache::load() {
        Ok(Some(lic)) => bound_and_active(&lic),
        _ => false,
    }
}

fn bound_and_active(lic: &Licence) -> bool {
    if !lic.is_entitled() {
        return false;
    }
    // A cache lifted off another machine carries that machine's hash.
    // Refuse it. This is a soft guard — the activation server is the
    // hard one, signing tokens per machine_hash — but it stops the
    // accidental copy-the-file case immediately.
    lic.machine_hash == hashed_machine_id() && super::verify::verify_token(&lic.token).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_plugins_always_allowed() {
        assert!(is_entitled("some-third-party"));
        assert!(is_entitled(""));
    }

    #[test]
    fn pro_plugin_list_matches_first_party_set() {
        assert!(is_pro_plugin("crunchr"));
        assert!(is_pro_plugin("CRUNCHR"));
        assert!(is_pro_plugin("archiver"));
        assert!(is_pro_plugin("viewguard"));
        assert!(is_pro_plugin("insights"));
        assert!(!is_pro_plugin("something-else"));
    }

    #[test]
    fn dev_unlock_env_grants_entitlement() {
        // SAFETY: this test only sets the env if not already set;
        // `STRIVO_DEV_UNLOCK_ALL=1` is the documented unlock path.
        std::env::set_var("STRIVO_DEV_UNLOCK_ALL", "1");
        assert!(is_entitled("crunchr"));
        assert!(entitled());
        std::env::remove_var("STRIVO_DEV_UNLOCK_ALL");
    }
}
