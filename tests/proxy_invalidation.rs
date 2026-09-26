//! Falsification: a source change must invalidate the proxy, a path change
//! must not. Written FIRST, against a deliberately naive store.

mod common;

#[tokio::test]
async fn a_source_change_rebuilds_the_proxy() {
    let dir = tempfile::tempdir().expect("a scratch dir");
    let source = dir.path().join("landmarks.mkv");
    std::fs::copy(common::corpus().join("landmarks.mkv"), &source).expect("fixture copy");

    let before = common::content_fingerprint(&source);
    let spec = strivo_core::media::proxy::ProxySpec::default();
    let store = strivo_core::media::proxy::ProxyStore::new(dir.path().join("cache"), spec.clone());

    let first = store
        .resolve(&source, &before)
        .await
        .expect("first resolve builds a proxy");
    let first_bytes = std::fs::read(&first.path).expect("proxy bytes");
    let first_key = first.key.clone();
    assert!(
        first.rebuilt,
        "the first resolve has nothing to reuse, so it builds"
    );

    // Change the CONTENT, keeping the path, size and mtime as close as a
    // rewrite can. The proxy must be rebuilt, not re-served.
    let mut bytes = std::fs::read(&source).expect("source bytes");
    let n = bytes.len();
    bytes[n / 2] ^= 0xff;
    std::fs::write(&source, &bytes).expect("mutate");

    let after = common::content_fingerprint(&source);
    assert_ne!(
        before, after,
        "the content fingerprint must change with the bytes"
    );

    let rebuilt = store
        .resolve(&source, &after)
        .await
        .expect("resolve after a content change rebuilds");
    assert_ne!(
        first_key, rebuilt.key,
        "a content change must invalidate the proxy cache key"
    );
    assert!(
        rebuilt.rebuilt,
        "the resolve must report that it rebuilt rather than reused"
    );

    let rebuilt_bytes = std::fs::read(&rebuilt.path).expect("rebuilt proxy bytes");
    assert_ne!(
        first_bytes, rebuilt_bytes,
        "the rebuilt proxy must be different bytes, not the stale artifact re-served"
    );

    // The pre-change artifact must not be what a later interaction is handed
    // under the pre-change key.
    let stale = store
        .resolve(&source, &before)
        .await
        .expect("asking for the old key still yields a usable proxy");
    assert!(std::path::Path::new(&stale.path).exists());
}

#[tokio::test]
async fn a_path_change_keeps_the_proxy() {
    let dir = tempfile::tempdir().expect("a scratch dir");
    let source = dir.path().join("landmarks.mkv");
    std::fs::copy(common::corpus().join("landmarks.mkv"), &source).expect("fixture copy");

    let key = common::content_fingerprint(&source);
    let spec = strivo_core::media::proxy::ProxySpec::default();
    let store = strivo_core::media::proxy::ProxyStore::new(dir.path().join("cache"), spec.clone());

    let first = store
        .resolve(&source, &key)
        .await
        .expect("first resolve builds a proxy");
    let first_key = first.key.clone();

    let moved = dir.path().join("renamed.mkv");
    std::fs::rename(&source, &moved).expect("rename");
    let after_move = store
        .resolve(&moved, &key)
        .await
        .expect("resolve after a rename");
    assert_eq!(
        first_key, after_move.key,
        "a path change must not invalidate a proxy of identical content"
    );
    assert!(
        !after_move.rebuilt,
        "identical content under a new path must reuse the artifact, not rebuild it"
    );
}
