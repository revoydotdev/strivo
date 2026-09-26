//! The entitlement set is derived, and the derivation is enforced (todo 27).
//!
//! The set core's gate is consulted with is not written down anywhere. It is
//! `CAPABILITIES.iter().filter(|c| c.entitlement == Entitlement::Pro)` —
//! `capabilities::pro_slugs()`. This file is the proof that the derivation
//! still holds, in the two configurations that matter, and it is
//! deliberately an *integration* test in the `strivo-web` test binary: the
//! subject is the boundary between the descriptor and core's gate, so
//! asserting either side alone would assert the wrong thing.
//!
//! ## What is checked, and what is deliberately not
//!
//! Checked here: that the set the gate uses is the descriptor's `entitlement`
//! field, that it is **enforced** (every Pro capability is refused with no
//! licence, every Included one allowed), that the per-capability entry point
//! and the free function agree, and that the gate call sites are non-empty so
//! none of the above can pass by scanning nothing.
//!
//! Not checked here: the *shape* of a `gate_pro(...)` argument. That is
//! `scripts/check-capability-descriptor.sh`'s job — it is an ast-grep search,
//! and it must be, because "is this a string literal or an enum path" is not a
//! question a text scan can answer. Duplicating it here in weaker form would
//! be worse than leaving it to the script.
//!
//! ## The non-vacuity property
//!
//! An empty Pro set makes every check below pass: nothing to refuse, and
//! "every Pro capability is refused" is true of the empty set. That is the
//! fail-open direction — an entitlement bug that grants is the dangerous one —
//! so the Creator half asserts the set is non-empty *before* asserting
//! anything about its members, and the scan asserts it found call sites.

use strivo_core::licence::gate;
use strivo_web::capabilities::{pro_slugs, CapabilityKey, Entitlement, CAPABILITIES};

/// The licence-cache reconnection is off unless a config asks for it, and no
/// test in this binary turns it on, so a Pro capability must be refused here.
/// A developer's exported `STRIVO_DEV_UNLOCK_ALL` would otherwise make this
/// binary's answer depend on their shell.
fn without_dev_unlock<T>(f: impl FnOnce() -> T) -> T {
    std::env::remove_var("STRIVO_DEV_UNLOCK_ALL");
    f()
}

/// The gate call sites, read from the source. Counted rather than trusted: a
/// refactor that emptied them would make the subset assertions below vacuous,
/// which is precisely the failure this file exists to make impossible.
#[cfg(feature = "creator")]
fn gated_keys() -> Vec<CapabilityKey> {
    // Variant name -> key, derived from the enum itself: `Debug` gives the
    // variant spelling, `as_str` the wire one. A variant missing from this
    // map makes the scan panic below, so a rename cannot silently stop being
    // checked.
    let by_variant: std::collections::HashMap<String, CapabilityKey> = CapabilityKey::all()
        .map(|k| (format!("{k:?}"), k))
        .collect();

    let src = include_str!("../src/routes/plugins.rs");
    let mut keys = Vec::new();
    let mut rest = src;
    while let Some(at) = rest.find("gate_pro(CapabilityKey::") {
        rest = &rest[at + "gate_pro(CapabilityKey::".len()..];
        let variant: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        let key = *by_variant
            .get(&variant)
            .unwrap_or_else(|| panic!("gate site names an undeclared capability {variant:?}"));
        keys.push(key);
    }
    keys
}

/// The subset invariant, with the scan pinned so it cannot report zero.
///
/// Creator-only: `routes::plugins` is `#[cfg(feature = "creator")]`, so in a
/// PVR build this module is not compiled and its *text* is not part of that
/// binary. Scanning it anyway would assert a relationship between a build
/// artefact and code the build does not contain. The PVR half of the contract
/// is `a_pvr_build_has_nothing_to_gate_and_refuses_nothing` below.
#[cfg(feature = "creator")]
#[test]
fn every_gate_call_site_names_a_capability_the_descriptor_carries() {
    let gated = gated_keys();
    assert!(
        gated.len() > 50,
        "only {} gate sites parsed; the scan is broken, not passing",
        gated.len()
    );
    // Every gated capability has a descriptor row — the enum is the anchor,
    // so this holds however the call sites are written.
    for key in &gated {
        assert_eq!(key.descriptor().key, *key);
    }
    // And they are a *subset* of the descriptor, not the whole of it: the
    // descriptor also carries capabilities with no gate at all.
    let gated_names: std::collections::BTreeSet<&str> = gated.iter().map(|k| k.as_str()).collect();
    let described: std::collections::BTreeSet<&str> =
        CAPABILITIES.iter().map(|c| c.key.as_str()).collect();
    assert!(
        gated_names.is_subset(&described),
        "gated but not described: {:?}",
        gated_names.difference(&described).collect::<Vec<_>>()
    );
    assert!(
        described.difference(&gated_names).count() > 0,
        "equality would make a capability with no gate unrepresentable"
    );
}

/// The derivation, stated as an assertion: the set is the `entitlement`
/// field, filtered to `Pro`, mapped to canonical names. Recomputed here from
/// the same table rather than compared to a literal, so this cannot become a
/// second list.
#[test]
fn the_entitlement_set_is_the_descriptors_pro_column() {
    let derived: Vec<&str> = CAPABILITIES
        .iter()
        .filter(|c| c.entitlement == Entitlement::Pro)
        .map(|c| c.key.as_str())
        .collect();
    assert_eq!(derived, pro_slugs());
    // `Included` is the complement — the two columns partition the table, so
    // no row can be silently in neither set.
    let included: Vec<&str> = CAPABILITIES
        .iter()
        .filter(|c| c.entitlement == Entitlement::Included)
        .map(|c| c.key.as_str())
        .collect();
    assert_eq!(derived.len() + included.len(), CAPABILITIES.len());
}

/// Enforcement, in the configuration where Pro exists.
///
/// Every Pro capability is refused with no licence, and every Included one is
/// allowed. Both directions matter: a Pro row that is not enforced is a
/// fail-open, and an Included row that is refused is a free feature broken by
/// an entitlement change.
#[cfg(feature = "creator")]
#[test]
fn a_creator_build_refuses_every_pro_capability_and_allows_every_included_one() {
    let pro = pro_slugs();
    assert!(
        !pro.is_empty(),
        "a Creator descriptor with no Pro row is vacuous"
    );
    let included: Vec<&str> = CAPABILITIES
        .iter()
        .filter(|c| c.entitlement == Entitlement::Included)
        .map(|c| c.key.as_str())
        .collect();
    assert!(
        !included.is_empty(),
        "every capability being Pro is as vacuous as none"
    );

    without_dev_unlock(|| {
        for name in &pro {
            assert!(
                !gate::is_entitled(name, &pro),
                "{name} is Pro and was allowed without a licence"
            );
            assert!(
                !gate::entitled(),
                "a Creator build with no licence must not report itself entitled"
            );
        }
        for name in &included {
            assert!(
                gate::is_entitled(name, &pro),
                "{name} is Included and was refused"
            );
        }
    });

    // The per-capability entry point every route calls must reach the same
    // verdict, so a 402 can never contradict the matrix or the upgrade card.
    for key in CapabilityKey::all() {
        assert_eq!(
            key.is_entitled(),
            gate::is_entitled(key.as_str(), &pro),
            "{} disagrees with the gate",
            key.as_str()
        );
    }
}

/// The PVR configuration of the same contract. Here the derived set *is*
/// empty, which is correct: the PVR edition ships no Creator capability, so
/// there is nothing Pro to gate and nothing to refuse. Asserted explicitly so
/// that a future PVR capability cannot be added without a decision about it —
/// a PVR row that nobody gates would be the same silent fail-open in the
/// opposite direction.
#[cfg(not(feature = "creator"))]
#[test]
fn a_pvr_build_has_nothing_to_gate_and_refuses_nothing() {
    assert!(
        pro_slugs().is_empty(),
        "a PVR build must report no Pro capability"
    );
    assert!(CAPABILITIES.is_empty());
    without_dev_unlock(|| {
        for key in CapabilityKey::all() {
            assert!(
                gate::is_entitled(key.as_str(), &[]),
                "{} must not be gated in a PVR build",
                key.as_str()
            );
            assert!(key.is_entitled());
        }
        assert!(!gate::entitled(), "a PVR build is never a paid install");
    });
}
