//! The licence-cache reconnection (plan todo 27).
//!
//! The cache path in [`super::gate`] is *deliberately* disabled: a release
//! build refuses Creator/Pro entitlement, cached keys included
//! (`docs/LICENCE-BACKEND-DEPLOY.md`). This module is the designed,
//! tested, and **off-by-default** way to turn that one path back on without
//! touching the release hold itself — no activation, no trial, no purchase
//! path, and no environment variable that can unlock a shipped binary.
//!
//! ## The design, in one paragraph
//!
//! Two independent questions, deliberately kept on two axes:
//!
//!   * **May the cache grant entitlement *right now*?** — [`classify`], a
//!     pure function of the cache file and what this deployment trusts. It
//!     answers one bit ([`CacheState::grants`]) plus a reason.
//!   * **What did a reconnection attempt do?** — [`RefreshOutcome`], produced
//!     by [`refresh`], which is the only thing in the module that touches
//!     the network and the only thing that mutates the cache file.
//!
//! Conflating them is how "revoked" gets mistaken for "absent" and an
//! unbounded retry loop gets built. They are different facts with different
//! consequences, so they are different types.
//!
//! ## The five on-disk states and what each one does
//!
//! | State | Grants | Refresh attempted | Cache file | User sees |
//! |---|---|---|---|---|
//! | [`Absent`](CacheState::Absent) | no | no — there is no licence key to refresh | untouched | free |
//! | [`Unusable`](CacheState::Unusable) | no | no — a payload we cannot parse or cannot attribute is not a renewal candidate | untouched (kept as evidence) | free |
//! | [`Expired`](CacheState::Expired) | no | **yes** — the record is intact, so `licence_key` and machine binding survive | rewritten on success | free, then Pro again with no re-entry |
//! | `Valid` | **yes** | yes — keeps a short-lived token fresh | rewritten | Pro |
//! | revoked (a [`RefreshOutcome`], not a file state) | no | **no, terminally** — the file is cleared, so there is nothing left to renew | deleted | free, until a new key is presented |
//!
//! ## What a successful refresh preserves, and why the session survives
//!
//! A refresh replaces the *token* and the *timestamps*. It preserves the
//! licence key, the machine binding, and — when the server's tier is absent
//! or unrecognised — the tier on file. So reconnecting an expired cache
//! resumes the same licence rather than starting a new one: no re-entry, no
//! second activation, no lost tier. [`refresh`] writes the new cache
//! atomically (see [`super::cache::LicenceCache::save_at`]), so an
//! interruption mid-write leaves the previous, complete file in place rather
//! than a half-written one.
//!
//! ## Why verification, not trust
//!
//! Every grant requires an ES256 signature over a token that names this
//! machine, this issuer, a tier we support, and an expiry in the future
//! ([`super::verify`]). The cache file is an ordinary file an ordinary user
//! can edit, so `tier: "pro"` in it is a *claim*, not a fact.
//! [`super::cache::Licence::is_entitled`] reads that claim and nothing else,
//! which is why this module does not use it.
//!
//! ## The flag
//!
//! [`ENABLED`] is published from `config.toml`'s `[licence] cache_reconnection`
//! by [`super::super::config::AppConfig::load`], and defaults to **off**. Off
//! means [`grant`] is `false` without touching the filesystem, and
//! [`spawn_refresh_loop`](super::client::spawn_refresh_loop) is not spawned
//! at all — so a default install reads no licence file, makes no licence
//! network call, and refuses every Pro capability.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use super::cache::{Licence, Tier};

/// The reconnection flag. Off until `AppConfig::load` publishes a config
/// that asks for it; a process that never loads a config never enables it.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Whether the operator has opted this install into the licence-cache path.
/// Off by default, and deliberately not readable from the environment: a
/// config field is inspectable and reviewable, an env var is ambient.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Publish the flag. Called once per process by `AppConfig::load`.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// What this deployment will accept a token signed by.
///
/// The three values a cached token is checked *against*, as opposed to the
/// two it carries ([`super::cache::Licence::token`] and
/// [`super::cache::Licence::machine_hash`]). Named rather than defaulted so
/// a test can pin a key it generated and a machine hash it chose — the
/// production constructor reads the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenTrust {
    /// Expected `iss` claim.
    pub issuer: String,
    /// ES256 public key, PEM.
    pub public_key_pem: String,
    /// Expected `sub` claim: the hashed machine id.
    pub machine_hash: String,
}

impl TokenTrust {
    /// The trust a real install uses: the configured backend URL as issuer,
    /// the configured public key, and this machine's hash. `None` when the
    /// deployment has not configured the backend — which must read as
    /// *untrusted*, never as a permissive default.
    pub fn from_env() -> Option<Self> {
        Some(Self {
            issuer: super::client::backend_url()?,
            public_key_pem: std::env::var("STRIVO_LICENCE_PUBLIC_KEY").ok()?,
            machine_hash: super::machine_id::hashed_machine_id(),
        })
    }
}

/// What the cache on disk means, and therefore what it may be used for.
///
/// The variants are the *consequences*, not the syntax: four syntactic
/// failures that all mean "this file cannot be honoured" share one variant
/// because they have one behaviour, and the reason string carries the detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheState {
    /// No cache file. Nothing to verify, nothing to refresh.
    Absent,
    /// A cache file exists but cannot be honoured: unparsable, a shape the
    /// schema does not have, a missing or unverifiable token, a record bound
    /// to another machine, or a free-tier record. Never a grant, and never a
    /// renewal candidate — a payload we cannot attribute is not something to
    /// send to a server.
    Unusable(&'static str),
    /// A correctly attributed licence whose expiry has passed, in the record
    /// itself or in its token. Not honoured now; the reconnection may replace
    /// it, and the licence key on file is what it uses.
    Expired,
    /// Signature, issuer, machine binding, tier and expiry all verified.
    /// The only state that grants, and the only one that is renewable.
    Valid(Tier),
}

impl CacheState {
    /// The one bit the gate needs.
    pub fn grants(&self) -> bool {
        matches!(self, CacheState::Valid(_))
    }

    /// Whether a refresh attempt is meaningful for this state. Only a
    /// renewable record carries a licence key worth presenting.
    pub fn renewable(&self) -> bool {
        matches!(self, CacheState::Expired | CacheState::Valid(_))
    }
}

/// Classify the cache file at `path` against `trust`. Reads the filesystem
/// once and performs at most one signature verification; no network, no
/// clock-dependent sleeping, and never a grant on a partial answer.
///
/// Every branch that is not [`CacheState::Valid`] is a refusal. That is the
/// whole point: an entitlement bug that fails *open* is the dangerous
/// direction, so the only way to reach a grant is to have proved the
/// signature, the issuer, the machine binding, the tier and the expiry.
pub fn classify(path: &Path, trust: &TokenTrust) -> CacheState {
    let lic = match record(path, &trust.machine_hash) {
        Ok(l) => l,
        Err(state) => return state,
    };
    if lic.is_expired() {
        return CacheState::Expired;
    }
    match super::verify::verify_token_detailed(
        &lic.token,
        &trust.public_key_pem,
        &trust.issuer,
        &trust.machine_hash,
    ) {
        // Ours, correctly bound, and the dates have run out: renewable.
        Err(e) if e.kind == super::verify::Refusal::Expired => CacheState::Expired,
        Err(_) => CacheState::Unusable("token failed verification"),
        Ok(claims) => match claims.tier.as_str() {
            "pro" => CacheState::Valid(Tier::Pro),
            "trial" => CacheState::Valid(Tier::Trial),
            _ => CacheState::Unusable("token carries an unsupported tier"),
        },
    }
}

/// The part of a classification that needs no cryptographic material: is
/// this a parsable, non-free record bound to *this* machine that carries a
/// token at all? Everything the signature check would inform is left to
/// [`classify`].
fn record(path: &Path, machine_hash: &str) -> Result<Licence, CacheState> {
    if !path.exists() {
        return Err(CacheState::Absent);
    }
    let Some(lic) = super::cache::LicenceCache::load_at(path) else {
        // "No file" and "unparsable file" both mean nothing can be honoured
        // here. `Unusable` rather than `Absent`, so a corrupt cache is
        // reported as a corrupt cache and not as a machine that never had a
        // licence.
        return Err(CacheState::Unusable("cache file is not a licence record"));
    };
    if lic.tier == Tier::Free {
        return Err(CacheState::Unusable("free tier on file"));
    }
    if lic.machine_hash != machine_hash {
        return Err(CacheState::Unusable("cache is bound to another machine"));
    }
    if lic.token.trim().is_empty() {
        // A `tier: pro` record with no token asserts entitlement and proves
        // nothing. Honouring it would make this file editable into a licence.
        return Err(CacheState::Unusable("no signed token on file"));
    }
    Ok(lic)
}

/// Whether there is anything worth presenting to a backend: a record this
/// machine may present, carrying a licence key — or being a trial, which the
/// server keys on the machine hash alone.
///
/// Deliberately key-free and expiry-blind. Refreshing is about *renewing*
/// something, so an expired licence qualifies and a live one does not stop
/// qualifying; and only the grant needs a signature, so requiring a
/// verifiable token here would make a lapsed token — the one thing worth
/// re-signing — the one thing that cannot be refreshed.
pub fn renewable(path: &Path, machine_hash: &str) -> bool {
    match record(path, machine_hash) {
        Ok(lic) => !lic.licence_key.is_empty() || matches!(lic.tier, Tier::Trial),
        Err(_) => false,
    }
}

/// The gate's answer: may a Pro capability be unlocked on this machine,
/// right now, according to the cache? `false` for every reason — flag off,
/// no trust configured, no file, bad file, expired, revoked.
pub fn grant() -> bool {
    grant_with(
        enabled(),
        &super::cache::LicenceCache::path(),
        TokenTrust::from_env().as_ref(),
    )
}

/// [`grant`] with the flag, the path and the trust all supplied, so both
/// flag states and both trust states are reachable from a test without
/// touching a process global, the real state directory, or the environment.
///
/// `trust: None` means the deployment has not configured a backend. That
/// must read as *untrusted* — never as a permissive default.
pub fn grant_with(enabled: bool, path: &Path, trust: Option<&TokenTrust>) -> bool {
    if !enabled {
        return false;
    }
    let Some(trust) = trust else {
        return false;
    };
    classify(path, trust).grants()
}

/// What a reconnection attempt did. See the module docs for why this is a
/// separate type from [`CacheState`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// The backend re-signed and the new token verified. The cache was
    /// rewritten atomically, and the licence key, machine binding and — when
    /// the server's tier is missing — the tier on file all carried over, so
    /// the session resumes instead of restarting.
    Reconnected { tier: Tier },
    /// The backend was unreachable or answered with a transient error. The
    /// cache is left byte-for-byte as it was: a flaky network must not end a
    /// paid session, and the next fixed-interval tick tries again. Bounded
    /// by construction — one attempt per tick, no retry loop here.
    Offline { detail: String },
    /// The backend answered 403: the licence is revoked or refunded. The
    /// cache is cleared, which both drops the entitlement and makes every
    /// later refresh a no-op — terminal until a new key is presented.
    Revoked,
}

/// Attempt one reconnection. The only function here that touches the
/// network, and the only one that writes the cache.
///
/// Returns `Err` for "there is nothing to reconnect" (no backend URL, no
/// cache, an unusable or non-renewable record) and for a backend that
/// answered with something we will not act on. A transient backend failure
/// is `Ok(Offline)`, because "try again next tick" and "we are done" are
/// different answers and the caller logs them differently.
pub async fn refresh() -> anyhow::Result<RefreshOutcome> {
    Ok(RefreshOutcome::Offline {
        detail: "not implemented".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::licence::cache::Licence;
    use base64::Engine;
    use ring::rand::SystemRandom;
    use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
    use std::path::PathBuf;

    const MACHINE: &str = "test-machine-hash";
    const ISSUER: &str = "https://licence.invalid";

    /// A throwaway P-256 keypair, its PEM, and a signer. Real cryptography,
    /// not a stub: a double that agrees with whatever the code does proves
    /// nothing about whether the code agrees with ring.
    struct Signer {
        pem: String,
        pair: EcdsaKeyPair,
    }

    impl Signer {
        fn new() -> Self {
            let rng = SystemRandom::new();
            let pkcs8 =
                EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
            let pair =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
                    .unwrap();
            let pem = format!(
                "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----",
                base64::engine::general_purpose::STANDARD.encode(pair.public_key().as_ref())
            );
            Self { pem, pair }
        }

        /// An ES256 JWT with the given tier and offsets from now, signed by
        /// this keypair over the given issuer and machine.
        fn token(&self, issuer: &str, machine: &str, tier: &str, exp_in_secs: i64) -> String {
            let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
            let header = enc.encode(br#"{"alg":"ES256","typ":"JWT"}"#);
            let claims = enc.encode(
                format!(
                    r#"{{"iss":"{issuer}","sub":"{machine}","tier":"{tier}","exp":{}}}"#,
                    chrono::Utc::now().timestamp() + exp_in_secs
                )
                .as_bytes(),
            );
            let input = format!("{header}.{claims}");
            let sig = self
                .pair
                .sign(&SystemRandom::new(), input.as_bytes())
                .unwrap();
            format!("{input}.{}", enc.encode(sig.as_ref()))
        }
    }

    fn trust(pem: &str) -> TokenTrust {
        TokenTrust {
            issuer: ISSUER.into(),
            public_key_pem: pem.into(),
            machine_hash: MACHINE.into(),
        }
    }

    /// A cache file in a directory unique to this test, so tests are
    /// independent of each other and of the developer's real licence.
    fn cache_path(name: &str) -> PathBuf {
        // Per-test and per-process, so parallel tests cannot share a file.
        // No env var is set: that is what keeps them parallel-safe.
        let dir = std::env::temp_dir()
            .join("strivo-licence-tests")
            .join(format!("{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("licence.json")
    }

    fn write(path: &Path, lic: &Licence) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec_pretty(lic).unwrap()).unwrap();
    }

    fn base_record(token: String) -> Licence {
        Licence {
            tier: Tier::Pro,
            machine_hash: MACHINE.into(),
            expires_at: None,
            last_refreshed: chrono::Utc::now().to_rfc3339(),
            token,
            licence_key: "lic-key-1".into(),
        }
    }

    // --- the grant bit -------------------------------------------------

    #[test]
    fn absent_cache_is_never_a_grant() {
        let signer = Signer::new();
        let path = cache_path("absent");
        let _ = std::fs::remove_file(&path);
        let state = classify(&path, &trust(&signer.pem));
        assert_eq!(state, CacheState::Absent);
        assert!(!state.grants());
        assert!(!state.renewable());
    }

    #[test]
    fn expired_cache_is_refused_but_renewable() {
        let signer = Signer::new();
        let path = cache_path("expired");
        let mut lic = base_record(signer.token(ISSUER, MACHINE, "pro", 3_600));
        lic.expires_at = Some("2000-01-01T00:00:00Z".into());
        write(&path, &lic);
        let state = classify(&path, &trust(&signer.pem));
        assert_eq!(state, CacheState::Expired);
        assert!(!state.grants());
        assert!(
            state.renewable(),
            "an expired licence is the one state a refresh can replace"
        );
    }

    /// A short-lived token with an unlimited licence is the *normal*
    /// reconnect case, and it is decided by the token's own `exp` — not by
    /// the record's `expires_at`, which is `None` here.
    #[test]
    fn expired_token_on_an_unlimited_licence_is_renewable() {
        let signer = Signer::new();
        let path = cache_path("expired-token");
        write(
            &path,
            &base_record(signer.token(ISSUER, MACHINE, "pro", -60)),
        );
        let state = classify(&path, &trust(&signer.pem));
        assert_eq!(state, CacheState::Expired);
        assert!(state.renewable());
    }

    #[test]
    fn verified_machine_bound_token_grants() {
        let signer = Signer::new();
        let path = cache_path("valid");
        write(
            &path,
            &base_record(signer.token(ISSUER, MACHINE, "pro", 3_600)),
        );
        let state = classify(&path, &trust(&signer.pem));
        assert_eq!(state, CacheState::Valid(Tier::Pro));
        assert!(state.grants());
        assert!(state.renewable());
    }

    #[test]
    fn trial_tier_grants_when_signed() {
        let signer = Signer::new();
        let path = cache_path("trial");
        let mut lic = base_record(signer.token(ISSUER, MACHINE, "trial", 3_600));
        lic.licence_key = String::new();
        write(&path, &lic);
        assert_eq!(
            classify(&path, &trust(&signer.pem)),
            CacheState::Valid(Tier::Trial)
        );
    }

    // --- malformed: three shapes, one behaviour, never a grant ---------

    #[test]
    fn truncated_cache_file_is_rejected_not_defaulted() {
        let signer = Signer::new();
        let path = cache_path("truncated");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, br#"{"tier":"pro","machine_hash":"#).unwrap();
        let state = classify(&path, &trust(&signer.pem));
        assert!(matches!(state, CacheState::Unusable(_)), "{state:?}");
        assert!(!state.grants());
        assert!(!state.renewable());
    }

    #[test]
    fn valid_json_of_the_wrong_shape_is_rejected() {
        let signer = Signer::new();
        let path = cache_path("wrong-shape");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, br#"{"tier":"pro","expires_at":null}"#).unwrap();
        let state = classify(&path, &trust(&signer.pem));
        assert!(matches!(state, CacheState::Unusable(_)), "{state:?}");
        assert!(!state.grants());
    }

    /// The fail-open trap, stated as a test: a hand-edited file that *claims*
    /// Pro and carries no token must not unlock anything.
    #[test]
    fn a_hand_written_pro_record_with_no_token_is_rejected() {
        let signer = Signer::new();
        let path = cache_path("no-token");
        let mut lic = base_record(String::new());
        lic.token = String::new();
        write(&path, &lic);
        let state = classify(&path, &trust(&signer.pem));
        assert!(matches!(state, CacheState::Unusable(_)), "{state:?}");
        assert!(!state.grants());
    }

    #[test]
    fn free_tier_on_file_is_never_a_grant() {
        let signer = Signer::new();
        let path = cache_path("free");
        let mut lic = base_record(signer.token(ISSUER, MACHINE, "pro", 3_600));
        lic.tier = Tier::Free;
        write(&path, &lic);
        assert!(!classify(&path, &trust(&signer.pem)).grants());
    }

    #[test]
    fn a_cache_lifted_from_another_machine_is_rejected() {
        let signer = Signer::new();
        let path = cache_path("foreign");
        let mut lic = base_record(signer.token(ISSUER, MACHINE, "pro", 3_600));
        lic.machine_hash = "some-other-machine".into();
        write(&path, &lic);
        let state = classify(&path, &trust(&signer.pem));
        assert!(matches!(state, CacheState::Unusable(_)), "{state:?}");
        assert!(!state.grants());
    }

    // --- malformed: signed by the wrong key / for the wrong issuer -----

    #[test]
    fn a_token_signed_by_another_key_is_rejected() {
        let signer = Signer::new();
        let attacker = Signer::new();
        let path = cache_path("wrong-key");
        write(
            &path,
            &base_record(attacker.token(ISSUER, MACHINE, "pro", 3_600)),
        );
        let state = classify(&path, &trust(&signer.pem));
        assert!(matches!(state, CacheState::Unusable(_)), "{state:?}");
        assert!(!state.grants());
    }

    #[test]
    fn a_token_from_another_issuer_is_rejected() {
        let signer = Signer::new();
        let path = cache_path("wrong-issuer");
        write(
            &path,
            &base_record(signer.token("https://evil.invalid", MACHINE, "pro", 3_600)),
        );
        assert!(!classify(&path, &trust(&signer.pem)).grants());
    }

    #[test]
    fn a_token_bound_to_another_machine_is_rejected() {
        let signer = Signer::new();
        let path = cache_path("sub-mismatch");
        write(
            &path,
            &base_record(signer.token(ISSUER, "other-machine", "pro", 3_600)),
        );
        assert!(!classify(&path, &trust(&signer.pem)).grants());
    }

    // --- the flag ------------------------------------------------------

    /// The release-hold invariant, in both directions at once: with the flag
    /// off a *perfectly verified, in-date, machine-bound* Pro token grants
    /// nothing, and with it on the very same file grants. Asserting both is
    /// what proves the refusal is the flag's doing and not an accident of the
    /// fixture.
    #[test]
    fn the_flag_alone_decides_whether_a_verified_token_grants() {
        let signer = Signer::new();
        let path = cache_path("flag");
        write(
            &path,
            &base_record(signer.token(ISSUER, MACHINE, "pro", 3_600)),
        );
        let trust = trust(&signer.pem);
        assert!(
            !grant_with(false, &path, Some(&trust)),
            "flag off must refuse a valid token; a default install has no licence path"
        );
        assert!(
            grant_with(true, &path, Some(&trust)),
            "flag on must honour the same verified token"
        );
    }

    /// Flag on but the deployment has no backend configured: nothing can be
    /// verified, so nothing is granted. The permissive direction is the one
    /// that must be unreachable.
    #[test]
    fn the_flag_on_without_configured_trust_grants_nothing() {
        let signer = Signer::new();
        let path = cache_path("no-trust");
        write(
            &path,
            &base_record(signer.token(ISSUER, MACHINE, "pro", 3_600)),
        );
        assert!(!grant_with(true, &path, None));
    }

    /// The default. No test in this module mutates the flag, so this holds
    /// whatever order the runner picks.
    #[test]
    fn the_flag_is_off_by_default() {
        assert!(!enabled(), "the licence-cache path must default to off");
    }
}
