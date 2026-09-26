//! Waveform pyramids and filmstrips, both derived from the proxy.
//!
//! Neither is produced from a user's source. A waveform is a whole-file
//! decode — the most expensive thing in this module by a wide margin — so it is
//! taken from the proxy's already-transcoded audio, and it is built once and
//! addressed by the same content fingerprint as the proxy. A filmstrip is a
//! sparse frame sample and is likewise taken from the proxy.
//!
//! Todo 37 already models `IngestStep::Waveform` and `IngestStep::Filmstrip`
//! as readiness *states*. Nothing here adds a parallel status: the
//! [`Derived`] enum below reports which artifact a build produced, and the
//! readiness step remains the only place that says whether a source is ready.

use anyhow::{Context, Result};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::proxy::{keyframe_times, ProxyArtifact};
use super::spawn;

/// The rate peaks are computed at. Peaks do not need a rate, and 8 kHz keeps a
/// six-hour source's decoded audio to 691 MB streamed rather than 4.1 GB.
pub const PEAK_SAMPLE_RATE: u32 = 8_000;

/// A whole-file audio or frame build may take this long.
pub const BUILD_TIMEOUT: Duration = Duration::from_secs(900);

/// Which artifact a build produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Derived {
    Waveform,
    Filmstrip,
}

/// A multi-resolution peak pyramid over one source's audio.
///
/// Level 0 holds one peak per [`base_samples`] decoded samples; each further
/// level halves the resolution by taking the max of adjacent buckets. A
/// waveform drawn across a six-hour timeline needs a few thousand points, and
/// the pyramid is what lets a zoom from six hours to six seconds be a level
/// change rather than a re-decode.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WaveformPyramid {
    pub sample_rate: u32,
    pub base_samples: usize,
    /// Coarsest last. `levels[0]` is the finest.
    pub levels: Vec<Vec<f32>>,
    /// Seconds of audio the finest level covers.
    pub duration_secs: f64,
}

impl WaveformPyramid {
    /// Build from interleaved-free mono f32 samples in `[-1, 1]`.
    ///
    /// Pure, so it is testable without ffmpeg and so the shape of the pyramid
    /// is not a property of whichever encoder produced the samples.
    pub fn build(samples: &[f32], sample_rate: u32) -> Self {
        let base = sample_rate as usize;
        let mut finest = Vec::with_capacity(samples.len() / base + 1);
        for chunk in samples.chunks(base) {
            finest.push(chunk.iter().fold(0.0_f32, |acc, s| acc.max(s.abs())));
        }
        let duration_secs = samples.len() as f64 / sample_rate as f64;
        let mut levels = vec![finest];
        // Halve until a level is small enough to draw whole, or until a level
        // would be a single bucket. 64 is roughly a screen width of columns.
        while levels.last().map(|l| l.len()).unwrap_or(0) > 64 {
            let previous = levels.last().expect("a level exists");
            let mut next = Vec::with_capacity(previous.len().div_ceil(2));
            for pair in previous.chunks(2) {
                next.push(pair.iter().copied().fold(0.0_f32, f32::max));
            }
            levels.push(next);
        }
        Self {
            sample_rate,
            base_samples: base,
            levels,
            duration_secs,
        }
    }

    /// The coarsest level whose bucket is at most `target_points` wide, so a
    /// caller asking for a viewport gets the cheapest level that still
    /// resolves it.
    pub fn level_for_points(&self, target_points: usize) -> usize {
        self.levels
            .iter()
            .position(|l| l.len() <= target_points.max(1))
            .unwrap_or(self.levels.len().saturating_sub(1))
    }

    /// Peaks covering `[start_secs, end_secs)` at `level`, normalised to
    /// `0.0..=1.0`. One peak per bucket of the chosen level, so a caller can
    /// draw a bar per entry.
    pub fn peaks(&self, level: usize, start_secs: f64, end_secs: f64) -> Vec<f32> {
        let Some(values) = self
            .levels
            .get(level.min(self.levels.len().saturating_sub(1)))
        else {
            return Vec::new();
        };
        if values.is_empty() {
            return Vec::new();
        }
        let secs_per_bucket =
            self.base_samples as f64 / self.sample_rate as f64 / 2f64.powi(level as i32);
        let span = (end_secs - start_secs).max(0.0);
        let buckets = ((span / secs_per_bucket).ceil() as usize).clamp(1, 4_096);
        let first = (start_secs / secs_per_bucket).floor().max(0.0) as usize;
        let mut out = Vec::with_capacity(buckets);
        for i in 0..buckets {
            let idx = first + i;
            out.push(values.get(idx).copied().unwrap_or(0.0));
        }
        out
    }

    /// Serialise for the on-disk cache.
    pub fn to_json(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }
}

/// Read the proxy's audio as mono f32 at [`PEAK_SAMPLE_RATE`] and fold it into
/// a pyramid.
///
/// The decode is streamed, so peak memory is one chunk rather than the whole
/// file, and the child's own output pipe is what bounds it.
pub async fn build_pyramid(proxy: &ProxyArtifact) -> Result<WaveformPyramid> {
    let _permit = spawn::acquire_slot("waveform build")
        .await
        .map_err(anyhow::Error::msg)?;
    let rate = PEAK_SAMPLE_RATE.to_string();
    let argv: Vec<OsString> = [
        "-nostdin".to_string(),
        "-i".to_string(),
        proxy.path.to_string_lossy().into_owned(),
        "-vn".to_string(),
        "-ac".to_string(),
        "1".to_string(),
        "-ar".to_string(),
        rate,
        "-f".to_string(),
        "f32le".to_string(),
        "-".to_string(),
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    let argv: Vec<&OsStr> = argv.iter().map(OsString::as_os_str).collect();

    spawn::record(spawn::Spawn {
        tool: "ffmpeg",
        purpose: "pyramid-build",
        input: proxy.path.to_string_lossy().into_owned(),
        input_is_proxy: true,
        width: 0,
        height: 0,
        decodes_picture: false,
    });
    let mut cmd = tokio::process::Command::new(crate::tools::resolve_tool("ffmpeg"));
    cmd.kill_on_drop(true);
    cmd.args(&argv).stdout(std::process::Stdio::piped());

    let mut child = cmd.spawn().context("spawn ffmpeg")?;
    let mut stdout = child
        .stdout
        .take()
        .context("ffmpeg produced no audio stream")?;

    use tokio::io::AsyncReadExt;
    let mut samples: Vec<f32> = Vec::new();
    let mut buf = vec![0_u8; 64 * 1024];
    loop {
        let read = tokio::time::timeout(BUILD_TIMEOUT, stdout.read(&mut buf))
            .await
            .map_err(|_| anyhow::anyhow!("waveform build exceeded {BUILD_TIMEOUT:?}"))??;
        if read < 4 {
            break;
        }
        let usable = read - read % 4;
        let (floats, _) = buf[..usable].as_chunks::<4>();
        samples.extend(floats.iter().map(|c| f32::from_le_bytes(*c)));
    }
    let status = tokio::time::timeout(BUILD_TIMEOUT, child.wait())
        .await
        .map_err(|_| anyhow::anyhow!("waveform build exceeded {BUILD_TIMEOUT:?}"))??;
    if !status.success() {
        anyhow::bail!("ffmpeg exited {status} decoding proxy audio");
    }
    Ok(WaveformPyramid::build(&samples, PEAK_SAMPLE_RATE))
}

/// A filmstrip: `tiles` frames sampled evenly across the source, tiled into a
/// single PNG.
#[derive(Debug, Clone, PartialEq)]
pub struct Filmstrip {
    pub path: PathBuf,
    pub tiles: u32,
    pub columns: u32,
    pub rows: u32,
    pub tile_width: u32,
    /// Seconds between consecutive tiles, so a caller can label the strip.
    pub interval_secs: f64,
}

/// Build a filmstrip from the proxy.
///
/// `duration_secs` comes from the proxy's measured keyframe index rather than
/// a second probe, so the strip and the seek map agree by construction.
pub async fn build_filmstrip(
    proxy: &ProxyArtifact,
    duration_secs: f64,
    tiles: u32,
    tile_width: u32,
    columns: u32,
) -> Result<Filmstrip> {
    let _permit = spawn::acquire_slot("filmstrip build")
        .await
        .map_err(anyhow::Error::msg)?;
    let tiles = tiles.max(1);
    let columns = columns.clamp(1, tiles);
    let rows = tiles.div_ceil(columns);
    let interval_secs = if duration_secs > 0.0 {
        duration_secs / tiles as f64
    } else {
        0.0
    };

    let dir = proxy
        .path
        .parent()
        .context("the proxy has no parent directory")?;
    let stem = proxy
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("proxy");
    let out = dir.join(format!("{stem}.filmstrip-{tiles}x{columns}.png"));
    let tmp = dir.join(format!("{stem}.filmstrip.{}.tmp.png", std::process::id()));
    let _ = std::fs::remove_file(&tmp);

    // `fps=1/interval` samples the strip evenly in time rather than every Nth
    // frame, so a variable-frame-rate source still gets an even strip.
    let filter = format!("fps=1/{interval_secs:.6},scale={tile_width}:-2,tile={columns}x{rows}");
    let argv: Vec<OsString> = [
        "-nostdin".to_string(),
        "-y".to_string(),
        "-i".to_string(),
        proxy.path.to_string_lossy().into_owned(),
        "-vf".to_string(),
        filter,
        "-frames:v".to_string(),
        "1".to_string(),
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
        BUILD_TIMEOUT,
        spawn::ffmpeg(
            "filmstrip-build",
            &proxy.path,
            true,
            proxy.width,
            proxy.height,
            true,
            &argv,
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("filmstrip build exceeded {BUILD_TIMEOUT:?}"))?
    .map_err(|e| anyhow::anyhow!("spawn ffmpeg: {e}"))?;
    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("ffmpeg exited {status} building a {tiles}-tile filmstrip");
    }
    std::fs::rename(&tmp, &out).with_context(|| format!("publish filmstrip {}", out.display()))?;
    Ok(Filmstrip {
        path: out,
        tiles,
        columns,
        rows,
        tile_width,
        interval_secs,
    })
}

/// The keyframe times of a file, re-exported so a caller holding only a path
/// can build a seek map without importing [`super::proxy`].
pub async fn keyframes_of(path: &Path) -> Result<Vec<f64>> {
    keyframe_times(path).await
}
