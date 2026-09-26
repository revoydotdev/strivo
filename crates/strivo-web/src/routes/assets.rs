use axum::extract::{Path, RawQuery};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::assets::Assets;
use crate::server::AppState;

async fn asset(Path(path): Path<String>, RawQuery(query): RawQuery) -> Response {
    match Assets::get(&path) {
        Some(content) => {
            let mime = mime_for(&path);
            // Only a content-versioned URL (`?v=<hash>`) is safe to cache
            // forever, because a deploy changes the hash and therefore the URL.
            // Anything fetched without a version — notably the ES modules that
            // `spa.js` imports by relative path — must revalidate, or a deploy
            // would leave browsers on an indefinitely stale copy.
            let cache = if query.is_some_and(|q| q.starts_with("v=") || q.contains("&v=")) {
                "public, max-age=31536000, immutable"
            } else {
                "no-cache"
            };
            (
                [(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, cache)],
                content.data.into_owned(),
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Short content hash of the SPA assets, used to cache-bust the shell's
/// asset URLs on every deploy that changes them. Cheap std hash over the
/// embedded bytes — no extra crate, and the bytes are fixed at build time.
fn assets_version() -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for name in ["spa.js", "spa.css"] {
        if let Some(c) = Assets::get(name) {
            c.data.hash(&mut h);
        }
    }
    format!("{:x}", h.finish())
}

fn mime_for(path: &str) -> &'static str {
    if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".js") || path.ends_with(".mjs") {
        "application/javascript; charset=utf-8"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".png") {
        "image/png"
    } else {
        "application/octet-stream"
    }
}

async fn spa_shell() -> Response {
    match Assets::get("spa.html") {
        Some(content) => {
            let v = assets_version();
            let html = String::from_utf8_lossy(&content.data)
                .replace("/assets/spa.css", &format!("/assets/spa.css?v={v}"))
                .replace("/assets/spa.js", &format!("/assets/spa.js?v={v}"));
            (
                [
                    (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                    // The shell itself must always be revalidated so the
                    // freshly-hashed asset URLs reach the browser.
                    (header::CACHE_CONTROL, "no-cache"),
                ],
                html,
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/assets/{*path}", get(asset))
        // The SPA is the webui. Served at both `/` and `/app`; the legacy
        // askama/htmx page routers are no longer mounted (see server.rs).
        .route("/", get(spa_shell))
        .route("/app", get(spa_shell))
}

#[cfg(test)]
mod tests {
    use super::{asset, get, mime_for, StatusCode};
    use axum::http::header;
    use axum::Router;

    /// A JS content type is load-bearing, not cosmetic: a browser refuses to
    /// execute an `application/octet-stream` ES module, and reports it the same
    /// way it reports a 404.
    #[test]
    fn mjs_served_with_a_javascript_content_type() {
        assert_eq!(
            mime_for("vendor/mediabunny.min.mjs"),
            "application/javascript; charset=utf-8"
        );
    }

    /// The half of the `.mjs` rule that holds in *both* editions: whatever
    /// `mime_for` decides reaches the wire. Asserting `mime_for` alone would
    /// pass even if the handler stopped consulting it, and a browser refuses
    /// an ES module whose response is not a JavaScript content type — a hard
    /// failure that looks exactly like a 404.
    #[tokio::test]
    async fn the_content_type_the_handler_decides_is_the_one_on_the_wire() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        for (path, expected) in [
            ("/assets/spa.js", mime_for("spa.js")),
            ("/assets/spa.css", mime_for("spa.css")),
        ] {
            let response = Router::<()>::new()
                .route("/assets/{*path}", get(asset))
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            let served = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            assert_eq!(served, expected, "{path}");
            assert!(
                served.starts_with("text/") || served.starts_with("application/javascript"),
                "{path} is served as {served:?}, which a browser will not execute or render"
            );
        }
    }

    /// Every vendored library, over the real router, in the edition that ships
    /// them — including the fact that the bytes actually contain the export the
    /// Creator timeline imports. A wrong filename or a typo'd export name is a
    /// 404 or a link-time `SyntaxError` that nothing else in the tree fetches,
    /// so it would otherwise surface only when a creator opened the timeline.
    #[cfg(feature = "creator")]
    #[tokio::test]
    async fn every_vendored_library_is_served_with_a_javascript_content_type_and_its_export() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        // (served path, the token the artifact must actually expose). Read
        // off the shipped bytes, not off the library's documentation:
        // mediabunny is a real ES module with named exports; wavesurfer's ESM
        // build has a *default* export and nothing else, so `WaveSurfer` is the
        // name the importer binds and appears nowhere in the file; x6 is a UMD
        // bundle whose only export is the `globalThis.X6` it attaches.
        const VENDORED: [(&str, &str); 3] = [
            ("/assets/vendor/mediabunny.min.mjs", "as canDecodeVideo"),
            ("/assets/vendor/x6.umd.js", ").X6="),
            ("/assets/vendor/wavesurfer.esm.js", "as default"),
        ];
        for (path, symbol) in VENDORED {
            let response = Router::<()>::new()
                .route("/assets/{*path}", get(asset))
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path} must resolve");
            let content_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            assert!(
                content_type.starts_with("text/javascript")
                    || content_type.starts_with("application/javascript"),
                "{path} is served as {content_type:?}; a browser will refuse to execute it"
            );
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let text = String::from_utf8_lossy(&bytes);
            assert!(
                text.contains(symbol),
                "{path} does not mention {symbol:?}, so the import in the Creator \
                 timeline names something the artifact does not expose"
            );
        }
    }

    /// The other half of the edition contract, and the reason the assertion
    /// above can be creator-only: a PVR build must not so much as serve the
    /// vendor tree. `build.rs` deletes it from `$OUT_DIR`, so these are 404s
    /// with an empty body — not a redirect, not a placeholder.
    #[cfg(not(feature = "creator"))]
    #[tokio::test]
    async fn a_pvr_build_serves_no_vendor_asset_at_all() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        for path in [
            "/assets/vendor/mediabunny.min.mjs",
            "/assets/vendor/x6.umd.js",
            "/assets/vendor/wavesurfer.esm.js",
        ] {
            let response = Router::<()>::new()
                .route("/assets/{*path}", get(asset))
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
    }

    /// An octet-streamed SVG is *downloaded* rather than rendered, so the
    /// missing glyph is silent. The DataViz charts are inline `<svg>` and
    /// never fetched; this covers the served icon set, and is the branch a
    /// future chart export would fall under.
    #[test]
    fn svg_served_as_an_image_not_as_an_octet_stream() {
        assert_eq!(mime_for("icons/candy/settings.svg"), "image/svg+xml");
    }
}
