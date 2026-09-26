//! Measuring a multitrack container: what is in it, and where its streams'
//! presentation times jump.
//!
//! The measurement half of the OBS multitrack import, and it lives here rather
//! than in `strivo-research` for the same reason todo 25's probe does: spawning
//! a media tool belongs to the process that owns the tool pool. Todo 33 states
//! the same rule from the other side — import runs no `ffprobe` — and this
//! module is where the media facts come from instead.
//!
//! It is **not** a second admission path. Every function here takes an
//! [`AdmittedFile`], the type whose only constructor is a path that has already
//! passed `media_input::admit_path`; there is no `&str` and no `PathBuf` entry
//! point, so nothing a client names reaches ffprobe without the gate having seen
//! it.
//!
//! ## Two children, whatever the track count
//!
//! One `-show_format -show_streams` for the container's own facts, and one
//! `-show_frames` for every stream's presentation times. Not one child per
//! stream: a six-track session would cost seven processes, and a per-child
//! deadline would let one long track exceed the whole budget six times over.
//!
//! ## The bound is the session's length
//!
//! The frame table is O(duration) — a four-hour session is about two million
//! lines — so **duration is the denial-of-service axis and it is refused before
//! the second child is spawned**, from the first child's own `format.duration`.
//! Reading the whole table and then discovering it was too long is the version
//! that costs the memory.

use std::path::Path;
use std::time::Duration;

use strivo_research::creator::TrackMeasurement;

use crate::routes::multitrack_gaps::{self, Reading};
use strivo_research::creator::AdmittedFile;

/// How long either probe child may run. The same order as `media_input`'s probe
/// deadline: one media child, one bound, and `docs/PERFORMANCE.md`'s two-slot
/// account stays the only one in the process.
const PROBE_DEADLINE: Duration = Duration::from_secs(30);

/// The longest session the frame scan will read, in whole seconds.
///
/// Six hours is a full broadcast day and roughly three million presentation
/// lines, which ffprobe emits in a few seconds. Past it the request is refused
/// with the figure rather than truncated, because a session whose later half was
/// never scanned would report a clean alignment over half its length.
const MAX_SESSION_SECS: i64 = 6 * 60 * 60;

/// Why a container could not be measured.
///
/// Deliberately not [`media_input::MediaRejection`](crate::routes::media_input::MediaRejection):
/// that is the vocabulary of the **path** gate, and these are statements about
/// what the file contains. The path was already admitted by the time anything
/// here runs, so borrowing the gate's codes for a fact it never checked would be
/// a class of error the caller cannot act on correctly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeRejection {
    /// ffprobe could not read the container at all.
    Undecodable { detail: String },
    /// Longer than [`MAX_SESSION_SECS`], so the presentation-time scan is not
    /// affordable. `secs` is what ffprobe stated.
    SessionTooLong { secs: i64, limit_secs: i64 },
    /// The container states no duration, so the bound cannot be applied. Refused
    /// rather than assumed: an unbounded session is the case the bound is for,
    /// and a live stream legitimately reports none.
    DurationUnknown,
}

impl ProbeRejection {
    /// Stable machine code, the key a caller branches on.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Undecodable { .. } => "multitrack/undecodable",
            Self::SessionTooLong { .. } => "multitrack/session-too-long",
            Self::DurationUnknown => "multitrack/duration-unknown",
        }
    }
}

impl std::fmt::Display for ProbeRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Undecodable { detail } => write!(f, "container is not readable: {detail}"),
            Self::SessionTooLong { secs, limit_secs } => write!(
                f,
                "session is {secs} s and the scan limit is {limit_secs} s"
            ),
            Self::DurationUnknown => {
                f.write_str("the container states no duration, so the scan cannot be bounded")
            }
        }
    }
}

/// What one container holds, measured.
pub struct SessionProbe {
    /// The video stream's measurement, where the container has one. The
    /// session's own timeline and its holes come from here.
    pub picture: Option<TrackMeasurement>,
    /// One measurement per audio stream, in the container's own stream order.
    pub tracks: Vec<TrackMeasurement>,
    /// The container's stated length, in whole microseconds.
    pub duration_us: i64,
}

/// Measure an admitted container, or say why it cannot be measured.
pub async fn probe_session(file: &AdmittedFile) -> Result<SessionProbe, ProbeRejection> {
    let path = file.locator().to_path_buf();
    let header: serde_json::Value =
        serde_json::from_str(&run(&path, &["-show_format", "-show_streams"]).await?)
            .unwrap_or(serde_json::Value::Null);
    let duration_us = bound_duration(&header)?;
    let csv = run(
        &path,
        &[
            "-show_frames",
            "-show_entries",
            "frame=stream_index,pts,key_frame",
            "-of",
            "compact=p=0:nk=0",
        ],
    )
    .await?;
    Ok(assemble(&header, &csv, duration_us))
}

/// Refuse a session the frame scan cannot afford, before the scan is spawned.
fn bound_duration(header: &serde_json::Value) -> Result<i64, ProbeRejection> {
    let raw = header
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(serde_json::Value::as_str);
    let secs = raw
        .and_then(|t| t.parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .ok_or(ProbeRejection::DurationUnknown)?;
    if secs > MAX_SESSION_SECS as f64 {
        return Err(ProbeRejection::SessionTooLong {
            secs: secs as i64,
            limit_secs: MAX_SESSION_SECS,
        });
    }
    // The one float boundary, and the same named one `media_input::read_duration`
    // uses: seconds to whole microseconds, refusing what is not a real position.
    strivo_editor::time::MediaTime::from_secs_f64(secs)
        .map(|d| d.micros())
        .map_err(|e| ProbeRejection::Undecodable {
            detail: format!("duration out of range: {e}"),
        })
}

/// Turn the two probe outputs into measurements.
fn assemble(header: &serde_json::Value, csv: &str, duration_us: i64) -> SessionProbe {
    let readings = multitrack_gaps::read_frames(csv);
    let mut picture = None;
    let mut tracks = Vec::new();
    for s in header
        .get("streams")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(index) = s.get("index").and_then(serde_json::Value::as_u64) else {
            continue;
        };
        let Some(kind) = s.get("codec_type").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(time_base) = s.get("time_base").and_then(serde_json::Value::as_str) else {
            continue;
        };
        // A stream whose clock cannot be converted has no position this model can
        // state, so it is not a track. A skipped stream is not a silent loss: it
        // is absent from the track list, which is the answer.
        if multitrack_gaps::tick_micros(time_base).is_none() {
            continue;
        }
        let Some(base) = multitrack_gaps::declared_base(s, kind) else {
            continue;
        };
        let Reading {
            start,
            coverage,
            gaps,
            complete,
        } = multitrack_gaps::walk(&readings, index as u32, time_base);
        let m = TrackMeasurement {
            stream_index: index as u32,
            label: s
                .get("tags")
                .and_then(|t| t.get("title"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            time_base: base,
            start,
            coverage,
            gaps,
            complete,
        };
        if kind == "video" {
            picture = Some(m);
        } else if tracks.len() < strivo_research::creator::MAX_TRACKS {
            tracks.push(m);
        }
    }
    SessionProbe {
        picture,
        tracks,
        duration_us,
    }
}

/// One `-print_format json` probe under [`PROBE_DEADLINE`], killed on drop so a
/// cancelled request leaves no orphan holding memory.
async fn run(path: &Path, flags: &[&str]) -> Result<String, ProbeRejection> {
    let mut probe = tokio::process::Command::new(strivo_core::tools::resolve_tool("ffprobe"));
    probe.kill_on_drop(true);
    probe.args(["-v", "error", "-print_format", "json"]);
    for flag in flags {
        probe.arg(flag);
    }
    probe.arg(path);
    let out = match tokio::time::timeout(PROBE_DEADLINE, probe.output()).await {
        Ok(Ok(o)) => o,
        Err(_) => {
            return Err(ProbeRejection::Undecodable {
                detail: "ffprobe timed out".into(),
            });
        }
        Ok(Err(e)) => {
            return Err(ProbeRejection::Undecodable {
                detail: format!("ffprobe spawn: {e}"),
            });
        }
    };
    if !out.status.success() {
        return Err(ProbeRejection::Undecodable {
            detail: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    String::from_utf8(out.stdout).map_err(|e| ProbeRejection::Undecodable {
        detail: format!("ffprobe utf-8 output: {e}"),
    })
}
