//! Entitlement gate — the single read path for "should this Pro
//! feature unlock?". Everything that gates on a paid plugin calls
//! this. Lives outside the plugin loader so the same function can
//! gate UI surfaces (the upgrade card) and runtime loads alike.
//!
//! Decision order:
//!
//!   1. **Not a Pro capability** — always allowed.
//!   2. **Developer override** — `STRIVO_DEV_UNLOCK_ALL=1` unlocks every
//!      Pro capability in a debug build, or in a release build compiled
//!      with the `dev-unlock` feature, which no published build enables.
//!   3. **A verified, in-date licence cache** — only when the operator has
//!      opted in with `[licence] cache_reconnection`, which is off by
//!      default. See [`crate::licence::reconnect`]; the entitlement hold
//!      itself is unchanged, and a default install takes this branch to a
//!      `false` without reading a file.
//!
//! The set of "Pro" plugin names is not core's to know — core has no
//! notion of what a plugin *is*, let alone which ones cost money. Both
//! [`is_pro_plugin`] and [`is_entitled`] take that set as a parameter:
//! the caller that composes plugins into the running app (the Creator
//! Edition web server) supplies it, derived from the capability descriptor
//! by `capabilities::pro_slugs()`; the PVR edition passes an empty set (or
//! never calls in on a code path it doesn't compile), so nothing is
//! Pro-gated there. See ADR 0001 (CE06): entitlement is a Creator Edition
//! product concept, and core's gate stays generic over *which* plugins that
//! means without naming any of them. There is no second list of Pro names
//! anywhere in the tree, and adding one is the drift todo 20 removed.

pub fn is_pro_plugin(name: &str, pro_plugins: &[&str]) -> bool {
    pro_plugins.iter().any(|p| p.eq_ignore_ascii_case(name))
}

/// Returns true if `plugin` should be allowed to load / be exposed in
/// the UI. Free plugins (not in `pro_plugins`) are always allowed; Pro
/// plugins require the dev override or a verified licence cache.
pub fn is_entitled(plugin: &str, pro_plugins: &[&str]) -> bool {
    if !is_pro_plugin(plugin, pro_plugins) {
        return true;
    }
    if dev_unlock() {
        return true;
    }
    super::reconnect::grant()
}

/// Whole-app entitlement (used by the upgrade card, the licence
/// status route, etc.) — true iff *any* Pro feature is unlocked on
/// this machine right now.
pub fn entitled() -> bool {
    if dev_unlock() {
        return true;
    }
    // Same answer the per-capability gate reaches, so the upgrade card and
    // the 402 can never disagree about whether this install is paid.
    super::reconnect::grant()
}

/// The developer override: `STRIVO_DEV_UNLOCK_ALL=1` in a debug build or a
/// `dev-unlock` build. Also decides whether the web UI exposes the Creator
/// surface, so the override unlocks the UI and the gate together.
pub fn dev_unlock() -> bool {
    (cfg!(debug_assertions) || cfg!(feature = "dev-unlock"))
        && std::env::var("STRIVO_DEV_UNLOCK_ALL")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PRO_PLUGINS: &[&str] = &["test-plugin-a", "test-plugin-b"];

    #[test]
    fn free_plugins_always_allowed() {
        assert!(is_entitled("some-third-party", TEST_PRO_PLUGINS));
        assert!(is_entitled("", TEST_PRO_PLUGINS));
    }

    #[test]
    fn pro_plugin_list_matches_caller_supplied_set() {
        assert!(is_pro_plugin("test-plugin-a", TEST_PRO_PLUGINS));
        assert!(is_pro_plugin("TEST-PLUGIN-A", TEST_PRO_PLUGINS));
        assert!(is_pro_plugin("test-plugin-b", TEST_PRO_PLUGINS));
        assert!(!is_pro_plugin("something-else", TEST_PRO_PLUGINS));
        assert!(!is_pro_plugin("test-plugin-a", &[]));
    }

    #[test]
    fn dev_unlock_env_grants_entitlement_only_in_debug_or_dev_unlock_builds() {
        // The override is debug-only by construction (see `dev_unlock`):
        // a release binary must never unlock Pro features from an
        // environment variable, which is the property "hold creator
        // edition release paths" established.
        //
        // Assert against `cfg!(debug_assertions)` rather than
        // `#[cfg(debug_assertions)]`-gating the whole test, so the
        // release half — the half that actually matters for shipping —
        // is covered instead of compiled away. CI runs the suite with
        // `--release`, where an unconditional assert here fails.
        // The `dev-unlock` feature is the one sanctioned exception, and no
        // published build enables it.
        let unlockable = cfg!(debug_assertions) || cfg!(feature = "dev-unlock");
        std::env::set_var("STRIVO_DEV_UNLOCK_ALL", "1");
        assert_eq!(
            is_entitled("test-plugin-a", TEST_PRO_PLUGINS),
            unlockable,
            "dev unlock must grant entitlement in debug or dev-unlock builds and never in a published release"
        );
        assert_eq!(entitled(), unlockable);
        std::env::remove_var("STRIVO_DEV_UNLOCK_ALL");

        // Absent the variable, nothing is entitled in either profile.
        assert!(!is_entitled("test-plugin-a", TEST_PRO_PLUGINS));
        assert!(!entitled());
    }
}
