// Creator Edition only: `/api/v1/dataviz/corpus` and its plugin router live
// behind the `creator` feature (see server.rs: `routes::plugins::router()`
// is only merged `#[cfg(feature = "creator")]`).
#![cfg(feature = "creator")]
//! Non-vacuous coverage for the `GET /api/v1/dataviz/corpus` wiring.
//!
//! `AppState` needs a live `IpcClient` (real daemon PID file + Unix socket
//! via `strivo_core::ipc::is_daemon_running()`, not overridable from a
//! test), so a real `.oneshot()` HTTP dispatch through
//! `routes::plugins::router().with_state(state)` isn't achievable from this
//! external test crate — `tests/routes.rs::router_empty_404s` documents the
//! same constraint. Instead we prove:
//!
//!  (a) the route is actually registered in `routes::plugins::router()` and
//!      the handler is auth-gated the same way its sibling `dataviz_run`
//!      handler is, by reading the real source text at test time; and
//!  (b) the *compiled, embedded* spa.js (the exact bytes the server would
//!      serve) was updated to call the endpoint instead of hand-assembling
//!      the corpus client-side from Crunchr transcripts.

/// The literal source of `crates/strivo-web/src/routes/plugins.rs`, read at
/// test-compile time. Any edit to the real file is reflected here — this is
/// not a hardcoded snapshot.
const PLUGINS_ROUTES_SRC: &str = include_str!("../src/routes/plugins.rs");

/// Slice out the body of a top-level `async fn <name>(` in `src`, from its
/// signature up to (but excluding) the next top-level `async fn `. Panics
/// with a helpful message if `name` isn't found, so a rename fails loudly
/// instead of the test silently checking nothing.
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
fn corpus_endpoint_route_is_registered_in_the_plugin_router() {
    assert!(
        PLUGINS_ROUTES_SRC
            .contains(r#".route("/api/v1/dataviz/corpus", get(dataviz_corpus))"#),
        "routes::plugins::router() must register GET /api/v1/dataviz/corpus \
         -> dataviz_corpus; source text has drifted from the expected \
         registration line"
    );
}

#[test]
fn corpus_endpoint_route_handler_is_auth_gated_like_its_sibling() {
    let corpus_fn = function_body(PLUGINS_ROUTES_SRC, "dataviz_corpus");
    let run_fn = function_body(PLUGINS_ROUTES_SRC, "dataviz_run");

    // Both handlers must guard on the same auth check before doing any
    // work — proves dataviz_corpus isn't just present but wired the same
    // way as the rest of the auth-gated plugin surface.
    const AUTH_GUARD: &str = "authed(&headers, &state)";
    const UNAUTHORIZED: &str = "Problem::unauthorized()";

    assert!(
        corpus_fn.contains(AUTH_GUARD) && corpus_fn.contains(UNAUTHORIZED),
        "dataviz_corpus handler body is missing the `{AUTH_GUARD}` guard \
         (or its 401 response) — got:\n{corpus_fn}"
    );
    assert!(
        run_fn.contains(AUTH_GUARD) && run_fn.contains(UNAUTHORIZED),
        "sibling dataviz_run handler no longer uses the expected auth \
         pattern; the comparison baseline has drifted"
    );
}

#[test]
fn corpus_endpoint_route_spa_asset_consumes_the_endpoint() {
    // Read the *compiled* asset the server would actually serve, not the
    // working-copy file, via the same RustEmbed mechanism
    // routes::assets uses (`Assets::get("spa.js")`).
    let spa = strivo_web::assets::Assets::get("spa.js")
        .expect("spa.js must be embedded in the strivo-web binary");
    let spa_js = std::str::from_utf8(spa.data.as_ref())
        .expect("embedded spa.js must be valid UTF-8");

    assert!(
        spa_js.contains("datavizCorpus"),
        "compiled spa.js must define/call an API.datavizCorpus helper"
    );
    assert!(
        spa_js.contains("/dataviz/corpus"),
        "compiled spa.js must GET /dataviz/corpus somewhere"
    );

    // The old dead path: hand-assembling episodes from per-recording
    // Crunchr transcripts client-side. That call must be fully gone, not
    // just unreached from dz-run — dead code must not remain shipped.
    assert!(
        !spa_js.contains("crunchrTranscript"),
        "compiled spa.js must no longer reference the removed \
         API.crunchrTranscript client-side corpus assembly"
    );
}
