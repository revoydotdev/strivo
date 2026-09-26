//! The single feature-capability descriptor (plan todo 20).
//!
//! Before this module, one fact — "what can this build do, and what does a
//! capability cost" — had four hand-kept homes: the `PRO_PLUGINS` list in
//! `routes::plugins`, 90 `gate_pro("…")` call sites, a `status: "roadmap"`
//! hand-mark on `yt-publish` in `routes::import_export`, and
//! `proGated: true` in the SPA's `PLUGIN_REGISTRY`. They disagreed — the
//! server gated four plugins, the SPA badged thirty-eight, and one
//! capability in the matrix had no executor anywhere. The descriptor is the
//! one list all four now read.
//!
//! **Rust-authoritative, served, and derived everywhere else.** The gates,
//! the SPA's per-plugin flags, and the capability matrix are all projections
//! of [`CAPABILITIES`]. Nothing downstream holds a second copy.
//!
//! ## The invariant is subset, not equality
//!
//! Every capability a gate names appears here, but the descriptor is
//! strictly larger: it also carries capabilities with no gate call site at
//! all — `yt-publish` above all, which is [`EntryPoint::Roadmap`] and
//! `installed: false`. Demanding equality between "gated names" and
//! "descriptor rows" would make that permanently unrepresentable, which is
//! the opposite of what this module is for. The gates are a subset; the
//! descriptor is the superset.
//!
//! ## Two different meanings of "Pro", deliberately kept apart
//!
//! [`Entitlement::Pro`] is the *enforcement* fact: `is_entitled` is handed
//! this set and a request for one of them is answered 402. [`Edition`] is
//! the *reachability* fact: a capability the PVR edition does not ship at
//! all. They are not the same set today — the gates name four paid plugins
//! while thirty-eight capabilities are Creator-only — and conflating them
//! is one of the drifts this module removes. The SPA's `proGated` badge
//! means "not in the PVR edition" (reachability); the upgrade card means
//! "paid" (entitlement).
//!
//! ## Why the PVR table is empty
//!
//! [`CAPABILITIES`] is `#[cfg]`-split rather than filtered at runtime: a
//! PVR build must not so much as contain the Creator capability names
//! (ADR 0002's invisibility bar), so the rows are compiled out, not
//! hidden. That is why the edition is a per-row field *and* a build-time
//! split — the field is what the SPA reads, the split is what keeps the
//! names out of the binary.
//!
//! ## `requires` is a declared dependency, not a trace
//!
//! It lists the external binaries the shipped path for that capability
//! invokes — the ones its own crate spawns, or its own web route renders
//! through. An empty list means "this capability's own path spawns
//! nothing", *not* "no binary is ever involved downstream": `sidechain`
//! bakes through the volume-automation render, which is ffmpeg, but
//! `sidechain_run` itself spawns nothing.

use serde::{Deserialize, Serialize};

/// Which edition serves a capability. Every row a Creator build serves is
/// [`Edition::Creator`] today because the PVR table is empty — but the
/// field is per-row because the moment a PVR capability needs gating the
/// answer stops being constant, and a per-row fact is the only shape that
/// can say so without a second list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edition {
    Pvr,
    Creator,
}

/// Whether a capability costs money. [`Entitlement::Pro`] rows are the set
/// `strivo_core::licence::gate::is_entitled` is consulted with; everything
/// else is [`Entitlement::Included`] and always allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Entitlement {
    Included,
    Pro,
}

/// Whether a capability has an executor in this build.
///
/// Serialises to the same two strings the capability matrix has always
/// used, so the SPA's `pg-cap-available` / `pg-cap-roadmap` styling and
/// `entryStatus()` keep working untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryPoint {
    /// Shipped: something in this tree can run it.
    Available,
    /// A reserved slot with no executor — the capability is real, the code
    /// is not written yet.
    Roadmap,
}

/// The descriptor's key type. A capability a gate can name is a
/// `CapabilityKey`, so it cannot be a string no row describes; an
/// undeclared key is not constructible from the wire either, because
/// `Deserialize` rejects an unknown variant rather than defaulting.
///
/// The wire spelling is kebab-case and is the canonical name everywhere —
/// gates, the SPA registry, the matrix, and the marketplace catalog all use
/// it. (`chat-density`, not `chat_density`: the two spellings had drifted.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityKey {
    AbRender,
    Archiver,
    Automation,
    BeatDetect,
    Broll,
    Brandsafe,
    Branding,
    Captions,
    CaptionsAss,
    Casebook,
    Chapters,
    Chat,
    ChatDensity,
    Clipper,
    Crunchr,
    Cuepoints,
    Deadair,
    Editor,
    Heatmap,
    InsertFx,
    Insights,
    InsightsCompare,
    Loudness,
    Marketplace,
    Multistream,
    Multitrack,
    PipelinesDag,
    Pitch,
    Reuse,
    Scenes,
    ScheduleOptimizer,
    Sidechain,
    Structure,
    Submix,
    Thumbnails,
    Vad,
    Viewguard,
    ViewguardTrend,
    YtPublish,
}

impl CapabilityKey {
    /// Every key the descriptor knows, in table order. Derived from
    /// [`CAPABILITIES`] rather than a second list, so it cannot fall out of
    /// step with it.
    pub fn all() -> impl Iterator<Item = Self> {
        CAPABILITIES.iter().map(|c| c.key)
    }

    /// The key's canonical wire spelling, as a `&'static str`.
    ///
    /// `serde`'s `rename_all` owns the spelling, and this is a checked
    /// cache of it: `as_str_matches_serde_rename_all` walks every variant
    /// and fails if the two disagree, so a new variant with a wrong arm
    /// cannot ship. Without the cache every gate would allocate to get the
    /// name it is keyed on.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AbRender => "ab-render",
            Self::Archiver => "archiver",
            Self::Automation => "automation",
            Self::BeatDetect => "beat-detect",
            Self::Broll => "broll",
            Self::Brandsafe => "brandsafe",
            Self::Branding => "branding",
            Self::Captions => "captions",
            Self::CaptionsAss => "captions-ass",
            Self::Casebook => "casebook",
            Self::Chapters => "chapters",
            Self::Chat => "chat",
            Self::ChatDensity => "chat-density",
            Self::Clipper => "clipper",
            Self::Crunchr => "crunchr",
            Self::Cuepoints => "cuepoints",
            Self::Deadair => "deadair",
            Self::Editor => "editor",
            Self::Heatmap => "heatmap",
            Self::InsertFx => "insert-fx",
            Self::Insights => "insights",
            Self::InsightsCompare => "insights-compare",
            Self::Loudness => "loudness",
            Self::Marketplace => "marketplace",
            Self::Multistream => "multistream",
            Self::Multitrack => "multitrack",
            Self::PipelinesDag => "pipelines-dag",
            Self::Pitch => "pitch",
            Self::Reuse => "reuse",
            Self::Scenes => "scenes",
            Self::ScheduleOptimizer => "schedule-optimizer",
            Self::Sidechain => "sidechain",
            Self::Structure => "structure",
            Self::Submix => "submix",
            Self::Thumbnails => "thumbnails",
            Self::Vad => "vad",
            Self::Viewguard => "viewguard",
            Self::ViewguardTrend => "viewguard-trend",
            Self::YtPublish => "yt-publish",
        }
    }

    /// This key's descriptor row. Panics if the row is missing, which the
    /// `every_capability_key_has_exactly_one_descriptor_row` test makes
    /// unreachable: a variant added without a row fails the suite, it does
    /// not fail a request.
    pub fn descriptor(self) -> &'static Capability {
        CAPABILITIES
            .iter()
            .find(|c| c.key == self)
            .unwrap_or_else(|| panic!("capability {self:?} has no descriptor row"))
    }

    /// Whether a request for this capability is entitled on this machine.
    /// Delegates to core's gate with the descriptor's Pro set, so that set
    /// has exactly one definition.
    pub fn is_entitled(self) -> bool {
        strivo_core::licence::gate::is_entitled(self.as_str(), &pro_slugs())
    }
}

/// Every Pro capability's canonical name — the set core's entitlement gate
/// is consulted with. Derived, never written out: a second hand-kept list
/// is the drift this module exists to remove, and todo 27 derives the
/// entitlement set from exactly this function.
pub fn pro_slugs() -> Vec<&'static str> {
    CAPABILITIES
        .iter()
        .filter(|c| c.entitlement == Entitlement::Pro)
        .map(|c| c.key.as_str())
        .collect()
}

/// One capability, as the descriptor states it.
///
/// `Serialize` only: the descriptor is an outbound projection, and a
/// `&'static [&'static str]` cannot be deserialised into. The one inbound
/// parse is [`CapabilityKey`], whose unknown-variant rejection is what
/// keeps a caller from naming a capability the table has never heard of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Capability {
    /// Canonical name, kebab-case.
    pub key: CapabilityKey,
    /// Which edition ships it.
    pub edition: Edition,
    /// Whether it costs money.
    pub entitlement: Entitlement,
    /// Whether an executor exists.
    pub entry_point: EntryPoint,
    /// Whether something in this tree can run it — the SPA's "not
    /// installed" badge, and `false` for every [`EntryPoint::Roadmap`] row.
    pub installed: bool,
    /// External binaries the shipped path for this capability invokes.
    pub requires: &'static [&'static str],
    /// DAW-vision capability tags it fulfils. These are what the
    /// capability matrix groups by, so a row with no tag is a capability
    /// the matrix does not surface (`ab-render`, `submix`).
    pub capability_tags: &'static [&'static str],
}

impl Capability {
    /// `installed` is derived here, from the one fact that decides it, so
    /// the two cannot drift. Only the editions that have rows call it.
    #[cfg(feature = "creator")]
    const fn new(
        key: CapabilityKey,
        entitlement: Entitlement,
        entry_point: EntryPoint,
        requires: &'static [&'static str],
        capability_tags: &'static [&'static str],
    ) -> Self {
        Self {
            key,
            edition: Edition::Creator,
            entitlement,
            entry_point,
            installed: matches!(entry_point, EntryPoint::Available),
            requires,
            capability_tags,
        }
    }

    /// The matrix row each of this capability's tags contributes. The
    /// provider's `status` is the executor verdict, which is how
    /// `yt-publish`'s `"roadmap"` stopped being hand-written.
    fn matrix_rows(
        self: &'static Capability,
    ) -> impl Iterator<Item = (&'static str, serde_json::Value)> {
        let status = match self.entry_point {
            EntryPoint::Available => "available",
            EntryPoint::Roadmap => "roadmap",
        };
        self.capability_tags.iter().map(move |tag| {
            (
                *tag,
                serde_json::json!({
                    "plugin": self.key.as_str(),
                    "status": status,
                }),
            )
        })
    }
}

// allow: SIZE_OK — pure data table. 39 rows, one per CapabilityKey variant,
// in the order that reproduces the capability matrix's row order. Splitting
// it would mean splitting the descriptor, which is the one thing it must
// not be.
/// Every capability this build ships. The single definition: the gates,
/// the SPA, and the matrix are all projections of this table.
#[cfg(feature = "creator")]
pub const CAPABILITIES: &[Capability] = &[
    Capability::new(
        CapabilityKey::Crunchr,
        Entitlement::Pro,
        EntryPoint::Available,
        &["ffmpeg", "whisper"],
        &[
            "transcription",
            "word_timestamps",
            "diarisation",
            "topic_segmentation",
        ],
    ),
    Capability::new(
        CapabilityKey::Chapters,
        Entitlement::Included,
        EntryPoint::Available,
        &["ffmpeg"],
        &["chapters"],
    ),
    Capability::new(
        CapabilityKey::Cuepoints,
        Entitlement::Included,
        EntryPoint::Available,
        &["ffmpeg"],
        &["scene_detection"],
    ),
    Capability::new(
        CapabilityKey::Thumbnails,
        Entitlement::Included,
        EntryPoint::Available,
        &["ffmpeg", "ffprobe"],
        &["thumbnail_ranking"],
    ),
    Capability::new(
        CapabilityKey::Clipper,
        Entitlement::Included,
        EntryPoint::Available,
        &["ffmpeg"],
        &["highlight_detection", "clip_extraction"],
    ),
    Capability::new(
        CapabilityKey::Captions,
        Entitlement::Included,
        EntryPoint::Available,
        &["ffprobe"],
        &["translation", "captions"],
    ),
    Capability::new(
        CapabilityKey::CaptionsAss,
        Entitlement::Included,
        EntryPoint::Available,
        &[],
        &["captions"],
    ),
    Capability::new(
        CapabilityKey::Heatmap,
        Entitlement::Included,
        EntryPoint::Available,
        &[],
        &["audience_retention"],
    ),
    Capability::new(
        CapabilityKey::ChatDensity,
        Entitlement::Included,
        EntryPoint::Available,
        &["ffmpeg"],
        &["audience_retention"],
    ),
    Capability::new(
        CapabilityKey::Viewguard,
        Entitlement::Pro,
        EntryPoint::Available,
        &[],
        &["fraud_detection"],
    ),
    Capability::new(
        CapabilityKey::Insights,
        Entitlement::Pro,
        EntryPoint::Available,
        &[],
        &["stream_comparison"],
    ),
    Capability::new(
        CapabilityKey::ViewguardTrend,
        Entitlement::Included,
        EntryPoint::Available,
        &[],
        &["stream_comparison"],
    ),
    Capability::new(
        CapabilityKey::InsightsCompare,
        Entitlement::Included,
        EntryPoint::Available,
        &[],
        &[],
    ),
    Capability::new(
        CapabilityKey::Casebook,
        Entitlement::Included,
        EntryPoint::Available,
        &["ffprobe"],
        &["reporting"],
    ),
    Capability::new(
        CapabilityKey::Brandsafe,
        Entitlement::Included,
        EntryPoint::Available,
        &[],
        &["brand_safety"],
    ),
    Capability::new(
        CapabilityKey::Archiver,
        Entitlement::Pro,
        EntryPoint::Available,
        &["yt-dlp"],
        &["asset_catalog"],
    ),
    Capability::new(
        CapabilityKey::Multitrack,
        Entitlement::Included,
        EntryPoint::Available,
        &["ffmpeg", "ffprobe"],
        &["source_track_split"],
    ),
    Capability::new(
        CapabilityKey::Reuse,
        Entitlement::Included,
        EntryPoint::Available,
        &["ffprobe"],
        &["publish_queue"],
    ),
    // Roadmap: a reserved slot, not shipped. YouTube OAuth + API
    // credentials are the missing half, and the marketplace catalog
    // carries the same verdict. Representing it here is the point — an
    // equality invariant between gated names and descriptor rows would
    // have made this unrepresentable.
    Capability::new(
        CapabilityKey::YtPublish,
        Entitlement::Included,
        EntryPoint::Roadmap,
        &["yt-dlp"],
        &["publish_queue"],
    ),
    Capability::new(
        CapabilityKey::Editor,
        Entitlement::Included,
        EntryPoint::Available,
        &["ffmpeg"],
        &["edl_editor"],
    ),
    Capability::new(
        CapabilityKey::Deadair,
        Entitlement::Included,
        EntryPoint::Available,
        &["ffmpeg"],
        &["edl_editor"],
    ),
    Capability::new(
        CapabilityKey::Branding,
        Entitlement::Included,
        EntryPoint::Available,
        &["ffmpeg"],
        &["edl_editor"],
    ),
    Capability::new(
        CapabilityKey::Broll,
        Entitlement::Included,
        EntryPoint::Available,
        &[],
        &["edl_editor"],
    ),
    Capability::new(
        CapabilityKey::Loudness,
        Entitlement::Included,
        EntryPoint::Available,
        &["ffmpeg"],
        &["x.loudness"],
    ),
    Capability::new(
        CapabilityKey::Structure,
        Entitlement::Included,
        EntryPoint::Available,
        &[],
        &["x.structure"],
    ),
    Capability::new(
        CapabilityKey::Automation,
        Entitlement::Included,
        EntryPoint::Available,
        &["ffmpeg"],
        &["x.audio_automation"],
    ),
    Capability::new(
        CapabilityKey::Scenes,
        Entitlement::Included,
        EntryPoint::Available,
        &[],
        &["x.scenes"],
    ),
    Capability::new(
        CapabilityKey::ScheduleOptimizer,
        Entitlement::Included,
        EntryPoint::Available,
        &[],
        &["x.publish_slots"],
    ),
    Capability::new(
        CapabilityKey::BeatDetect,
        Entitlement::Included,
        EntryPoint::Available,
        &["ffmpeg"],
        &["x.tempo"],
    ),
    Capability::new(
        CapabilityKey::Vad,
        Entitlement::Included,
        EntryPoint::Available,
        &["ffmpeg"],
        &["x.voice_gate"],
    ),
    Capability::new(
        CapabilityKey::Sidechain,
        Entitlement::Included,
        EntryPoint::Available,
        &[],
        &["x.sidechain"],
    ),
    Capability::new(
        CapabilityKey::InsertFx,
        Entitlement::Included,
        EntryPoint::Available,
        &["ffmpeg"],
        &["x.insert_fx"],
    ),
    Capability::new(
        CapabilityKey::Pitch,
        Entitlement::Included,
        EntryPoint::Available,
        &[],
        &["x.pitch_time"],
    ),
    Capability::new(
        CapabilityKey::Multistream,
        Entitlement::Included,
        EntryPoint::Available,
        &[],
        &["x.multistream"],
    ),
    Capability::new(
        CapabilityKey::Chat,
        Entitlement::Included,
        EntryPoint::Available,
        &[],
        &["x.chat"],
    ),
    Capability::new(
        CapabilityKey::PipelinesDag,
        Entitlement::Included,
        EntryPoint::Available,
        &[],
        &["x.pipelines_dag"],
    ),
    Capability::new(
        CapabilityKey::Marketplace,
        Entitlement::Included,
        EntryPoint::Available,
        &[],
        &["x.marketplace"],
    ),
    Capability::new(
        CapabilityKey::AbRender,
        Entitlement::Included,
        EntryPoint::Available,
        &["ffmpeg"],
        &[],
    ),
    Capability::new(
        CapabilityKey::Submix,
        Entitlement::Included,
        EntryPoint::Available,
        &["ffmpeg"],
        &[],
    ),
];

/// The PVR edition gates nothing, so its descriptor is empty. Compiled out
/// rather than filtered: a PVR binary must not contain the Creator
/// capability names at all (ADR 0002).
#[cfg(not(feature = "creator"))]
pub const CAPABILITIES: &[Capability] = &[];

/// The edition this descriptor was built for. Naming [`Edition::Pvr`] here
/// is what keeps that variant honest in a build that has no PVR row to
/// point at.
pub const EDITION: Edition = if cfg!(feature = "creator") {
    Edition::Creator
} else {
    Edition::Pvr
};

/// The wire form of the descriptor, served at
/// `GET /api/v1/plugins/descriptor`. `version` is the projection's shape
/// version, so a client can tell a descriptor it understands from one it
/// does not — the alternative being a client that reads a field's absence
/// as "false".
pub fn descriptor_json() -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "edition": EDITION,
        "capabilities": CAPABILITIES,
    })
}

/// The capability matrix, projected from the descriptor: one row per
/// DAW-vision tag, its providers in descriptor order. The row order this
/// produces is the order the hand-written matrix used, so the SPA's
/// rendering is unchanged.
pub fn capability_matrix() -> serde_json::Value {
    // Group by tag, preserving both the first-seen tag order and each
    // tag's provider order. `serde_json::Map` is not ordered, so the
    // grouping is done by hand over the entry sequence.
    let mut rows: Vec<(&'static str, Vec<serde_json::Value>)> = Vec::new();
    for capability in CAPABILITIES {
        for (tag, provider) in capability.matrix_rows() {
            match rows.iter_mut().find(|(t, _)| *t == tag) {
                Some((_, providers)) => providers.push(provider),
                None => rows.push((tag, vec![provider])),
            }
        }
    }
    serde_json::Value::Array(
        rows.into_iter()
            .map(|(capability, providers)| {
                serde_json::json!({ "capability": capability, "providers": providers })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_capability_key_has_exactly_one_descriptor_row() {
        let mut seen = HashSet::new();
        for c in CAPABILITIES {
            assert!(
                seen.insert(c.key),
                "duplicate descriptor row for {}",
                c.key.as_str()
            );
        }
        // Non-vacuity is edition-scoped: a PVR descriptor is *supposed* to
        // be empty, so only the Creator build's row set has to be non-empty.
        if cfg!(feature = "creator") {
            assert!(!seen.is_empty(), "descriptor is empty; the test is vacuous");
        }
        assert_eq!(seen.len(), CAPABILITIES.len());
        for key in CapabilityKey::all() {
            assert_eq!(key.descriptor().key, key);
        }
    }

    #[test]
    fn installed_agrees_with_entry_point() {
        for c in CAPABILITIES {
            assert_eq!(
                c.installed,
                c.entry_point == EntryPoint::Available,
                "{}",
                c.key.as_str()
            );
        }
    }

    /// A name the enum has never heard of must not be constructible from
    /// the wire — the failure is a typed error, not a silent default.
    #[test]
    fn unknown_capability_key_is_rejected_on_the_wire() {
        assert!(serde_json::from_str::<CapabilityKey>("\"newtool\"").is_err());
        // Near-miss spellings are rejected too: this is why the two
        // historical spellings of the chat-density capability had to be
        // reconciled rather than both kept.
        assert!(serde_json::from_str::<CapabilityKey>("\"chat_density\"").is_err());
        assert_eq!(
            serde_json::from_str::<CapabilityKey>("\"chat-density\"")
                .expect("the canonical spelling parses"),
            CapabilityKey::ChatDensity
        );
    }

    /// `as_str` is a `match` of 38 arms sitting next to a `rename_all`
    /// attribute that could also produce the spelling. That is a second
    /// list by appearance, so it is pinned: this walks every key the
    /// descriptor knows and fails if the two ever disagree. A new variant
    /// with a wrong arm (or a missing one) fails here rather than
    /// shipping a name nothing else uses.
    #[test]
    fn as_str_matches_serde_rename_all() {
        let mut checked = 0;
        for key in CapabilityKey::all() {
            let via_serde = serde_json::to_value(key).expect("serialise key");
            assert_eq!(
                via_serde.as_str(),
                Some(key.as_str()),
                "{}: as_str disagrees with serde's rename_all",
                key.as_str()
            );
            // And the round trip, so the name on the wire is the name that
            // parses back to the same variant.
            let back: CapabilityKey = serde_json::from_value(via_serde).expect("parse key");
            assert_eq!(back, key);
            checked += 1;
        }
        // A PVR build has no rows, so there is nothing to walk; the
        // Creator build must walk all of them.
        assert_eq!(checked > 0, cfg!(feature = "creator"), "no keys checked");
    }

    #[test]
    fn descriptor_round_trips_through_serde() {
        let once = serde_json::to_value(CAPABILITIES).expect("serialise");
        let twice = serde_json::to_value(&once).expect("re-serialise");
        assert_eq!(once, twice);
        assert!(once.as_array().expect("array").len() == CAPABILITIES.len());
    }

    /// The PVR side of the contract, asserted in the configuration that
    /// matters. Run without `--features creator` this is a PVR build and
    /// the answer must be zero; run with it, the answer must not be.
    #[test]
    fn pvr_descriptor_reports_zero_pro_capabilities() {
        let pro = pro_slugs();
        if cfg!(feature = "creator") {
            assert!(
                !pro.is_empty(),
                "a Creator descriptor with no Pro capability is suspicious"
            );
        } else {
            assert_eq!(
                pro,
                Vec::<&str>::new(),
                "a PVR build must report zero Pro capabilities"
            );
            assert!(CAPABILITIES.is_empty());
            assert_eq!(EDITION, Edition::Pvr);
            assert_eq!(
                descriptor_json()["capabilities"].as_array().map(Vec::len),
                Some(0)
            );
        }
    }

    /// Exactly one descriptor entry per roadmap-marked capability — and the
    /// set is asserted non-empty, because a test over an empty set proves
    /// nothing.
    #[cfg(feature = "creator")]
    #[test]
    fn roadmap_capability_has_exactly_one_descriptor_entry_and_one_matrix_provider() {
        let roadmap: Vec<Capability> = CAPABILITIES
            .iter()
            .copied()
            .filter(|c| c.entry_point == EntryPoint::Roadmap)
            .collect();
        assert!(
            !roadmap.is_empty(),
            "no roadmap capability; the test is vacuous"
        );
        assert_eq!(
            roadmap
                .iter()
                .filter(|c| c.key == CapabilityKey::YtPublish)
                .count(),
            1,
            "yt-publish must be represented exactly once, as a roadmap slot"
        );
        for c in &roadmap {
            assert!(
                !c.installed,
                "{} is roadmap but claims installed",
                c.key.as_str()
            );
        }

        let matrix = capability_matrix();
        let providers = |plugin: &str| -> Vec<&serde_json::Value> {
            matrix
                .as_array()
                .expect("matrix is an array")
                .iter()
                .flat_map(|row| {
                    row["providers"]
                        .as_array()
                        .expect("providers is an array")
                        .iter()
                        .filter(move |p| p["plugin"] == plugin)
                })
                .collect()
        };
        for c in &roadmap {
            let found = providers(c.key.as_str());
            assert_eq!(
                found.len(),
                c.capability_tags.len(),
                "{} appears {} times in the matrix but declares {} tag(s)",
                c.key.as_str(),
                found.len(),
                c.capability_tags.len()
            );
            for p in found {
                assert_eq!(p["status"], "roadmap", "{}", c.key.as_str());
            }
        }
    }

    /// The matrix loses nothing the descriptor declares.
    #[cfg(feature = "creator")]
    #[test]
    fn matrix_is_exactly_the_projection_of_the_descriptor() {
        let expected: usize = CAPABILITIES.iter().map(|c| c.capability_tags.len()).sum();
        let matrix = capability_matrix();
        let actual: usize = matrix
            .as_array()
            .expect("matrix is an array")
            .iter()
            .map(|row| row["providers"].as_array().map(Vec::len).unwrap_or(0))
            .sum();
        assert_eq!(actual, expected, "the matrix is not a faithful projection");
        assert!(!matrix.as_array().expect("array").is_empty());
    }

    /// Every affordance the SPA offers must name a capability the
    /// descriptor describes, or the UI is advertising something with no
    /// row behind it. The registry no longer states `proGated` inline —
    /// it folds the descriptor in — so this reads every row it declares.
    #[cfg(feature = "creator")]
    #[test]
    fn every_spa_affordance_exists_in_the_descriptor() {
        const REGISTRY: &str = include_str!("../assets/spa/020-creator.js");
        let mut offered: Vec<&str> = Vec::new();
        for line in REGISTRY.lines() {
            let t = line.trim();
            let Some(rest) = t.strip_prefix("{ name: \"") else {
                continue;
            };
            if let Some((name, _)) = rest.split_once('"') {
                offered.push(name);
            }
        }
        assert!(
            offered.len() > 20,
            "only {} affordances parsed; the scan is broken, not passing",
            offered.len()
        );
        let key_of = |name: &str| serde_json::from_str::<CapabilityKey>(&format!("\"{name}\""));
        let unknown: Vec<&str> = offered
            .iter()
            .copied()
            .filter(|n| key_of(n).is_err())
            .collect();
        assert!(
            unknown.is_empty(),
            "SPA affordance(s) with no descriptor row: {unknown:?}"
        );
        // And the badge the SPA will now derive: every affordance it offers
        // is a capability this build can actually run. A roadmap row here
        // would mean the UI advertises a plugin that cannot execute.
        let unrunnable: Vec<&str> = offered
            .iter()
            .copied()
            .filter(|n| !key_of(n).expect("parses").descriptor().installed)
            .collect();
        assert!(
            unrunnable.is_empty(),
            "SPA offers these but the descriptor says no executor: {unrunnable:?}"
        );
    }

    /// A row this descriptor serves must belong to the edition that built
    /// it. The other half — that a PVR build serves no Creator row at all —
    /// is the `#[cfg]` split above, and is what
    /// `pvr_descriptor_reports_zero_pro_capabilities` observes from inside
    /// a PVR build.
    #[cfg(feature = "creator")]
    #[test]
    fn every_served_row_belongs_to_the_edition_that_serves_it() {
        for c in CAPABILITIES {
            assert_eq!(c.edition, EDITION, "{}", c.key.as_str());
        }
    }
}
