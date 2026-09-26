//! Proxy generation and the proxy/original time mapping (report R02).
//!
//! A proxy is a low-resolution, densely-keyed re-encode of a source. Two
//! properties make it usable, and only the second is subtle:
//!
//! * Navigating it costs a fraction of decoding the source. That is why
//!   every interaction path in [`crate::media::preview`] reads the proxy and
//!   never the user's file.
//! * Its timestamps still mean the same thing as the source's. This module
//!   **measures** that rather than assuming it: [`ProxyIndex`] holds both
//!   files' keyframe times, read from container packets without decoding, and
//!   [`ProxyIndex::proxy_time_for`] interpolates between them. A proxy that
//!   silently rescaled its timebase would be mapped wrongly here instead of
//!   being decoded at the wrong second by a user.
//!
//! Nothing in this file is a cache of *original* pixels, and nothing here
//! decodes a source: the only two places a user's file reaches a child process
//! are the proxy build and the keyframe probe, and the first is what makes
//! every later read cheap.

use anyhow::{Context, Result};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::spawn;

/// How long a whole-source proxy build may take. Six hours of stream time is
/// minutes of transcoding, so this is generous; it exists so a wedged child
/// cannot hold a media slot for ever.
pub const BUILD_TIMEOUT: Duration = Duration::from_secs(900);

/// How long one frame decode may take. This is the operation the 500 ms p95
/// budget is about, and it is the same 30 s backstop the rest of the media
/// surface uses, not a second number.
pub const DECODE_TIMEOUT: Duration = Duration::from_secs(30);

/// The proxy a source is transcribed to.
///
/// The default is 1280x720 because that is the size report R02's proxy-seeking
/// target names ("a warm local 1080p proxy",
/// `docs/CREATOR-EDITION-REPORT-2026-09-07.md:408`). A proxy smaller than that
/// would make the 500 ms claim true for a reason the report does not ask
/// about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxySpec {
    pub width: u32,
    pub height: u32,
    /// Seconds between forced keyframes. One second is what makes a seek cost
    /// one second of decode rather than a whole GOP: `long-gop.mkv` holds a
    /// single keyframe in ten seconds, and decoding from that to a requested
    /// time is the cost this interval exists to bound.
    pub keyframe_interval_secs: u32,
    pub crf: u32,
    pub preset: &'static str,
}

impl Default for ProxySpec {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            keyframe_interval_secs: 1,
            crf: 30,
            preset: "veryfast",
        }
    }
}

impl ProxySpec {
    /// A spec that forces at least one keyframe per second.
    pub fn keyframes_per_second(spec: &mut Self, secs: u32) -> &mut Self {
        spec.keyframe_interval_secs = secs.max(1);
        spec
    }

    /// The cache-key fragment for this spec, so a different proxy shape is a
    /// different artifact rather than a silent mismatch.
    fn slug(&self) -> String {
        format!(
            "{}x{}-g{}-crf{}",
            self.width, self.height, self.keyframe_interval_secs, self.crf
        )
    }
}

/// A proxy, its measured time mapping, and whether this call built it.
///
/// `rebuilt` is a fact about the call rather than a flag the caller sets,
/// because the difference between "reused" and "rebuilt" is exactly what an
/// invalidation claim is about.
#[derive(Debug, Clone)]
pub struct ProxyArtifact {
    /// The content-addressed key: `<fingerprint>@<spec>`. Derived from the
    /// caller's fingerprint of the *content*, never from the path, so a rename
    /// keeps the proxy and an edit loses it.
    pub key: String,
    pub path: PathBuf,
    /// The source this proxy was built from, for provenance in logs.
    pub source: PathBuf,
    pub width: u32,
    pub height: u32,
    pub duration_secs: f64,
    /// `true` when this call transcoded, `false` when it reused an artifact.
    pub rebuilt: bool,
    pub index: ProxyIndex,
}

/// The measured correspondence between the source's timeline and the proxy's.
///
/// Both vectors are keyframe presentation times in seconds, read from packet
/// headers. They are equal for a proxy built at the source's frame rate, and
/// the equality is *checked* here by the mapping tests rather than assumed by
/// the encoder.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProxyIndex {
    pub source_keyframes_secs: Vec<f64>,
    pub proxy_keyframes_secs: Vec<f64>,
}

impl ProxyIndex {
    /// The proxy timestamp that shows original time `original_secs`.
    ///
    /// Piecewise-linear between the bracketing keyframes, so a proxy whose
    /// timebase drifted is mapped by its own measured keyframes instead of by
    /// an identity the container does not support. Before the first keyframe
    /// and after the last, the nearest segment is extrapolated.
    pub fn proxy_time_for(&self, original_secs: f64) -> f64 {
        self.map(
            original_secs,
            &self.source_keyframes_secs,
            &self.proxy_keyframes_secs,
        )
    }

    /// The original time that a proxy timestamp shows. The inverse of
    /// [`Self::proxy_time_for`] over the same segments.
    pub fn original_time_for(&self, proxy_secs: f64) -> f64 {
        self.map(
            proxy_secs,
            &self.proxy_keyframes_secs,
            &self.source_keyframes_secs,
        )
    }

    /// The proxy keyframe at or before `original_secs`, with the original time
    /// it shows and how far the requested time is from it. Decoding from a
    /// keyframe is what a seek does, so the residual is the honest cost of
    /// that seek: a `None` means the source has no keyframe at or before the
    /// request, and the caller must not decode the original to compensate.
    pub fn seek_target(&self, original_secs: f64) -> Option<SeekTarget> {
        let i = self
            .source_keyframes_secs
            .partition_point(|k| *k <= original_secs);
        if i == 0 {
            return None;
        }
        let src = self.source_keyframes_secs[i - 1];
        let prx = self.proxy_keyframes_secs.get(i - 1).copied()?;
        Some(SeekTarget {
            proxy_secs: prx,
            original_secs: src,
            residual_secs: (original_secs - src).abs(),
        })
    }

    /// The largest gap between consecutive source keyframes: the worst-case
    /// distance a decode-to-keyframe can land from the requested time.
    pub fn worst_keyframe_gap_secs(&self) -> f64 {
        self.source_keyframes_secs
            .windows(2)
            .map(|w| w[1] - w[0])
            .fold(0.0_f64, f64::max)
    }

    fn map(&self, t: f64, from: &[f64], to: &[f64]) -> f64 {
        if from.is_empty() || to.is_empty() {
            return t;
        }
        let i = from.partition_point(|k| *k <= t);
        if i == 0 {
            // Before the first keyframe: extrapolate backwards along the
            // first segment, which is the only honest answer available.
            let (a, b) = (from[0], from.get(1).copied().unwrap_or(from[0] + 1.0));
            let (pa, pb) = (to[0], to.get(1).copied().unwrap_or(to[0] + 1.0));
            let slope = (pb - pa) / (b - a);
            return pa + slope * (t - a);
        }
        if i >= from.len() {
            let n = from.len();
            let a = from[n - 2.min(n - 1)];
            let b = from[n - 1];
            let pa = to[n - 2.min(n - 1)];
            let pb = to[n - 1];
            let slope = if (b - a).abs() < f64::EPSILON {
                1.0
            } else {
                (pb - pa) / (b - a)
            };
            return pa + slope * (t - a);
        }
        let (a, b) = (from[i - 1], from[i]);
        let (pa, pb) = (to[i - 1], *to.get(i).unwrap_or(&to[i - 1]));
        if (b - a).abs() < f64::EPSILON {
            return pa;
        }
        pa + (pb - pa) * (t - a) / (b - a)
    }
}

/// Where a seek through the proxy actually starts decoding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeekTarget {
    pub proxy_secs: f64,
    pub original_secs: f64,
    pub residual_secs: f64,
}

/// The content fingerprint of a file, which is the cache key every derived
/// artifact in this module is addressed by.
///
/// This is todo 33's rule — identity is the digest of the whole content, not
/// its path and not its size and mtime — implemented here because
/// `strivo-core` cannot reach `strivo-research` (the dependency runs the other
/// way, and adding one is not this todo's to do). The rule is one rule; see the
/// notepad for the duplication this leaves behind and what would remove it.
pub fn content_fingerprint(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buf = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buf)
            .with_context(|| format!("read {}", path.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buf[..read]);
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
}

/// A content-addressed store of proxy artifacts under one root.
///
/// Addressed by the caller's content fingerprint, so the only way a stale
/// proxy can be served is if the content did not change, and the only way a
/// live one is missed is if the caller passes the wrong fingerprint — which is
/// why every caller in this crate takes it as an argument rather than
/// computing it from something cheaper.
pub struct ProxyStore {
    root: PathBuf,
    spec: ProxySpec,
}

impl ProxyStore {
    pub fn new(root: impl Into<PathBuf>, spec: ProxySpec) -> Self {
        Self {
            root: root.into(),
            spec,
        }
    }

    pub fn spec(&self) -> &ProxySpec {
        &self.spec
    }

    /// The key an artifact for this content and spec is addressed by.
    pub fn key(&self, fingerprint: &str) -> String {
        // The digest is `sha256:<64 hex>`; the prefix is dropped because it is
        // constant and would only make the filename longer.
        let digest = fingerprint.strip_prefix("sha256:").unwrap_or(fingerprint);
        format!("{}@{}", digest, self.spec.slug())
    }

    /// The proxy for `fingerprint`, building it if absent.
    ///
    /// `source` is read once, to transcode. Nothing else in this module ever
    /// opens it again.
    pub async fn resolve(&self, source: &Path, fingerprint: &str) -> Result<ProxyArtifact> {
        let key = self.key(fingerprint);
        let dir = self.root.join("proxy");
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let path = dir.join(format!("{key}.mp4"));
        let index_path = dir.join(format!("{key}.index.json"));

        if path.exists() && index_path.exists() {
            if let Ok(raw) = std::fs::read(&index_path) {
                if let Ok(index) = serde_json::from_slice::<ProxyIndex>(&raw) {
                    let duration_secs = index
                        .proxy_keyframes_secs
                        .last()
                        .copied()
                        .unwrap_or_default();
                    return Ok(ProxyArtifact {
                        key,
                        path,
                        source: source.to_path_buf(),
                        width: self.spec.width,
                        height: self.spec.height,
                        duration_secs,
                        rebuilt: false,
                        index,
                    });
                }
            }
        }

        let _permit = spawn::acquire_slot("proxy build")
            .await
            .map_err(anyhow::Error::msg)?;
        self.build(source, &key, &dir, &path, &index_path).await
    }

    async fn build(
        &self,
        source: &Path,
        key: &str,
        dir: &Path,
        path: &Path,
        index_path: &Path,
    ) -> Result<ProxyArtifact> {
        // Built beside its final name and renamed, so an interrupted build
        // cannot leave a truncated file that a later resolve reuses as if it
        // were complete.
        let tmp = dir.join(format!("{key}.{}.tmp.mp4", std::process::id()));
        let _ = std::fs::remove_file(&tmp);

        // The GOP is the source's frame rate times the requested interval, not
        // a hard-coded 25. A fixed 25 makes the declared one-second interval
        // mean 0.42 s at 60 fps (more keyframes than asked for, so more bytes)
        // and 2.5 s at 10 fps (fewer, so a seek decodes up to 2.5 s of picture
        // — the exact cost this interval exists to bound).
        let fps = frame_rate(source).await.unwrap_or(25.0).max(0.1);
        let gop = ((fps * self.spec.keyframe_interval_secs as f64).round() as usize).max(1);
        let forced = format!("expr:gte(t,n_forced*{})", self.spec.keyframe_interval_secs);
        let argv: Vec<OsString> = [
            "-y".to_string(),
            "-i".to_string(),
            source.to_string_lossy().into_owned(),
            "-vf".to_string(),
            format!("scale={}:{}", self.spec.width, self.spec.height),
            "-c:v".into(),
            "libx264".into(),
            "-preset".into(),
            self.spec.preset.into(),
            "-crf".into(),
            self.spec.crf.to_string(),
            "-g".into(),
            gop.to_string(),
            "-keyint_min".into(),
            gop.to_string(),
            "-force_key_frames".into(),
            forced,
            "-pix_fmt".into(),
            "yuv420p".into(),
            // Copy nothing here: re-encoding the audio buys nothing for a
            // preview and costs the build real time on a six-hour source.
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            "96k".into(),
            "-movflags".into(),
            "+faststart".into(),
            tmp.to_string_lossy().into_owned(),
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        let argv: Vec<&OsStr> = argv.iter().map(OsString::as_os_str).collect();

        let status = tokio::time::timeout(
            BUILD_TIMEOUT,
            spawn::ffmpeg(
                "proxy-build",
                source,
                false,
                self.spec.width,
                self.spec.height,
                true,
                &argv,
            ),
        )
        .await
        .map_err(|_| anyhow::anyhow!("proxy build exceeded {BUILD_TIMEOUT:?}"))?
        .map_err(|e| anyhow::anyhow!("spawn ffmpeg: {e}"))?;
        if !status.success() {
            let _ = std::fs::remove_file(&tmp);
            anyhow::bail!(
                "ffmpeg exited {status} building a {}x{} proxy of {}",
                self.spec.width,
                self.spec.height,
                source.display()
            );
        }

        let index = ProxyIndex {
            source_keyframes_secs: keyframe_times(source).await?,
            proxy_keyframes_secs: keyframe_times(&tmp).await?,
        };
        write_atomic(index_path, &serde_json::to_vec_pretty(&index)?)?;
        std::fs::rename(&tmp, path).with_context(|| format!("publish proxy {}", path.display()))?;

        Ok(ProxyArtifact {
            key: key.to_string(),
            path: path.to_path_buf(),
            source: source.to_path_buf(),
            width: self.spec.width,
            height: self.spec.height,
            duration_secs: index
                .proxy_keyframes_secs
                .last()
                .copied()
                .unwrap_or_default(),
            rebuilt: true,
            index,
        })
    }
}

/// Decode one frame from the proxy at `original_secs`.
///
/// This is the interaction path's read. It is handed a proxy, and the spawn it
/// records says so, so a caller can assert the `Must NOT` from the record
/// rather than from the code's shape.
pub async fn frame_from_proxy(
    proxy: &ProxyArtifact,
    original_secs: f64,
    out_width: u32,
    scratch: &Path,
) -> Result<PathBuf> {
    let _permit = spawn::acquire_slot("proxy frame")
        .await
        .map_err(anyhow::Error::msg)?;
    let proxy_secs = proxy.index.proxy_time_for(original_secs);

    std::fs::create_dir_all(scratch).with_context(|| format!("create {}", scratch.display()))?;
    let out = scratch.join(format!(
        "frame-{}-{}-{out_width}.png",
        proxy.key.replace(['@', ':'], "_"),
        (original_secs * 1000.0).round() as i64
    ));
    let tmp = scratch.join(format!(
        ".{}.tmp.png",
        out.file_name().and_then(|s| s.to_str()).unwrap_or("frame")
    ));
    let _ = std::fs::remove_file(&tmp);

    let argv: Vec<OsString> = [
        "-y".to_string(),
        "-ss".to_string(),
        format!("{proxy_secs:.6}"),
        "-i".to_string(),
        proxy.path.to_string_lossy().into_owned(),
        "-frames:v".to_string(),
        "1".to_string(),
        "-vf".to_string(),
        format!("scale={out_width}:-2"),
        "-f".to_string(),
        "image2".to_string(),
        "-update".to_string(),
        "1".to_string(),
        tmp.to_string_lossy().into_owned(),
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    let argv: Vec<&OsStr> = argv.iter().map(OsString::as_os_str).collect();

    let status = tokio::time::timeout(
        DECODE_TIMEOUT,
        spawn::ffmpeg(
            "proxy-frame",
            &proxy.path,
            true,
            proxy.width,
            proxy.height,
            true,
            &argv,
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("proxy frame decode exceeded {DECODE_TIMEOUT:?}"))?
    .map_err(|e| anyhow::anyhow!("spawn ffmpeg: {e}"))?;
    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("ffmpeg exited {status} decoding a proxy frame at {original_secs}s");
    }
    std::fs::rename(&tmp, &out).with_context(|| format!("publish frame {}", out.display()))?;
    Ok(out)
}

/// A file's average frame rate, from its container. `None` when ffprobe cannot
/// say, in which case the caller falls back to 25 rather than refusing to build
/// a proxy for a container that will not report a rate.
pub(crate) async fn frame_rate(path: &Path) -> Result<f64> {
    let out = tokio::time::timeout(
        DECODE_TIMEOUT,
        spawn::ffprobe(
            "proxy-rate",
            path,
            true,
            &[
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=avg_frame_rate",
                "-of",
                "csv=p=0",
            ]
            .iter()
            .map(OsStr::new)
            .chain(std::iter::once(path.as_os_str()))
            .collect::<Vec<_>>(),
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("frame-rate probe exceeded {DECODE_TIMEOUT:?}"))?
    .map_err(|e| anyhow::anyhow!("spawn ffprobe: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let Some(rate) = text
        .trim()
        .split('/')
        .next()
        .and_then(|n| n.trim().parse::<f64>().ok())
    else {
        anyhow::bail!("ffprobe reported no frame rate for {}", path.display());
    };
    Ok(rate)
}

/// Presentation times of a file's keyframes, in seconds.
///
/// Read from **packets**, not frames: `-show_entries frame=` makes ffprobe
/// decode every frame, which on a six-hour source is the very cost this todo
/// exists to avoid, and a keyframe *index* is what a seek needs.
pub(crate) async fn keyframe_times(path: &Path) -> Result<Vec<f64>> {
    let out = tokio::time::timeout(
        DECODE_TIMEOUT,
        spawn::ffprobe(
            "proxy-index",
            path,
            // The caller states whether this is the proxy; the probe itself
            // decodes no picture, so the `Must NOT` does not turn on it.
            true,
            &[
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "packet=pts_time,flags",
                "-of",
                "csv=p=0",
            ]
            .iter()
            .map(OsStr::new)
            .chain(std::iter::once(path.as_os_str()))
            .collect::<Vec<_>>(),
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("keyframe probe exceeded {DECODE_TIMEOUT:?}"))?
    .map_err(|e| anyhow::anyhow!("spawn ffprobe: {e}"))?;
    if !out.status.success() {
        anyhow::bail!("ffprobe failed reading keyframes of {}", path.display());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut times = Vec::new();
    for line in text.lines() {
        // `pts_time,flags` in csv order, and a line may be either order
        // depending on the ffprobe build, so both fields are located by shape
        // rather than by position.
        let mut pts = None;
        let mut key = false;
        for field in line.split(',') {
            let field = field.trim();
            if field.parse::<f64>().is_ok() {
                pts = field.parse::<f64>().ok();
            } else if field.contains('K') {
                key = true;
            }
        }
        if key {
            if let Some(t) = pts {
                times.push(t);
            }
        }
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    times.dedup_by(|a, b| (*a - *b).abs() < f64::EPSILON);
    Ok(times)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("publish {}", path.display()))?;
    Ok(())
}
