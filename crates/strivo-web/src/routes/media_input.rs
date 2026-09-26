//! Media-input admission: the gate every ffmpeg-bound media request passes.
//!
//! Report item F03 and the hardening note at
//! `docs/CREATOR-EDITION-REPORT-2026-09-07.md:98`: `editor_save` accepts
//! client-supplied cut paths, so a hand-crafted request can name any path on
//! the host and any `start_sec`/`end_sec`. The fix is a server-side admission
//! gate — never a UI check — and it runs **before** any child process is
//! created, which is the whole point of validating here rather than reading
//! ffmpeg's stderr afterwards.
//!
//! ## The two stages, and why they are two
//!
//! 1. **Static** ([`admit_path`]) — protocol grammar, containment, existence,
//!    file-ness, non-emptiness. Pure filesystem work. **No `ffprobe` child is
//!    created for any of these classes**, which is asserted by
//!    `tests/media_input.rs` through a PATH shim that would record any spawn.
//! 2. **Probe** ([`probe_admitted`]) — one `ffprobe` under the deadline the
//!    probe route already uses, from which the source's *media* facts come:
//!    a [`MediaTime`] duration, whether it carries a video stream, and whether
//!    it decodes at all. Only then can a cut be checked against its source.
//!
//! A caller that only needs stage 1 (the probe route, whose path comes from
//! the daemon-set jobs DB) stops there. An edit decision list needs both,
//! because "is `end_sec` inside the source?" is unanswerable without a
//! duration.
//!
//! ## Why the probe result is deliberately not cached
//!
//! The probe route caches on `(canonical path, length, mtime)` for 30 s. This
//! gate does not read that cache and does not write one. Reusing it would
//! inherit a key that cannot see a rewrite preserving both length and mtime,
//! which is exactly the "a file that has since become invalid" case; a TTL hit
//! is also indistinguishable from a fast recompute, so a test could not tell
//! which one admitted the input. The cost is one extra `ffprobe` per edit save
//! and per render — a few hundred milliseconds against a render that is
//! minutes of transcoding. Paying that is cheaper than a gate whose answer
//! depends on a cache it does not own.
//!
//! ## The units
//!
//! Every duration here is a [`MediaTime`] — whole microseconds, from
//! `strivo_editor::time`. There is deliberately no `f64` accessor on it and
//! none is added: `from_secs_f64` is the named boundary that reads ffprobe's
//! `"12.345000"`, and it refuses NaN, infinity and negatives by name. The
//! `f32` seconds in [`strivo_editor::Cut`] are read through
//! [`Cut::span`](strivo_editor::Cut::span), which already reports a
//! non-finite, negative or inverted cut as a typed [`TimeError`] — that error
//! is carried into [`MediaRejection::CutRange`] rather than being re-derived
//! here, because two implementations of "is this a real media coordinate" is
//! the drift this module exists to remove.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use strivo_editor::time::MediaTime;

use crate::problem::Problem;
use crate::server::AppState;

mod edl;
pub use edl::{admit_edl, AdmittedEdl};

/// The probe deadline. The same 30 s bound `settings::recording_probe` uses,
/// restated rather than imported so this module owns no dependency on that
/// handler's internals; `scripts` and `docs/PERFORMANCE.md` treat the probe
/// deadline as part of the cold-media contract.
const PROBE_DEADLINE: Duration = Duration::from_secs(30);

/// Why a path was refused, before anything was read from it.
///
/// Typed because "invalid media" is not something a caller can act on: the
/// SPA has to tell "you picked a directory" from "that file is not in your
/// recording folder" from "ffmpeg would treat that string as a protocol".
/// [`PathRejection::code`] is the branch key and is stable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathRejection {
    /// An empty or whitespace-only path.
    EmptyPath,
    /// The string carries a URI scheme — `file://`, `http://`, `concat:`,
    /// `subfile:`, `pipe:`. ffmpeg resolves those itself, so a bare path
    /// field is an unvalidated input channel, not a filename. A single
    /// leading letter is read as a Windows drive, not a scheme.
    ProtocolNotAllowed { scheme: String },
    /// The path does not resolve — missing, or a component is not readable.
    NotFound,
    /// It resolves to something that is not a regular file (a directory, a
    /// fifo, a device).
    NotAFile { kind: &'static str },
    /// Zero bytes. A recording that never received a byte is not media.
    EmptyFile,
    /// The root to contain within does not resolve — an operator
    /// misconfiguration, not a bad request, and reported as its own class so
    /// it is never mistaken for a client error.
    RootUnavailable,
    /// It resolves — symlinks included — to somewhere outside the root the
    /// caller declared. This is F03's "unauthorized asset".
    OutsideRoot,
}

impl PathRejection {
    /// Stable machine code, the key a caller branches on.
    pub fn code(&self) -> &'static str {
        match self {
            Self::EmptyPath => "media-input/empty-path",
            Self::ProtocolNotAllowed { .. } => "media-input/protocol-not-allowed",
            Self::NotFound => "media-input/not-found",
            Self::NotAFile { .. } => "media-input/not-a-file",
            Self::EmptyFile => "media-input/empty-file",
            Self::RootUnavailable => "media-input/root-unavailable",
            Self::OutsideRoot => "media-input/outside-root",
        }
    }
}

impl fmt::Display for PathRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPath => f.write_str("media input path is empty"),
            Self::ProtocolNotAllowed { scheme } => write!(
                f,
                "media input protocol {scheme:?} is not allowed; give a path on disk"
            ),
            Self::NotFound => f.write_str("media input path does not resolve"),
            Self::NotAFile { kind } => write!(f, "media input is a {kind}, not a regular file"),
            Self::EmptyFile => f.write_str("media input has zero bytes"),
            Self::RootUnavailable => f.write_str("the media root does not resolve"),
            Self::OutsideRoot => f.write_str("media input resolves outside the media root"),
        }
    }
}

impl PathRejection {
    /// The HTTP shape for a path refusal: 400 with the code in the
    /// `application/problem+json` body, so a client can branch on it instead
    /// of matching prose.
    pub fn to_problem(&self) -> Problem {
        Problem::bad_request(format!("{}: {}", self.code(), self))
    }
}

/// Why a media input was refused, once the gate has read what it needed.
///
/// Each class is one variant because each is one decision a caller can take a
/// different action on: fix the path, move the file, re-record the source, or
/// re-cut the edit. The cut index is carried on the per-cut classes so a
/// caller can point at the offending cut rather than the whole document.
#[derive(Debug, Clone, PartialEq)]
pub enum MediaRejection {
    /// A cut's path was refused. The class is the inner [`PathRejection`].
    CutPath { cut: usize, source: PathRejection },
    /// A cut's own coordinates are not a media coordinate: NaN, infinity,
    /// negative, or an end before its start. The `TimeError` is todo 22's,
    /// carried verbatim rather than re-derived.
    CutRange {
        cut: usize,
        error: strivo_editor::time::TimeError,
    },
    /// A cut reaches past the end of the source it names.
    CutOutOfRange {
        cut: usize,
        start_us: i64,
        end_us: i64,
        source_us: i64,
    },
    /// An edit decision list with no cuts renders to nothing.
    NoCuts,
    /// `ffprobe` could not read the file: it exited non-zero, timed out, or
    /// printed something that is not JSON.
    Undecodable { detail: String },
    /// It decoded, but carries no video stream. Every admitted source is
    /// something the editor cuts as moving pictures; an audio-only or
    /// data-only file has no frame to place a cut on.
    NoVideoStream { path: String },
    /// The container states no duration (`absent`, or ffprobe's `"N/A"`), so
    /// no cut can be range-checked against it.
    DurationMissing { path: String },
    /// The container states a duration that is not a finite number.
    DurationNotFinite { path: String, raw: String },
    /// The container states a negative duration.
    DurationNegative { path: String, raw: String },
}

impl MediaRejection {
    /// Stable machine code. For [`MediaRejection::CutPath`] the code is the
    /// path class's own — the *class* is what a caller branches on, and the
    /// cut index travels in the detail.
    pub fn code(&self) -> &'static str {
        match self {
            Self::CutPath { source, .. } => source.code(),
            Self::CutRange { .. } => "media-input/cut-range",
            Self::CutOutOfRange { .. } => "media-input/cut-out-of-range",
            Self::NoCuts => "media-input/no-cuts",
            Self::Undecodable { .. } => "media-input/undecodable",
            Self::NoVideoStream { .. } => "media-input/no-video-stream",
            Self::DurationMissing { .. } => "media-input/duration-missing",
            Self::DurationNotFinite { .. } => "media-input/duration-not-finite",
            Self::DurationNegative { .. } => "media-input/duration-negative",
        }
    }
}

impl fmt::Display for MediaRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CutPath { cut, source } => write!(f, "cut {cut}: {source}"),
            Self::CutRange { cut, error } => write!(f, "cut {cut}: {error}"),
            Self::CutOutOfRange {
                cut,
                start_us,
                end_us,
                source_us,
            } => write!(
                f,
                "cut {cut}: {start_us}..{end_us} us is outside the source's {source_us} us"
            ),
            Self::NoCuts => f.write_str("edit decision list has no cuts"),
            Self::Undecodable { detail } => write!(f, "media input is not decodable: {detail}"),
            Self::NoVideoStream { path } => {
                write!(f, "{}: carries no video stream", short(path))
            }
            Self::DurationMissing { path } => {
                write!(f, "{}: container states no duration", short(path))
            }
            Self::DurationNotFinite { path, raw } => {
                write!(f, "{}: duration {raw:?} is not finite", short(path))
            }
            Self::DurationNegative { path, raw } => {
                write!(f, "{}: duration {raw:?} is negative", short(path))
            }
        }
    }
}

/// The file name, not the whole absolute path: a rejection detail is a log
/// line and a UI string, and the caller already knows which file it submitted.
fn short(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
}

impl MediaRejection {
    /// 400 with the code in the body — a rejection the caller can branch on,
    /// not a generic failure with different prose.
    pub fn to_problem(&self) -> Problem {
        Problem::bad_request(format!("{}: {}", self.code(), self))
    }
}

/// What the gate learned about one admitted source.
#[derive(Debug, Clone, PartialEq)]
pub struct AdmittedMedia {
    /// The canonical path, symlinks resolved.
    pub path: PathBuf,
    /// Length in bytes, as read at admission.
    pub bytes: u64,
    /// The container's duration, in whole microseconds.
    pub duration: MediaTime,
}

/// Stage 1: everything decidable from the filesystem and the string itself.
///
/// `root` is `Some` for a client-supplied path — the recorded output path and
/// any insert must live inside the operator's media root — and `None` for a
/// path the daemon itself recorded, which is already trusted and only needs to
/// be usable. No child process is created on any path through this function.
pub fn admit_path(root: Option<&Path>, raw: &str) -> Result<PathBuf, PathRejection> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(PathRejection::EmptyPath);
    }
    if let Some(scheme) = uri_scheme(trimmed) {
        return Err(PathRejection::ProtocolNotAllowed { scheme });
    }
    let real = Path::new(trimmed)
        .canonicalize()
        .map_err(|_| PathRejection::NotFound)?;
    if let Some(root) = root {
        // Canonicalised too, or a root that is itself a symlink (a common
        // `/data` → `/mnt/disk2` setup) would refuse every file inside it.
        let real_root = root
            .canonicalize()
            .map_err(|_| PathRejection::RootUnavailable)?;
        if !real.starts_with(&real_root) {
            return Err(PathRejection::OutsideRoot);
        }
    }
    let meta = std::fs::metadata(&real).map_err(|_| PathRejection::NotFound)?;
    if !meta.is_file() {
        return Err(PathRejection::NotAFile {
            kind: file_kind(&meta),
        });
    }
    if meta.len() == 0 {
        return Err(PathRejection::EmptyFile);
    }
    Ok(real)
}

/// The URI scheme of a bare path string, if it has one.
///
/// The rule is deliberately coarser than RFC 3986's scheme grammar, because the
/// grammar is not what the attack looks like: `subfile,,start,0:/etc/shadow` is
/// the `subfile` protocol to ffmpeg but not a valid RFC scheme, and a parser
/// that only recognised the grammar would hand that string to the filesystem as
/// a filename. So the test is positional — a colon before the first separator,
/// with more than one character before it, is a protocol — and nothing after the
/// colon is inspected. That still leaves a Windows drive (`C:\…`, one character
/// before the colon) alone, which matters because misreading it as a scheme
/// would refuse every recording on Windows.
fn uri_scheme(raw: &str) -> Option<String> {
    let colon = raw.find(':')?;
    let head = &raw[..colon];
    if head.contains('/') || head.contains('\\') {
        return None;
    }
    if head.is_empty() || head.len() == 1 {
        return None;
    }
    Some(head.to_ascii_lowercase())
}

/// The human name of a non-regular file. Only the directory case is named:
/// distinguishing a fifo from a socket would need a platform-specific
/// `FileTypeExt` to describe cases the caller cannot act on differently.
fn file_kind(meta: &std::fs::Metadata) -> &'static str {
    if meta.file_type().is_dir() {
        "directory"
    } else {
        "special file"
    }
}

/// Stage 2: read one file's media facts, in one `ffprobe` under a deadline.
///
/// `bytes` is the length stage 1 already read, carried in so the admitted
/// value and the probed value cannot describe different files. The child is
/// killed on drop, so a cancelled request does not leave an orphan holding
/// memory; the deadline is [`PROBE_DEADLINE`].
pub async fn probe_admitted(path: &Path, bytes: u64) -> Result<AdmittedMedia, MediaRejection> {
    let mut probe = tokio::process::Command::new(strivo_core::tools::resolve_tool("ffprobe"));
    probe.kill_on_drop(true);
    probe.args([
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
    ]);
    probe.arg(path);
    let out = match tokio::time::timeout(PROBE_DEADLINE, probe.output()).await {
        Ok(Ok(o)) => o,
        Err(_) => {
            return Err(MediaRejection::Undecodable {
                detail: "ffprobe timed out".into(),
            });
        }
        Ok(Err(e)) => {
            return Err(MediaRejection::Undecodable {
                detail: format!("ffprobe spawn: {e}"),
            });
        }
    };
    if !out.status.success() {
        return Err(MediaRejection::Undecodable {
            detail: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    let raw: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| MediaRejection::Undecodable {
            detail: format!("ffprobe parse: {e}"),
        })?;
    let shown = path.to_string_lossy().to_string();
    let streams = raw
        .get("streams")
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if streams.is_empty() {
        return Err(MediaRejection::Undecodable {
            detail: "container declares no streams".into(),
        });
    }
    if !streams
        .iter()
        .any(|s| s.get("codec_type").and_then(|v| v.as_str()) == Some("video"))
    {
        return Err(MediaRejection::NoVideoStream { path: shown });
    }
    let duration = read_duration(&raw, &shown)?;
    Ok(AdmittedMedia {
        path: path.to_path_buf(),
        bytes,
        duration,
    })
}

/// The container's own duration, as a [`MediaTime`].
///
/// Read from `format.duration`, which ffprobe prints as a decimal string or
/// the literal `"N/A"`. Three refusals rather than one: a caller that has to
/// distinguish "this file has no length" from "this file's length is
/// nonsense" cannot do it from one error, and the second is a defect worth
/// seeing while the first is normal for a raw stream.
fn read_duration(raw: &serde_json::Value, shown: &str) -> Result<MediaTime, MediaRejection> {
    let text = raw
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(|v| v.as_str())
        .unwrap_or("N/A");
    let secs: f64 = text.parse().map_err(|_| MediaRejection::DurationMissing {
        path: shown.to_string(),
    })?;
    if !secs.is_finite() {
        return Err(MediaRejection::DurationNotFinite {
            path: shown.to_string(),
            raw: text.to_string(),
        });
    }
    if secs < 0.0 {
        return Err(MediaRejection::DurationNegative {
            path: shown.to_string(),
            raw: text.to_string(),
        });
    }
    // The one float boundary in the model, and the same one the legacy EDL
    // and a chat log's `time_sec` column use. Seconds -> whole microseconds,
    // rounded; anything it refuses was already refused above.
    MediaTime::from_secs_f64(secs).map_err(|e| MediaRejection::Undecodable {
        detail: format!("duration out of range: {e}"),
    })
}

/// Admit one source the **daemon** resolved from a recording id, rather than a
/// path a client named.
///
/// No containment check: the caller got the path from the jobs database, so
/// there is no client string to contain. The probe still runs, because a
/// recording row can name a file that has since been replaced, truncated or
/// emptied, and the caller's next step is a media tool.
pub async fn admit_source(state: &AppState, path: &Path) -> Result<AdmittedMedia, MediaRejection> {
    let _permit = state
        .probe_slots
        .acquire()
        .await
        .map_err(|_| MediaRejection::Undecodable {
            detail: "media worker pool closed".into(),
        })?;
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    probe_admitted(path, bytes).await
}

/// The whole EDL gate: stage 1, one probe per *distinct* source, then stage 3
/// (the per-cut range check). This is what a handler calls.
///
/// Two passes, and the order is the security property. Every cut's path is
/// resolved before any cut is probed, so a document that names one traversal
/// path or `file://` source among a hundred legitimate cuts still costs zero
/// child processes; a single interleaved pass would have handed the first cut
/// to ffprobe before reading the second cut's path.
///
/// A pool permit is taken around each probe so this gate queues behind the same
/// two media slots the probe route uses rather than adding a third class of
/// unaccounted child process — `docs/PERFORMANCE.md`'s two-slot bound stays the
/// only bound in the process. The permit is released before the range check,
/// which is pure computation.
pub async fn admit_edl_at(
    state: &AppState,
    root: &Path,
    edl: &strivo_editor::Edl,
) -> Result<AdmittedEdl, MediaRejection> {
    if edl.cuts.is_empty() {
        return Err(MediaRejection::NoCuts);
    }
    let mut sources: Vec<PathBuf> = Vec::new();
    for (cut, raw) in edl
        .cuts
        .iter()
        .enumerate()
        .map(|(i, c)| (i, edl::cut_path(c)))
    {
        let real = admit_path(Some(root), raw)
            .map_err(|source| MediaRejection::CutPath { cut, source })?;
        if !sources.contains(&real) {
            sources.push(real);
        }
    }

    let mut admitted: HashMap<PathBuf, AdmittedMedia> = HashMap::with_capacity(sources.len());
    for real in &sources {
        let bytes = std::fs::metadata(real).map(|m| m.len()).unwrap_or(0);
        let media = {
            let _permit = match state.probe_slots.acquire().await {
                Ok(permit) => permit,
                // The pool is closed only at shutdown; refusing is correct.
                Err(_) => {
                    return Err(MediaRejection::Undecodable {
                        detail: "media worker pool closed".into(),
                    });
                }
            };
            probe_admitted(real, bytes).await?
        };
        admitted.insert(real.clone(), media);
    }
    admit_edl(root, edl, &admitted)
}
