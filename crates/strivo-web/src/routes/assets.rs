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
    } else if path.ends_with(".js") {
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
