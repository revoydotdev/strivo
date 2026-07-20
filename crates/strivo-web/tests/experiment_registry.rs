// Creator Edition only: the registry + the hydrate-and-run endpoint it
// feeds live behind the `creator` feature (see server.rs: the plugin
// router is only merged `#[cfg(feature = "creator")]`, same constraint
// documented in tests/corpus_endpoint_route.rs).
#![cfg(feature = "creator")]
//! Coverage for the M4.P1.S1.T1 "experiment registry over dataviz, run
//! against a hydrated corpus" gap: the SPA/API consumer used to have to
//! hardcode the six `Experiment` variant names, and the only server-side
//! path from "scope selection" to a chartable `Series` required the
//! client to round-trip the whole hydrated corpus back through
//! `POST /api/v1/dataviz/run`.
//!
//! This file proves, at the two levels available to an external test
//! crate (see `tests/corpus_endpoint_route.rs` for why a real `.oneshot()`
//! HTTP dispatch isn't reachable here — `AppState` needs a live daemon
//! IPC connection):
//!
//!  (a) [`strivo_web::experiment_registry::list_experiments`] describes
//!      the real six experiments (unit-level coverage for this already
//!      lives in `src/experiment_registry.rs`; this file adds the
//!      cross-crate angle: run each descriptor's default experiment
//!      through the real `strivo_dataviz::run` end to end);
//!  (b) hydrating a corpus from a populated in-memory `SignalStore`
//!      (reusing `strivo_web::corpus::hydrate_corpus`, exactly as the new
//!      `dataviz_experiment_run` handler does) and running an experiment
//!      over it produces a correct, non-empty `Series` — the actual
//!      "hydrate + run" path the new endpoint wires up server-side; and
//!  (c) the new routes are registered in `routes::plugins::router()`,
//!      auth-gated the same way as their siblings, and the compiled
//!      spa.js asset consumes the registry endpoint instead of the old
//!      hardcoded experiment list.

use strivo_core::signal_store::{NewSignal, SignalStore};
use strivo_dataviz::Experiment;
use strivo_web::corpus::{hydrate_corpus, CorpusScope, RecordingMeta};
use strivo_web::experiment_registry::list_experiments;

fn segment(recording_id: &str, speaker: &str, text: &str, start: f64, end: f64) -> NewSignal {
    NewSignal {
        recording_id: recording_id.to_string(),
        t_start: start,
        t_end: end,
        kind: "speaker_segment".to_string(),
        label: speaker.to_string(),
        payload: serde_json::json!({ "text": text, "speaker": speaker }),
        confidence: 1.0,
        source_plugin: "crunchr".to_string(),
    }
}

/// Two recordings, two speakers, on one channel — small enough to hand
/// verify the resulting minutes.
fn recordings() -> Vec<RecordingMeta> {
    vec![
        RecordingMeta {
            id: "rec-1".into(),
            title: "Pilot".into(),
            date: "2026-01-15 00:00:00".into(),
            channel: "TalkShow".into(),
            playlist: None,
        },
        RecordingMeta {
            id: "rec-2".into(),
            title: "Sequel".into(),
            date: "2026-02-10 00:00:00".into(),
            channel: "TalkShow".into(),
            playlist: None,
        },
    ]
}

fn seeded_store() -> SignalStore {
    let store = SignalStore::open_in_memory().unwrap();
    store
        .write_signals(&[
            // rec-1: Alice 60s, Bob 30s.
            segment("rec-1", "Alice", "opening remarks", 0.0, 60.0),
            segment("rec-1", "Bob", "a reply", 60.0, 90.0),
            // rec-2: Alice another 90s.
            segment("rec-2", "Alice", "the sequel monologue", 0.0, 90.0),
        ])
        .unwrap();
    store
}

#[test]
fn experiment_registry_hydrate_and_run_produces_speaker_minutes_series() {
    // Mirrors exactly what `routes::plugins::dataviz_experiment_run` does:
    // hydrate a corpus server-side from the signal store, scoped by
    // channel, then run the chosen experiment straight over it — no
    // client round-trip of the corpus JSON.
    let store = seeded_store();
    let recs = recordings();

    let corpus = hydrate_corpus(
        &store,
        &recs,
        &CorpusScope::Channel {
            name: "TalkShow".into(),
        },
        None,
    )
    .expect("hydrate_corpus should succeed against a populated store");

    assert_eq!(corpus.episodes.len(), 2, "both recordings are in scope");

    let series = strivo_dataviz::run(&corpus, &Experiment::SpeakerTime);

    assert!(!series.points.is_empty(), "series must not be empty");
    let minutes: std::collections::HashMap<&str, f64> = series
        .points
        .iter()
        .map(|p| (p.label.as_str(), p.value))
        .collect();
    // Alice: 60s (rec-1) + 90s (rec-2) = 150s = 2.5 min.
    assert!(
        (minutes.get("Alice").copied().unwrap_or(0.0) - 2.5).abs() < 1e-9,
        "expected Alice at 2.5 minutes, got {minutes:?}"
    );
    // Bob: 30s = 0.5 min.
    assert!(
        (minutes.get("Bob").copied().unwrap_or(0.0) - 0.5).abs() < 1e-9,
        "expected Bob at 0.5 minutes, got {minutes:?}"
    );
}

#[test]
fn experiment_registry_every_descriptor_runs_cleanly_against_a_hydrated_corpus() {
    // Every experiment the registry advertises must actually be runnable
    // by strivo_dataviz::run — not just deserializable (unit tests in
    // src/experiment_registry.rs already cover that). Uses the same
    // hydrated corpus as above so this also doubles as end-to-end
    // coverage that the corpus->run seam works for every kind, not only
    // SpeakerTime.
    let store = seeded_store();
    let recs = recordings();
    let corpus = hydrate_corpus(
        &store,
        &recs,
        &CorpusScope::Channel {
            name: "TalkShow".into(),
        },
        None,
    )
    .unwrap();

    for desc in list_experiments() {
        let mut obj = serde_json::Map::new();
        obj.insert("kind".to_string(), serde_json::json!(desc.kind));
        for p in &desc.params {
            obj.insert(p.name.clone(), p.default.clone());
        }
        let exp: Experiment = serde_json::from_value(serde_json::Value::Object(obj))
            .unwrap_or_else(|e| panic!("descriptor {desc:?} failed to parse: {e}"));
        let series = strivo_dataviz::run(&corpus, &exp);
        // Two non-empty episodes with two speakers guarantee every one
        // of the six experiment kinds yields at least one point.
        assert!(
            !series.points.is_empty(),
            "experiment {:?} produced an empty series over a non-empty corpus",
            desc.kind
        );
    }
}

/// The literal source of `crates/strivo-web/src/routes/plugins.rs`, read
/// at test-compile time — any edit to the real file is reflected here.
const PLUGINS_ROUTES_SRC: &str = include_str!("../src/routes/plugins.rs");

fn function_body<'a>(src: &'a str, name: &str) -> &'a str {
    let marker = format!("async fn {name}(");
    let start = src
        .find(&marker)
        .unwrap_or_else(|| panic!("expected to find `{marker}` in plugins.rs"));
    let rest = &src[start..];
    let end = rest[marker.len()..]
        .find("\nasync fn ")
        .map(|i| i + marker.len())
        .unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn experiment_registry_routes_are_registered_in_the_plugin_router() {
    assert!(
        PLUGINS_ROUTES_SRC.contains(
            r#".route(
            "/api/v1/dataviz/experiments",
            get(dataviz_experiments_list),
        )"#
        ),
        "routes::plugins::router() must register GET /api/v1/dataviz/experiments \
         -> dataviz_experiments_list; source text has drifted from the expected \
         registration"
    );
    assert!(
        PLUGINS_ROUTES_SRC.contains(
            r#".route(
            "/api/v1/dataviz/experiment",
            axum::routing::post(dataviz_experiment_run),
        )"#
        ),
        "routes::plugins::router() must register POST /api/v1/dataviz/experiment \
         -> dataviz_experiment_run; source text has drifted from the expected \
         registration"
    );
}

#[test]
fn experiment_registry_endpoints_are_auth_gated_like_their_siblings() {
    const AUTH_GUARD: &str = "authed(&headers, &state)";
    const UNAUTHORIZED: &str = "Problem::unauthorized()";

    let list_fn = function_body(PLUGINS_ROUTES_SRC, "dataviz_experiments_list");
    assert!(
        list_fn.contains(AUTH_GUARD) && list_fn.contains(UNAUTHORIZED),
        "dataviz_experiments_list handler body is missing the `{AUTH_GUARD}` guard \
         (or its 401 response) — got:\n{list_fn}"
    );

    let run_fn = function_body(PLUGINS_ROUTES_SRC, "dataviz_experiment_run");
    assert!(
        run_fn.contains(AUTH_GUARD) && run_fn.contains(UNAUTHORIZED),
        "dataviz_experiment_run handler body is missing the `{AUTH_GUARD}` guard \
         (or its 401 response) — got:\n{run_fn}"
    );
}

#[test]
fn experiment_registry_spa_asset_consumes_the_registry_and_combined_endpoints() {
    // Read the *compiled* asset the server would actually serve, via the
    // same RustEmbed mechanism routes::assets uses.
    let spa = strivo_web::assets::Assets::get("spa.js")
        .expect("spa.js must be embedded in the strivo-web binary");
    let spa_js = std::str::from_utf8(spa.data.as_ref())
        .expect("embedded spa.js must be valid UTF-8");

    assert!(
        spa_js.contains("datavizExperiments") && spa_js.contains("/dataviz/experiments"),
        "compiled spa.js must define API.datavizExperiments and fetch \
         /dataviz/experiments instead of hardcoding the experiment list"
    );
    assert!(
        spa_js.contains("datavizExperimentRun") && spa_js.contains("`/dataviz/experiment`"),
        "compiled spa.js must define API.datavizExperimentRun and POST to \
         the combined hydrate+run /dataviz/experiment endpoint"
    );
    // The old hardcoded catalog must be fully gone, not just unreached.
    assert!(
        !spa_js.contains("DATAVIZ_EXPERIMENTS"),
        "compiled spa.js must no longer define the removed hardcoded \
         DATAVIZ_EXPERIMENTS list"
    );
}
