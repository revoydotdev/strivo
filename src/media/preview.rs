//! The interaction surface: bounded preview caching over the proxy, plus the
//! pyramid and filmstrip an interaction reads beside it.
//!
//! Every read here goes through a proxy. That is the whole design: a caller
//! holding a [`PreviewSurface`] cannot reach a user's source, because the
//! surface takes the source and its content fingerprint and hands the caller
//! bytes decoded from the proxy. The `Must NOT` is therefore a property of the
//! surface's shape *and* is independently observable through
//! [`crate::media::spawn::snapshot`].
//!
//! The decoded-frame cache is **bounded**. [`PREVIEW_CACHE_ENTRIES`] is the
//! bound, eviction is least-recently-used, and an evicted frame's file is
//! deleted rather than left for the next run to find. An unbounded frame cache
//! on a six-hour source is a disk leak, not a cache.

use anyhow::Result;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use super::proxy::{self, ProxyArtifact, ProxySpec, ProxyStore};
use super::pyramid::{self, Filmstrip, WaveformPyramid};

/// Decoded preview frames retained. The stated bound.
///
/// At roughly 40 KB per 640-wide PNG this is a few megabytes, and it is
/// deliberately smaller than any plausible working set for one viewport, so
/// the eviction path is exercised in normal use rather than only in a test.
pub const PREVIEW_CACHE_ENTRIES: usize = 64;

/// A bounded, least-recently-used cache of decoded frames on disk.
///
/// `HashMap` for lookup and a `VecDeque` for order, rather than an ordered map
/// type, because the operations are a lookup, an insert and a pop-oldest and
/// nothing in between.
pub struct PreviewCache {
    capacity: usize,
    entries: HashMap<String, PathBuf>,
    order: VecDeque<String>,
}

impl PreviewCache {
    /// A cache holding at most `capacity` frames. A capacity of zero is
    /// refused rather than silently disabling the cache, because a caller who
    /// asked for zero almost certainly meant a number they did not compute.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The cached path for `key`, if present. A hit moves the key to the back
    /// of the eviction order, so a frame a user is scrubbing over stays.
    pub fn get(&mut self, key: &str) -> Option<PathBuf> {
        let path = self.entries.get(key)?.clone();
        self.touch(key);
        Some(path)
    }

    /// Forget `key` without deleting its file. Used when the file went missing
    /// underneath the cache, so the entry is re-decoded rather than re-served
    /// as a path that does not exist.
    pub fn forget(&mut self, key: &str) {
        self.entries.remove(key);
        if let Some(at) = self.order.iter().position(|k| k == key) {
            self.order.remove(at);
        }
    }

    /// Whether `key` is held, WITHOUT changing the eviction order. A caller
    /// inspecting the cache must not thereby protect its oldest entry — using
    /// [`Self::get`] to look would silently reorder it.
    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Record `path` under `key`, evicting the least-recently-used entry if
    /// that would exceed the bound. The evicted file is removed: the cache
    /// owns the bytes it points at.
    pub fn insert(&mut self, key: String, path: PathBuf) -> Vec<PathBuf> {
        if self.entries.contains_key(&key) {
            self.entries.insert(key.clone(), path);
            self.touch(&key);
            return Vec::new();
        }
        self.entries.insert(key.clone(), path);
        self.order.push_back(key);
        let mut evicted = Vec::new();
        while self.entries.len() > self.capacity {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(path) = self.entries.remove(&oldest) {
                let _ = std::fs::remove_file(&path);
                evicted.push(path);
            }
        }
        evicted
    }

    /// Drop everything, deleting the files. Used by tests and by a caller
    /// discarding a source's previews.
    pub fn clear(&mut self) {
        for (_, path) in self.entries.drain() {
            let _ = std::fs::remove_file(&path);
        }
        self.order.clear();
    }

    fn touch(&mut self, key: &str) {
        if let Some(at) = self.order.iter().position(|k| k == key) {
            let k = self
                .order
                .remove(at)
                .expect("the position came from the deque");
            self.order.push_back(k);
        }
    }
}

/// Everything an interaction path needs, and the only way to get it.
///
/// Constructed once per cache root. A source is addressed by
/// (`path`, `content fingerprint`) — the fingerprint is taken as an argument
/// rather than computed here, because todo 33 owns the identity rule and a
/// cheaper rule computed in this module would be a second answer to the same
/// question.
pub struct PreviewSurface {
    store: ProxyStore,
    cache: PreviewCache,
    scratch: PathBuf,
}

impl PreviewSurface {
    pub fn new(root: impl Into<PathBuf>, spec: ProxySpec) -> Result<Self> {
        let root = root.into();
        let scratch = root.join("frames");
        std::fs::create_dir_all(&scratch)
            .map_err(|e| anyhow::anyhow!("create {}: {e}", scratch.display()))?;
        Ok(Self {
            store: ProxyStore::new(root, spec),
            cache: PreviewCache::new(PREVIEW_CACHE_ENTRIES),
            scratch,
        })
    }

    pub fn store(&self) -> &ProxyStore {
        &self.store
    }

    pub fn cache(&mut self) -> &mut PreviewCache {
        &mut self.cache
    }

    /// The proxy for this source, building it if the fingerprint is new.
    pub async fn proxy(&self, source: &Path, fingerprint: &str) -> Result<ProxyArtifact> {
        self.store.resolve(source, fingerprint).await
    }

    /// A decoded frame at `original_secs`, through the proxy.
    ///
    /// The cache key carries the proxy key, the width and the time, so a frame
    /// decoded for one source is never handed for another and a width change
    /// is a different entry.
    pub async fn frame(
        &mut self,
        source: &Path,
        fingerprint: &str,
        original_secs: f64,
        width: u32,
    ) -> Result<PathBuf> {
        let proxy = self.proxy(source, fingerprint).await?;
        let key = format!("{}|{width}|{original_secs:.3}", proxy.key);
        if let Some(hit) = self.cache.get(&key) {
            if hit.exists() {
                return Ok(hit);
            }
            // The cache is the authority on membership, so a file that vanished
            // underneath it is re-decoded rather than re-served as a path that
            // does not exist.
            self.cache.forget(&key);
        }
        let path = proxy::frame_from_proxy(&proxy, original_secs, width, &self.scratch).await?;
        self.cache.insert(key, path.clone());
        Ok(path)
    }

    /// The waveform pyramid for this source, built once per fingerprint and
    /// cached beside the proxy.
    pub async fn waveform(&self, source: &Path, fingerprint: &str) -> Result<WaveformPyramid> {
        let proxy = self.proxy(source, fingerprint).await?;
        let cached = self.store.key(fingerprint).replace(['@', ':'], "_");
        let path = proxy
            .path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("the proxy has no parent directory"))?
            .join(format!("{cached}.waveform.json"));
        if let Ok(raw) = std::fs::read(&path) {
            if let Ok(pyramid) = WaveformPyramid::from_json(&raw) {
                return Ok(pyramid);
            }
        }
        let pyramid = pyramid::build_pyramid(&proxy).await?;
        let bytes = pyramid.to_json()?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &path)?;
        Ok(pyramid)
    }

    /// A filmstrip for this source, built once per fingerprint and shape.
    pub async fn filmstrip(
        &self,
        source: &Path,
        fingerprint: &str,
        tiles: u32,
        tile_width: u32,
        columns: u32,
    ) -> Result<Filmstrip> {
        let proxy = self.proxy(source, fingerprint).await?;
        if proxy.path.exists() {
            let stem = proxy
                .path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("proxy");
            let dir = proxy.path.parent().expect("the proxy path has a parent");
            let cached = dir.join(format!("{stem}.filmstrip-{tiles}x{columns}.png"));
            if cached.exists() {
                let columns = columns.clamp(1, tiles);
                return Ok(Filmstrip {
                    path: cached,
                    tiles,
                    columns,
                    rows: tiles.div_ceil(columns),
                    tile_width,
                    interval_secs: interval_for(proxy.duration_secs, tiles),
                });
            }
        }
        pyramid::build_filmstrip(&proxy, proxy.duration_secs, tiles, tile_width, columns).await
    }
}

/// Seconds between consecutive filmstrip tiles. The proxy's own measured
/// duration is the numerator, so the strip and the seek map agree.
fn interval_for(duration_secs: f64, tiles: u32) -> f64 {
    if duration_secs > 0.0 {
        duration_secs / tiles.max(1) as f64
    } else {
        0.0
    }
}
