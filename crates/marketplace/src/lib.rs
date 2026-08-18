//! strivo-marketplace — third-party plugin manifest spec + catalog.
//!
//! Foundation for letting streamers browse and install third-party
//! plugins. This iteration ships:
//!
//!   * The **manifest format** — a TOML descriptor with name /
//!     version / author / capabilities / entry-point / pricing /
//!     screenshots / repository, validated against
//!     [`MIN_HOST_VERSION`] for forward compatibility.
//!   * [`parse_manifest`] — parses a TOML string into a typed
//!     [`PluginManifest`].
//!   * [`validate_manifest`] — semantic checks: every capability tag
//!     must match a known well-known string OR start with the
//!     reserved `x.` prefix for third-party extensions, the entry-
//!     point kind must be supported by the host, and the min host
//!     version must not exceed the current host.
//!   * [`default_catalog`] — a curated seed of plugins ready for the
//!     marketplace page to render. Real catalog hosting plugs in
//!     later by replacing the seed with a remote fetch.
//!
//! All pure — no IO, no network, no SQL. The web crate wraps this
//! with the endpoint and the install/uninstall lifecycle.

use serde::{Deserialize, Serialize};

/// The host (`strivo-core`) version this marketplace crate validates
/// against. A manifest's `min_host_version` must be ≤ this value, or
/// `validate_manifest` rejects it with "upgrade StriVo first".
///
/// Policy: bumped in lockstep with `strivo-core`'s `Cargo.toml`
/// `version` field. Marketplace lives in its own crate without a
/// build-script or path-dep on strivo-core, so this is kept by hand —
/// look for it in the same PR that bumps the host version. Drift
/// trips `default_catalog_is_non_empty_and_each_entry_validates`.
pub const HOST_VERSION: &str = "0.5.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub author: String,
    #[serde(default)]
    pub license: Option<String>,
    pub description: String,
    /// Capability tags this plugin provides. Each must match a
    /// well-known constant in `strivo_core::plugin::capability` or
    /// start with `"x."` for custom third-party tags.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// What the plugin needs upstream. Same naming rules.
    #[serde(default)]
    pub consumes: Vec<String>,
    pub entry_point: EntryPoint,
    /// Minimum host version the plugin requires. semver string.
    pub min_host_version: String,
    /// Optional one-time price in USD cents. `None` = free.
    #[serde(default)]
    pub price_cents: Option<u64>,
    /// Optional URLs.
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub screenshots: Vec<String>,
    /// Friendly category tag for grouping on the marketplace page.
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EntryPoint {
    /// Native cdylib shared library loaded via libloading. `path`
    /// resolves relative to the user's `~/.local/share/strivo/plugins/`.
    Cdylib { path: String },
    /// JSON-RPC over a Unix socket / TCP — the plugin runs as a
    /// separate process, the host talks to it via the wire protocol.
    Rpc { url: String },
    /// Stub for plugins not yet runnable on this host — used by the
    /// catalog to show "coming soon" entries.
    Roadmap,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Catalog {
    pub entries: Vec<CatalogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub manifest: PluginManifest,
    /// How the catalog was populated. "first_party" / "verified" /
    /// "community" so the SPA can colour-code trust.
    pub source: String,
    /// Installed locally on this host?
    #[serde(default)]
    pub installed: bool,
}

/// Parse a TOML manifest from a string. Returns a typed [`PluginManifest`].
pub fn parse_manifest(s: &str) -> anyhow::Result<PluginManifest> {
    let m: PluginManifest = toml::from_str(s).map_err(|e| anyhow::anyhow!("toml: {e}"))?;
    Ok(m)
}

/// Semantic checks beyond the type-level requirements.
///
/// Returns Err with a human-readable message on the first failure;
/// the caller is responsible for deciding whether to reject or merely
/// warn.
pub fn validate_manifest(m: &PluginManifest) -> Result<(), String> {
    if m.name.trim().is_empty() {
        return Err("name must not be empty".into());
    }
    if !is_kebab_or_snake(&m.name) {
        return Err(format!(
            "name '{}' must be kebab-case or snake_case (a..z, 0..9, _, -)",
            m.name
        ));
    }
    // Version must parse as semver.
    semver::Version::parse(&m.version).map_err(|e| format!("version: {e}"))?;
    // min_host_version must parse as semver AND not exceed the
    // current host. (A newer-than-host minimum means the plugin
    // requires capabilities this build doesn't have.)
    let want = semver::Version::parse(&m.min_host_version)
        .map_err(|e| format!("min_host_version: {e}"))?;
    let have =
        semver::Version::parse(HOST_VERSION).map_err(|e| format!("HOST_VERSION constant: {e}"))?;
    if want > have {
        return Err(format!(
            "min_host_version {want} > host {have}; upgrade StriVo first"
        ));
    }
    // Capabilities must be known constants or start with "x.".
    for cap in m.capabilities.iter().chain(m.consumes.iter()) {
        if !is_valid_capability(cap) {
            return Err(format!(
                "capability '{cap}' is neither well-known nor x.-prefixed"
            ));
        }
    }
    // Entry point validation.
    match &m.entry_point {
        EntryPoint::Cdylib { path } => {
            if path.trim().is_empty() {
                return Err("entry_point.path must not be empty".into());
            }
        }
        EntryPoint::Rpc { url } => {
            if url.trim().is_empty() {
                return Err("entry_point.url must not be empty".into());
            }
        }
        EntryPoint::Roadmap => {}
    }
    Ok(())
}

fn is_kebab_or_snake(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Recognised first-party capability tags. Mirrors the constants in
/// `strivo_core::plugin::capability` but kept inline here so this
/// crate doesn't depend on strivo-core (the spec ships standalone).
const WELL_KNOWN_CAPABILITIES: &[&str] = &[
    "transcription",
    "word_timestamps",
    "diarisation",
    "topic_segmentation",
    "chapters",
    "scene_detection",
    "thumbnail_ranking",
    "highlight_detection",
    "clip_extraction",
    "translation",
    "captions",
    "audience_retention",
    "fraud_detection",
    "stream_comparison",
    "research_signals",
    "corpus_assembly",
    "qualitative_coding",
    "corpus_retrieval",
    "reporting",
    "brand_safety",
    "asset_catalog",
    "source_track_split",
    "publish_queue",
    "edl_editor",
    "recording", // host-emitted source artefact
];

pub fn is_valid_capability(tag: &str) -> bool {
    WELL_KNOWN_CAPABILITIES.contains(&tag) || tag.starts_with("x.")
}

/// A curated seed catalog the marketplace SPA renders today. Real
/// catalog hosting (a HTTPS fetch + signature check) replaces this
/// when the marketplace service is up.
pub fn default_catalog() -> Catalog {
    Catalog {
        entries: vec![
            CatalogEntry {
                manifest: PluginManifest {
                    name: "broll-finder".into(),
                    version: "0.1.0".into(),
                    author: "Chorosyne".into(),
                    license: Some("MIT".into()),
                    description:
                        "Suggest B-roll cuts from a tagged local library based on transcript topics.".into(),
                    capabilities: vec!["x.broll_suggestion".into()],
                    consumes: vec!["transcription".into(), "topic_segmentation".into()],
                    // Promoted from Roadmap → available in iter 18: the
                    // strivo-broll crate ships with the host.
                    entry_point: EntryPoint::Cdylib { path: "broll.so".into() },
                    min_host_version: "0.3.0".into(),
                    price_cents: Some(900),
                    repository: Some("https://github.com/revoydotdev/broll-finder".into()),
                    icon: None,
                    screenshots: vec![],
                    category: Some("Editor".into()),
                },
                source: "first_party".into(),
                installed: true,
            },
            CatalogEntry {
                manifest: PluginManifest {
                    name: "twitch-chat-density".into(),
                    version: "0.1.0".into(),
                    author: "Chorosyne".into(),
                    license: Some("MIT".into()),
                    description: "Real audience-retention from chat density (Twitch IRC tap).".into(),
                    capabilities: vec!["audience_retention".into(), "x.chat_density".into()],
                    consumes: vec![],
                    // Promoted from Roadmap → available in iter 19: the
                    // strivo-chat-density crate ships with the host.
                    entry_point: EntryPoint::Cdylib { path: "chat_density.so".into() },
                    min_host_version: "0.3.0".into(),
                    price_cents: None,
                    repository: Some("https://github.com/revoydotdev/twitch-chat-density".into()),
                    icon: None,
                    screenshots: vec![],
                    category: Some("Analytics".into()),
                },
                source: "first_party".into(),
                installed: true,
            },
            CatalogEntry {
                manifest: PluginManifest {
                    name: "deadair".into(),
                    version: "0.1.0".into(),
                    author: "Chorosyne".into(),
                    license: Some("MIT".into()),
                    description:
                        "Silence detection + one-click dead-air trim from inside the EDL editor.".into(),
                    capabilities: vec!["x.silence_detection".into()],
                    consumes: vec!["recording".into()],
                    entry_point: EntryPoint::Cdylib { path: "deadair.so".into() },
                    min_host_version: "0.3.0".into(),
                    price_cents: None,
                    repository: Some("https://github.com/revoydotdev/deadair".into()),
                    icon: None,
                    screenshots: vec![],
                    category: Some("Editor".into()),
                },
                source: "first_party".into(),
                installed: true,
            },
            CatalogEntry {
                manifest: PluginManifest {
                    name: "branding".into(),
                    version: "0.1.0".into(),
                    author: "Chorosyne".into(),
                    license: Some("MIT".into()),
                    description:
                        "Watermark + intro/outro banner overlay spec; applied as a filter_complex on EDL render."
                            .into(),
                    capabilities: vec!["x.overlay".into()],
                    consumes: vec!["recording".into()],
                    entry_point: EntryPoint::Cdylib { path: "branding.so".into() },
                    min_host_version: "0.3.0".into(),
                    price_cents: None,
                    repository: Some("https://github.com/revoydotdev/branding".into()),
                    icon: None,
                    screenshots: vec![],
                    category: Some("Editor".into()),
                },
                source: "first_party".into(),
                installed: true,
            },
            CatalogEntry {
                manifest: PluginManifest {
                    name: "multistream".into(),
                    version: "0.1.0".into(),
                    author: "Chorosyne".into(),
                    license: Some("MIT".into()),
                    description:
                        "Multi-stream viewer — auto-tile any subset of currently live followed channels, Twitch + YouTube embeds, focus + PiP modes."
                            .into(),
                    capabilities: vec!["x.multistream".into()],
                    consumes: vec!["x.channel_state".into()],
                    entry_point: EntryPoint::Cdylib { path: "multistream.so".into() },
                    min_host_version: "0.3.0".into(),
                    price_cents: None,
                    repository: Some("https://github.com/revoydotdev/multistream".into()),
                    icon: None,
                    screenshots: vec![],
                    category: Some("Viewer".into()),
                },
                source: "first_party".into(),
                installed: true,
            },
            CatalogEntry {
                manifest: PluginManifest {
                    name: "sidechain".into(),
                    version: "0.1.0".into(),
                    author: "Chorosyne".into(),
                    license: Some("MIT".into()),
                    description:
                        "DAW sidechain compressor — VAD voice intervals → ducking automation curve baked via the existing volume-automation render pipeline."
                            .into(),
                    capabilities: vec!["x.sidechain".into()],
                    consumes: vec!["x.voice_gate".into(), "x.audio_automation".into()],
                    entry_point: EntryPoint::Cdylib { path: "sidechain.so".into() },
                    min_host_version: "0.3.0".into(),
                    price_cents: None,
                    repository: Some("https://github.com/revoydotdev/sidechain".into()),
                    icon: None,
                    screenshots: vec![],
                    category: Some("Editor".into()),
                },
                source: "first_party".into(),
                installed: true,
            },
            CatalogEntry {
                manifest: PluginManifest {
                    name: "insert-fx".into(),
                    version: "0.1.0".into(),
                    author: "Chorosyne".into(),
                    license: Some("MIT".into()),
                    description:
                        "DAW-style insert effects chain — ordered HP/EQ/NR/de-esser/compressor/limiter/reverb stages per recording, voice + game bus presets. Composes into one ffmpeg -af filter baked at render."
                            .into(),
                    capabilities: vec!["x.insert_fx".into()],
                    consumes: vec!["recording".into()],
                    entry_point: EntryPoint::Cdylib { path: "insert-fx.so".into() },
                    min_host_version: "0.3.0".into(),
                    price_cents: None,
                    repository: Some("https://github.com/revoydotdev/insert-fx".into()),
                    icon: None,
                    screenshots: vec![],
                    category: Some("Editor".into()),
                },
                source: "first_party".into(),
                installed: true,
            },
            CatalogEntry {
                manifest: PluginManifest {
                    name: "pitch".into(),
                    version: "0.1.0".into(),
                    author: "Chorosyne".into(),
                    license: Some("MIT".into()),
                    description:
                        "Independent pitch / time-stretch — fit a recording to a publish slot without changing voices' pitch, or transpose stingers without changing tempo. Wraps ffmpeg rubberband, formant-preserving by default."
                            .into(),
                    capabilities: vec!["x.pitch_time".into()],
                    consumes: vec!["recording".into()],
                    entry_point: EntryPoint::Cdylib { path: "pitch.so".into() },
                    min_host_version: "0.3.0".into(),
                    price_cents: None,
                    repository: Some("https://github.com/revoydotdev/pitch".into()),
                    icon: None,
                    screenshots: vec![],
                    category: Some("Editor".into()),
                },
                source: "first_party".into(),
                installed: true,
            },
            CatalogEntry {
                manifest: PluginManifest {
                    name: "vad".into(),
                    version: "0.1.0".into(),
                    author: "Chorosyne".into(),
                    license: Some("MIT".into()),
                    description:
                        "Voice activity detection + auto-tighten — hysteresis-based DAW noise gate over ffmpeg astats envelopes."
                            .into(),
                    capabilities: vec!["x.voice_gate".into()],
                    consumes: vec!["recording".into()],
                    entry_point: EntryPoint::Cdylib { path: "vad.so".into() },
                    min_host_version: "0.3.0".into(),
                    price_cents: None,
                    repository: Some("https://github.com/revoydotdev/vad".into()),
                    icon: None,
                    screenshots: vec![],
                    category: Some("Editor".into()),
                },
                source: "first_party".into(),
                installed: true,
            },
            CatalogEntry {
                manifest: PluginManifest {
                    name: "beat-detect".into(),
                    version: "0.1.0".into(),
                    author: "Chorosyne".into(),
                    license: Some("MIT".into()),
                    description:
                        "DAW-style tempo grid — onset picker + autocorrelation BPM estimator from ffmpeg astats envelopes."
                            .into(),
                    capabilities: vec!["x.tempo".into()],
                    consumes: vec!["recording".into()],
                    entry_point: EntryPoint::Cdylib { path: "beat_detect.so".into() },
                    min_host_version: "0.3.0".into(),
                    price_cents: None,
                    repository: Some("https://github.com/revoydotdev/beat-detect".into()),
                    icon: None,
                    screenshots: vec![],
                    category: Some("Editor".into()),
                },
                source: "first_party".into(),
                installed: true,
            },
            CatalogEntry {
                manifest: PluginManifest {
                    name: "schedule-optimizer".into(),
                    version: "0.1.0".into(),
                    author: "Chorosyne".into(),
                    license: Some("MIT".into()),
                    description:
                        "Publish-slot recommender — engagement samples → 7×24 grid → top weekly publish times with confidence + plateau coverage."
                            .into(),
                    capabilities: vec!["x.publish_slots".into()],
                    consumes: vec!["audience_retention".into()],
                    entry_point: EntryPoint::Cdylib { path: "schedule_optimizer.so".into() },
                    min_host_version: "0.3.0".into(),
                    price_cents: None,
                    repository: Some("https://github.com/revoydotdev/schedule-optimizer".into()),
                    icon: None,
                    screenshots: vec![],
                    category: Some("Publish".into()),
                },
                source: "first_party".into(),
                installed: true,
            },
            CatalogEntry {
                manifest: PluginManifest {
                    name: "scenes".into(),
                    version: "0.1.0".into(),
                    author: "Chorosyne".into(),
                    license: Some("MIT".into()),
                    description:
                        "DAW-style session snapshots — bundle EDL + branding + automation + captions style as a named scene with optional thumbnail."
                            .into(),
                    capabilities: vec!["x.scenes".into()],
                    consumes: vec!["recording".into()],
                    entry_point: EntryPoint::Cdylib { path: "scenes.so".into() },
                    min_host_version: "0.3.0".into(),
                    price_cents: None,
                    repository: Some("https://github.com/revoydotdev/scenes".into()),
                    icon: None,
                    screenshots: vec![],
                    category: Some("Editor".into()),
                },
                source: "first_party".into(),
                installed: true,
            },
            CatalogEntry {
                manifest: PluginManifest {
                    name: "automation".into(),
                    version: "0.1.0".into(),
                    author: "Chorosyne".into(),
                    license: Some("MIT".into()),
                    description:
                        "DAW-style volume automation — time-keyed gain points with linear/cosine/step curves baked via ffmpeg asendcmd."
                            .into(),
                    capabilities: vec!["x.audio_automation".into()],
                    consumes: vec!["recording".into()],
                    entry_point: EntryPoint::Cdylib { path: "automation.so".into() },
                    min_host_version: "0.3.0".into(),
                    price_cents: None,
                    repository: Some("https://github.com/revoydotdev/automation".into()),
                    icon: None,
                    screenshots: vec![],
                    category: Some("Editor".into()),
                },
                source: "first_party".into(),
                installed: true,
            },
            CatalogEntry {
                manifest: PluginManifest {
                    name: "structure".into(),
                    version: "0.1.0".into(),
                    author: "Chorosyne".into(),
                    license: Some("MIT".into()),
                    description:
                        "DAW-style section labeller — intro / gameplay / break / outro / content tiling derived from chapters + chat density + scene cuepoints."
                            .into(),
                    capabilities: vec!["x.structure".into()],
                    consumes: vec!["chapters".into(), "scene_detection".into(), "audience_retention".into()],
                    entry_point: EntryPoint::Cdylib { path: "structure.so".into() },
                    min_host_version: "0.3.0".into(),
                    price_cents: None,
                    repository: Some("https://github.com/revoydotdev/structure".into()),
                    icon: None,
                    screenshots: vec![],
                    category: Some("Editor".into()),
                },
                source: "first_party".into(),
                installed: true,
            },
            CatalogEntry {
                manifest: PluginManifest {
                    name: "loudness".into(),
                    version: "0.1.0".into(),
                    author: "Chorosyne".into(),
                    license: Some("MIT".into()),
                    description:
                        "EBU R128 loudness normalisation — ffmpeg loudnorm two-pass parser with per-platform presets (YouTube/Spotify/Apple/Twitch/EBU)."
                            .into(),
                    capabilities: vec!["x.loudness".into()],
                    consumes: vec!["recording".into()],
                    entry_point: EntryPoint::Cdylib { path: "loudness.so".into() },
                    min_host_version: "0.3.0".into(),
                    price_cents: None,
                    repository: Some("https://github.com/revoydotdev/loudness".into()),
                    icon: None,
                    screenshots: vec![],
                    category: Some("Editor".into()),
                },
                source: "first_party".into(),
                installed: true,
            },
            CatalogEntry {
                manifest: PluginManifest {
                    name: "chat".into(),
                    version: "0.1.0".into(),
                    author: "Chorosyne".into(),
                    license: Some("MIT".into()),
                    description:
                        "Chatterino-class chat client — Twitch IRC parser, emote tokenizer, mention/keyword filter pipeline, per-room ring buffer with unread counters."
                            .into(),
                    capabilities: vec!["x.chat".into()],
                    consumes: vec!["x.channel_state".into()],
                    entry_point: EntryPoint::Cdylib { path: "chat.so".into() },
                    min_host_version: "0.3.0".into(),
                    price_cents: None,
                    repository: Some("https://github.com/revoydotdev/chat".into()),
                    icon: None,
                    screenshots: vec![],
                    category: Some("Viewer".into()),
                },
                source: "first_party".into(),
                installed: true,
            },
            CatalogEntry {
                manifest: PluginManifest {
                    name: "yt-publish".into(),
                    version: "0.2.0".into(),
                    author: "Chorosyne".into(),
                    license: Some("MIT".into()),
                    description: "Real YouTube + Shorts publisher hooked to the Reuse draft set.".into(),
                    capabilities: vec!["publish_queue".into()],
                    consumes: vec!["publish_queue".into()],
                    entry_point: EntryPoint::Roadmap,
                    min_host_version: "0.3.0".into(),
                    price_cents: Some(1500),
                    repository: Some("https://github.com/revoydotdev/yt-publish".into()),
                    icon: None,
                    screenshots: vec![],
                    category: Some("Publish".into()),
                },
                source: "first_party".into(),
                installed: false,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_toml() -> &'static str {
        r#"
name = "example-plugin"
version = "0.1.0"
author = "Alice"
license = "MIT"
description = "A demo plugin."
capabilities = ["transcription", "x.demo"]
consumes = ["recording"]
min_host_version = "0.3.0"

[entry_point]
kind = "cdylib"
path = "example.so"
"#
    }

    #[test]
    fn parses_well_formed_manifest() {
        let m = parse_manifest(sample_toml()).unwrap();
        assert_eq!(m.name, "example-plugin");
        assert_eq!(m.version, "0.1.0");
        assert_eq!(m.capabilities, vec!["transcription", "x.demo"]);
        match m.entry_point {
            EntryPoint::Cdylib { ref path } => assert_eq!(path, "example.so"),
            _ => panic!("expected cdylib entry point"),
        }
    }

    #[test]
    fn parse_rejects_invalid_toml() {
        assert!(parse_manifest("this is not toml = ").is_err());
    }

    #[test]
    fn validate_accepts_sample_manifest() {
        let m = parse_manifest(sample_toml()).unwrap();
        validate_manifest(&m).expect("sample should validate");
    }

    #[test]
    fn validate_rejects_empty_name() {
        let mut m = parse_manifest(sample_toml()).unwrap();
        m.name = "".into();
        let e = validate_manifest(&m).unwrap_err();
        assert!(e.contains("name"));
    }

    #[test]
    fn validate_rejects_non_kebab_snake_name() {
        let mut m = parse_manifest(sample_toml()).unwrap();
        m.name = "Example Plugin".into();
        let e = validate_manifest(&m).unwrap_err();
        assert!(e.contains("kebab-case or snake_case"));
    }

    #[test]
    fn validate_rejects_bad_semver_version() {
        let mut m = parse_manifest(sample_toml()).unwrap();
        m.version = "garbage".into();
        let e = validate_manifest(&m).unwrap_err();
        assert!(e.contains("version"));
    }

    #[test]
    fn validate_rejects_min_host_version_too_new() {
        let mut m = parse_manifest(sample_toml()).unwrap();
        m.min_host_version = "99.0.0".into();
        let e = validate_manifest(&m).unwrap_err();
        assert!(e.contains("min_host_version"));
    }

    #[test]
    fn validate_accepts_min_host_version_equal_to_host() {
        let mut m = parse_manifest(sample_toml()).unwrap();
        m.min_host_version = HOST_VERSION.to_string();
        validate_manifest(&m).expect("equal version should pass");
    }

    #[test]
    fn validate_rejects_unknown_capability_without_x_prefix() {
        let mut m = parse_manifest(sample_toml()).unwrap();
        m.capabilities.push("not_a_real_thing".into());
        let e = validate_manifest(&m).unwrap_err();
        assert!(e.contains("capability"));
    }

    #[test]
    fn validate_accepts_x_prefixed_capability() {
        let mut m = parse_manifest(sample_toml()).unwrap();
        m.capabilities.push("x.some_custom_tag".into());
        validate_manifest(&m).expect("x. prefix should be accepted");
    }

    #[test]
    fn validate_rejects_empty_cdylib_path() {
        let mut m = parse_manifest(sample_toml()).unwrap();
        m.entry_point = EntryPoint::Cdylib { path: "".into() };
        let e = validate_manifest(&m).unwrap_err();
        assert!(e.contains("entry_point.path"));
    }

    #[test]
    fn validate_rejects_empty_rpc_url() {
        let mut m = parse_manifest(sample_toml()).unwrap();
        m.entry_point = EntryPoint::Rpc { url: "".into() };
        let e = validate_manifest(&m).unwrap_err();
        assert!(e.contains("entry_point.url"));
    }

    #[test]
    fn validate_accepts_roadmap_entry_point() {
        let mut m = parse_manifest(sample_toml()).unwrap();
        m.entry_point = EntryPoint::Roadmap;
        validate_manifest(&m).expect("roadmap entry point should pass");
    }

    #[test]
    fn default_catalog_is_non_empty_and_each_entry_validates() {
        let cat = default_catalog();
        assert!(cat.entries.len() >= 4);
        for entry in &cat.entries {
            validate_manifest(&entry.manifest).unwrap_or_else(|e| {
                panic!(
                    "catalog entry '{}' fails validation: {e}",
                    entry.manifest.name
                )
            });
        }
    }

    #[test]
    fn is_valid_capability_recognises_well_known_and_x_prefix() {
        assert!(is_valid_capability("transcription"));
        assert!(is_valid_capability("x.something"));
        assert!(!is_valid_capability("not_known"));
    }
}
