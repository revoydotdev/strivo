//! The preview cache is bounded, and the bound is exercised at the bound.
//!
//! A cache with a capacity and no test that fills it past that capacity is a
//! cache with an unverified eviction path. These tests sit exactly on
//! `PREVIEW_CACHE_ENTRIES` rather than near it, and they check the *files* are
//! deleted on eviction as well as the map entries, because an evicted entry
//! whose file survives is the same unbounded growth one level down.

use strivo_core::media::preview::{PreviewCache, PREVIEW_CACHE_ENTRIES};

fn scratch(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, b"frame bytes").expect("a scratch file");
    path
}

#[test]
fn the_cache_holds_exactly_its_bound_and_no_more() {
    let dir = tempfile::tempdir().expect("a scratch dir");
    let mut cache = PreviewCache::new(PREVIEW_CACHE_ENTRIES);
    assert_eq!(cache.capacity(), PREVIEW_CACHE_ENTRIES);

    // Fill to exactly the bound.
    for i in 0..PREVIEW_CACHE_ENTRIES {
        cache.insert(format!("k{i}"), scratch(dir.path(), &format!("f{i}.png")));
        assert_eq!(
            cache.len(),
            i + 1,
            "the cache must hold every entry up to the bound"
        );
    }
    assert_eq!(
        cache.len(),
        PREVIEW_CACHE_ENTRIES,
        "the bound must be reachable, not theoretical"
    );
    assert!(
        cache.contains("k0"),
        "the oldest entry is still inside the bound"
    );

    // One past the bound: the least-recently-used entry goes, the file with it.
    let evicted_path = scratch(dir.path(), "overflow.png");
    let evicted = cache.insert("overflow".into(), evicted_path.clone());
    assert_eq!(
        cache.len(),
        PREVIEW_CACHE_ENTRIES,
        "the cache must not exceed its bound"
    );
    assert_eq!(
        evicted.len(),
        1,
        "exactly one entry is evicted per insertion past the bound"
    );
    assert!(
        !cache.contains("k0"),
        "k0 was the least recently used and must be gone"
    );
    assert!(
        !evicted[0].exists(),
        "the EVICTED frame's file must be deleted, not orphaned; evicted was {}",
        evicted[0].display()
    );
    assert!(
        evicted_path.exists(),
        "the newly inserted frame's file is not the evicted one"
    );
    assert!(
        cache.contains("overflow"),
        "the newest entry is always retained"
    );
}

#[test]
fn touching_an_entry_protects_it_from_eviction() {
    let dir = tempfile::tempdir().expect("a scratch dir");
    let mut cache = PreviewCache::new(3);
    cache.insert("a".into(), scratch(dir.path(), "a.png"));
    cache.insert("b".into(), scratch(dir.path(), "b.png"));
    cache.insert("c".into(), scratch(dir.path(), "c.png"));
    // Scrub back over the oldest: it is now the most recently used.
    assert!(cache.get("a").is_some());
    cache.insert("d".into(), scratch(dir.path(), "d.png"));
    assert!(cache.get("a").is_some(), "a touched entry must survive");
    assert!(
        cache.get("b").is_none(),
        "the untouched middle entry is the one evicted"
    );
    assert_eq!(cache.len(), 3);
}

#[test]
fn a_repeat_insertion_does_not_grow_the_cache_or_evict() {
    let dir = tempfile::tempdir().expect("a scratch dir");
    let mut cache = PreviewCache::new(2);
    cache.insert("a".into(), scratch(dir.path(), "a.png"));
    cache.insert("b".into(), scratch(dir.path(), "b.png"));
    let evicted = cache.insert("a".into(), scratch(dir.path(), "a2.png"));
    assert!(
        evicted.is_empty(),
        "re-inserting an existing key evicts nothing"
    );
    assert_eq!(cache.len(), 2);
    assert!(
        cache.get("b").is_some(),
        "a re-insertion must not evict the other entry"
    );
}

#[test]
fn a_zero_capacity_is_refused_rather_than_silently_disabled() {
    let dir = tempfile::tempdir().expect("a scratch dir");
    let mut cache = PreviewCache::new(0);
    assert_eq!(
        cache.capacity(),
        1,
        "a cache asked to hold nothing holds one, not everything"
    );
    cache.insert("a".into(), scratch(dir.path(), "a.png"));
    cache.insert("b".into(), scratch(dir.path(), "b.png"));
    assert_eq!(
        cache.len(),
        1,
        "even the refused capacity must bound the cache"
    );
}

#[test]
fn forgetting_an_entry_does_not_delete_the_file() {
    // `forget` exists for the case where the file went missing underneath the
    // cache. Deleting on forget would be harmless there, but the two operations
    // are different and conflating them would make the re-decode path delete a
    // file it is about to rewrite.
    let dir = tempfile::tempdir().expect("a scratch dir");
    let mut cache = PreviewCache::new(2);
    let path = scratch(dir.path(), "a.png");
    cache.insert("a".into(), path.clone());
    cache.forget("a");
    assert_eq!(cache.len(), 0);
    assert!(
        path.exists(),
        "forgetting an entry must not delete its file"
    );
}
