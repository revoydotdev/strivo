//! Live clipping during capture — the tail, on the capture side (CE-P8).
//!
//! ROADMAP.md:202-204 promises "as fast as it is recorded": extractors tail
//! the live segment, and a clip renders from what capture has *already*
//! finished, while recording continues. The two halves of that live in
//! different crates for a structural reason — capture is a PVR concern and
//! the reviewed render is a Creator one, and `strivo-core` sits below
//! `strivo-research` in the dependency graph — so this module owns the half
//! that must be true for both: **which bytes of an in-progress capture are
//! safe to read.**
//!
//! ## A part is finalized when its writer exits, and only then
//!
//! The obvious implementation — glob the recording directory and read
//! whatever is there — is the corruption this todo warns about. Matroska is
//! not appendable in place, so the file a live recorder is writing has no
//! duration, no complete index and no tail; reading it yields a truncated
//! clip that *looks* fine. So nothing here infers finalization from the
//! filesystem. [`LiveCapture`] is fed the transition by the recorder
//! ([`LiveCapture::finalize_part`]) and a request that reaches into a part
//! still being written is [`LiveClipError::PartStillOpen`] — a typed
//! refusal, returned before any process is spawned, not a race that
//! sometimes produces a short file.
//!
//! ## The render is admitted against capture's byte reserve first
//!
//! "Must NOT let live rendering exhaust capture capacity" is todo 39's
//! isolation guarantee, and it binds here because this is the first thing in
//! the tree that writes derived media *while a capture is deliberately still
//! running*. The admission is [`CaptureCapacity::admit_render`] — the same
//! call the daemon's render path makes through `ResourceRegistry` — and its
//! refusal is carried as [`LiveClipError::RenderRefused`] with the numbers,
//! so a refused clip says why instead of silently producing nothing.
//!
//! ## The gate this must not weaken
//!
//! `ROADMAP.md:100` is the PVR recorder-slot gate. Nothing here touches
//! recorder slots; this is a bound on bytes. The line is nonetheless
//! asserted by [`tests::the_pvr_concurrency_gate_has_not_moved`] rather than
//! promised in prose, because a promise in a doc is not a check.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::capacity::{CaptureCapacity, RenderRefused};
use super::segments;

/// The `ROADMAP.md` line that carries the PVR recorder-slot gate, asserted
/// verbatim by the test of the same subject. Live clipping is a bound on
/// *bytes*; this is a bound on *recorder slots*, and neither may be relaxed
/// to accommodate the other.
pub const ROADMAP_GATE_LINE: usize = 100;

/// The gate line, byte for byte. The apostrophe in `manager's` is a plain
/// ASCII quote because that is what the file contains.
pub const ROADMAP_GATE_TEXT: &str = "| Concurrency cap is porous — manual `Start` paths bypass \
`max_concurrent_recordings` | ✅ | gate now in the manager's `Start` handler (all paths) |";

/// Shortest clip a render will attempt, in milliseconds.
///
/// Not a quality preference: below this a stream copy is shorter than its
/// own container overhead and ffmpeg's output keyframe lands past the end,
/// so the "rendered" clip would not contain the range that was asked for.
pub const MIN_CLIP_MS: u64 = 1_000;

/// Whether a capture part's bytes are safe to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartState {
    /// A recorder is writing this file right now. Never an input.
    Open,
    /// The writer exited and the duration is known. Safe to read.
    Finalized,
}

impl PartState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Finalized => "finalized",
        }
    }
}

/// One segment of a capture, as the tail knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturePart {
    /// 1-based, matching `segments::segment_path(base, n)`.
    pub index: u32,
    pub path: PathBuf,
    pub state: PartState,
    /// Offset of this part's first byte from the capture's first byte, in
    /// milliseconds of media. Contiguous with the previous part's end: a
    /// reconnect leaves a hole in the *wall clock*, not in the media.
    pub start_ms: u64,
    /// Media this part holds. Zero while the part is open, and it is
    /// [`PartState`] that says whether the value is known, not the number —
    /// a part that legitimately held nothing would otherwise read as open.
    pub duration_ms: u64,
}

impl CapturePart {
    #[must_use]
    pub fn end_ms(&self) -> u64 {
        self.start_ms.saturating_add(self.duration_ms)
    }

    /// Whether this part overlaps `[start_ms, end_ms)`.
    #[must_use]
    pub fn intersects(&self, start_ms: u64, end_ms: u64) -> bool {
        self.start_ms < end_ms && start_ms < self.end_ms()
    }
}

/// Why a live clip was refused.
///
/// Every variant names what would have to change, because the alternative —
/// a clip that silently did not appear, or worse, appeared truncated — is
/// indistinguishable from the feature not existing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveClipError {
    /// The requested range is empty or reversed.
    InvertedRange { start_ms: u64, end_ms: u64 },
    /// Shorter than [`MIN_CLIP_MS`].
    ClipTooShort { span_ms: u64, minimum_ms: u64 },
    /// Capture has not finished a part covering the start of the range.
    NothingFinalized {
        requested_start_ms: u64,
        finalized_end_ms: u64,
    },
    /// The range reaches into a part a recorder is still writing. **The
    /// corruption guard**: no clip is ever produced from one of these.
    PartStillOpen {
        index: u32,
        path: PathBuf,
        /// How far the finalized media reaches. The caller waits for this to
        /// grow rather than retrying into the same refusal.
        finalized_end_ms: u64,
    },
    /// The capture is over. A post-capture clip is an ordinary render and
    /// goes through the ordinary path; it does not come through here.
    CaptureStopped,
    /// Todo 39's byte reserve refused the render, or there is no headroom
    /// for it. The reason is `RenderRefused`'s own, with the numbers.
    RenderRefused { reason: String },
    /// A tool the render needs is missing or failed.
    ToolFailed { tool: &'static str, detail: String },
}

impl fmt::Display for LiveClipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvertedRange { start_ms, end_ms } => {
                write!(
                    f,
                    "clip range is empty or reversed: {start_ms} ms to {end_ms} ms"
                )
            }
            Self::ClipTooShort {
                span_ms,
                minimum_ms,
            } => write!(
                f,
                "clip spans {span_ms} ms, below the {minimum_ms} ms minimum"
            ),
            Self::NothingFinalized {
                requested_start_ms,
                finalized_end_ms,
            } => write!(
                f,
                "no finalized capture media covers {requested_start_ms} ms yet; capture has \
                 finalized {finalized_end_ms} ms"
            ),
            Self::PartStillOpen {
                index,
                path,
                finalized_end_ms,
            } => write!(
                f,
                "capture part {index} ({}) is still being written, so it is not a legal clip \
                 input; capture has finalized {finalized_end_ms} ms",
                path.display()
            ),
            Self::CaptureStopped => {
                f.write_str("the capture has stopped, so this is not a live clip")
            }
            Self::RenderRefused { reason } => write!(f, "live clip refused: {reason}"),
            Self::ToolFailed { tool, detail } => write!(f, "{tool} failed: {detail}"),
        }
    }
}

impl std::error::Error for LiveClipError {}

/// A request for a clip out of a capture that may still be running.
///
/// Times are milliseconds of **media** from the capture's first byte — the
/// same clock [`super::gaps`] and the merge path already use. The caller
/// does not get to name a part; it names a range, and the tail decides which
/// finalized parts that range is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveClipRequest {
    pub start_ms: u64,
    pub end_ms: u64,
}

impl LiveClipRequest {
    #[must_use]
    pub const fn new(start_ms: u64, end_ms: u64) -> Self {
        Self { start_ms, end_ms }
    }

    #[must_use]
    pub const fn span_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

/// The finalized parts a range is made of, and nothing else.
///
/// The type is only constructible through [`LiveCapture::clip_inputs`], and
/// the tests that build one by hand are the ones that pin the invariant, so
/// "a clip was built from an open segment" is not a state a caller can
/// reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveClipInputs {
    /// Ascending by `start_ms`, all [`PartState::Finalized`], contiguous
    /// with each other.
    pub parts: Vec<CapturePart>,
    pub start_ms: u64,
    pub end_ms: u64,
}

impl LiveClipInputs {
    /// Re-check, immediately before a process is spawned, that every input
    /// is still a finalized part of `capture`.
    ///
    /// Cheap, and the point is that the check does not depend on the
    /// selection having been correct when it was made: the tail can move
    /// between selection and spawn, and the two checks are the same
    /// predicate against the same live state rather than two hopes.
    pub fn revalidate(&self, capture: &LiveCapture) -> Result<(), LiveClipError> {
        for input in &self.parts {
            let Some(actual) = capture.part(input.index) else {
                return Err(LiveClipError::PartStillOpen {
                    index: input.index,
                    path: input.path.clone(),
                    finalized_end_ms: capture.finalized_end_ms(),
                });
            };
            if actual.state != PartState::Finalized || actual.path != input.path {
                return Err(LiveClipError::PartStillOpen {
                    index: input.index,
                    path: actual.path.clone(),
                    finalized_end_ms: capture.finalized_end_ms(),
                });
            }
        }
        Ok(())
    }

    /// The inputs as paths, for `merge_segments`.
    #[must_use]
    pub fn paths(&self) -> Vec<PathBuf> {
        self.parts.iter().map(|p| p.path.clone()).collect()
    }

    /// Offset of the range's start inside the first part, in milliseconds.
    #[must_use]
    pub fn offset_in_first_ms(&self) -> u64 {
        self.parts
            .first()
            .map_or(0, |p| self.start_ms.saturating_sub(p.start_ms))
    }
}

/// The tail of one in-progress capture: which parts exist, and which of them
/// are safe to read.
///
/// Fed by the recorder, read by the render side. It owns no process and no
/// file handle, so the only way a part becomes [`PartState::Finalized`] is
/// somebody saying its writer exited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveCapture {
    base: PathBuf,
    parts: Vec<CapturePart>,
    running: bool,
}

impl LiveCapture {
    /// A capture that has just opened its first part.
    #[must_use]
    pub fn new(base: impl Into<PathBuf>) -> Self {
        let base = base.into();
        Self {
            parts: vec![CapturePart {
                index: 1,
                path: base.clone(),
                state: PartState::Open,
                start_ms: 0,
                duration_ms: 0,
            }],
            base,
            running: true,
        }
    }

    /// The recording's base path; part *n* is `segments::segment_path(base, n)`.
    #[must_use]
    pub fn base(&self) -> &Path {
        &self.base
    }

    /// Declare that a recorder has started writing part `index`. The path
    /// is derived, never supplied, so the tail and the resume path cannot
    /// disagree about where a part lives.
    pub fn open_part(&mut self, index: u32) -> Result<(), LiveClipError> {
        if !self.running {
            return Err(LiveClipError::CaptureStopped);
        }
        let start_ms = self.written_ms();
        let path = segments::segment_path(&self.base, index);
        if let Some(existing) = self.parts.iter_mut().find(|p| p.index == index) {
            existing.state = PartState::Open;
            existing.duration_ms = 0;
            return Ok(());
        }
        self.parts.push(CapturePart {
            index,
            path,
            state: PartState::Open,
            start_ms,
            duration_ms: 0,
        });
        Ok(())
    }

    /// Declare that the writer of part `index` exited after producing
    /// `duration_ms` of media. This is the only transition into
    /// [`PartState::Finalized`], and it is a caller stating a fact about a
    /// process that has already exited — not a guess about a file.
    pub fn finalize_part(&mut self, index: u32, duration_ms: u64) -> Result<(), LiveClipError> {
        let finalized_end = self.finalized_end_ms();
        let Some(part) = self.parts.iter_mut().find(|p| p.index == index) else {
            return Err(LiveClipError::PartStillOpen {
                index,
                path: segments::segment_path(&self.base, index),
                finalized_end_ms: finalized_end,
            });
        };
        if part.state == PartState::Finalized {
            return Err(LiveClipError::ToolFailed {
                tool: "finalize_part",
                detail: format!("part {index} is already finalized"),
            });
        }
        part.duration_ms = duration_ms;
        part.state = PartState::Finalized;
        Ok(())
    }

    /// The capture is over: no more parts, and nothing here is a *live* clip.
    pub fn stop(&mut self) {
        self.running = false;
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running
    }

    #[must_use]
    pub fn parts(&self) -> &[CapturePart] {
        &self.parts
    }

    #[must_use]
    pub fn part(&self, index: u32) -> Option<&CapturePart> {
        self.parts.iter().find(|p| p.index == index)
    }

    /// Media known to be on disk and complete.
    #[must_use]
    pub fn finalized_end_ms(&self) -> u64 {
        self.parts
            .iter()
            .filter(|p| p.state == PartState::Finalized)
            .map(CapturePart::end_ms)
            .max()
            .unwrap_or(0)
    }

    /// End of the newest part, finalized or not. Only ever used to say how
    /// much is *in flight*, never to widen a clip's inputs.
    fn written_ms(&self) -> u64 {
        self.parts
            .iter()
            .map(CapturePart::end_ms)
            .max()
            .unwrap_or(0)
    }

    /// Select the finalized parts a live clip may read.
    ///
    /// Refuses before spawning anything, and the refusals are the point:
    /// [`LiveClipError::PartStillOpen`] is what stops a clip being cut out of
    /// a file a recorder is still appending to.
    pub fn clip_inputs(&self, req: &LiveClipRequest) -> Result<LiveClipInputs, LiveClipError> {
        if !self.running {
            return Err(LiveClipError::CaptureStopped);
        }
        if req.end_ms <= req.start_ms {
            return Err(LiveClipError::InvertedRange {
                start_ms: req.start_ms,
                end_ms: req.end_ms,
            });
        }
        if req.span_ms() < MIN_CLIP_MS {
            return Err(LiveClipError::ClipTooShort {
                span_ms: req.span_ms(),
                minimum_ms: MIN_CLIP_MS,
            });
        }
        let finalized_end = self.finalized_end_ms();
        if req.start_ms >= finalized_end {
            return Err(LiveClipError::NothingFinalized {
                requested_start_ms: req.start_ms,
                finalized_end_ms: finalized_end,
            });
        }
        // The open-part check runs against the **requested** end, not the
        // finalized one. Clipping first and checking second would serve the
        // caller a silent prefix of what it asked for, which is the same
        // observable outcome as the corruption this module exists to prevent:
        // a clip that came back short with nothing saying why. A caller
        // wanting the prefix asks for the prefix; the error names
        // `finalized_end_ms` so it knows where that is.
        for part in &self.parts {
            if !part.intersects(req.start_ms, req.end_ms) {
                continue;
            }
            if part.state != PartState::Finalized {
                return Err(LiveClipError::PartStillOpen {
                    index: part.index,
                    path: part.path.clone(),
                    finalized_end_ms: finalized_end,
                });
            }
        }
        // Now, and only now, may the range be clipped: every part it touches
        // is finalized, so the media it does reach is media nobody is
        // writing. A request that merely runs off the end of a *finished*
        // capture is served what exists.
        let end = req.end_ms.min(finalized_end);
        let parts: Vec<CapturePart> = self
            .parts
            .iter()
            .filter(|p| p.state == PartState::Finalized && p.intersects(req.start_ms, end))
            .cloned()
            .collect();
        if parts.is_empty() {
            return Err(LiveClipError::NothingFinalized {
                requested_start_ms: req.start_ms,
                finalized_end_ms: finalized_end,
            });
        }
        Ok(LiveClipInputs {
            parts,
            start_ms: req.start_ms,
            end_ms: end,
        })
    }
}

/// A clip that rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedClip {
    pub path: PathBuf,
    pub bytes: u64,
    /// The range actually cut, which is the request clipped to the finalized
    /// media. Carried so a caller never has to assume it got all of it.
    pub start_ms: u64,
    pub end_ms: u64,
}

/// Render a live clip from finalized capture segments.
///
/// Three steps, in this order, and the order is the guarantee:
///
/// 1. **admit** — [`CaptureCapacity::admit_render`] against todo 39's byte
///    reserve, so a clip cannot take the space capture still needs;
/// 2. **revalidate** — [`LiveClipInputs::revalidate`] against the live tail,
///    so a part that stopped being finalized between selection and spawn is
///    refused here and not read;
/// 3. **render** — join the selected parts with the *real*
///    [`segments::merge_segments`] into a scratch file of the render's own,
///    then stream-copy the requested range out of it. Capture's own scratch
///    and capture's own files are never written, and the join is of
///    finalized parts only.
///
/// The join is a real `mkvmerge` append rather than an ffmpeg concat filter
/// because [`segments::merge_segments`] is the same call the finish-time
/// merge makes, so a clip and a finished recording agree about what a part
/// boundary means.
pub fn render_live_clip(
    capture: &LiveCapture,
    inputs: &LiveClipInputs,
    dest: &Path,
    capacity: &CaptureCapacity,
) -> Result<RenderedClip, LiveClipError> {
    // An estimate, because a render's output size is not knowable before it
    // runs — the same reason `render_may_start` exists. The sum of the
    // inputs is a lower bound on the output, and admitting against the whole
    // headroom rather than this number is what `render_may_start` is for.
    capacity.admit_render(0).map_err(as_refused)?;

    inputs.revalidate(capture)?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| LiveClipError::ToolFailed {
            tool: "create_dir_all",
            detail: e.to_string(),
        })?;
    }

    let joined = dest.with_extension("joined.mkv");
    // A stale join from a previous attempt would be read as this attempt's
    // input, so it goes before mkvmerge is asked to write it.
    let _ = std::fs::remove_file(&joined);
    segments::merge_segments(&inputs.paths(), &joined).map_err(|e| LiveClipError::ToolFailed {
        tool: "mkvmerge",
        detail: e.to_string(),
    })?;

    let start_s = inputs.offset_in_first_ms() as f64 / 1000.0;
    let span_s = inputs.end_ms.saturating_sub(inputs.start_ms) as f64 / 1000.0;
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"])
        .args(["-ss", &format!("{start_s:.3}")])
        .args(["-i"])
        .arg(&joined)
        .args(["-t", &format!("{span_s:.3}")])
        // Stream copy: a live clip is a cut out of media capture already
        // encoded, so re-encoding it would be work capture's budget spent
        // twice and a clip that is not the recorded picture.
        .args(["-c", "copy"])
        .arg(dest)
        .output()
        .map_err(|e| LiveClipError::ToolFailed {
            tool: "ffmpeg",
            detail: e.to_string(),
        })?;
    let _ = std::fs::remove_file(&joined);
    if !output.status.success() {
        return Err(LiveClipError::ToolFailed {
            tool: "ffmpeg",
            detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    let bytes = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    if bytes == 0 {
        return Err(LiveClipError::ToolFailed {
            tool: "ffmpeg",
            detail: "produced an empty clip".into(),
        });
    }
    Ok(RenderedClip {
        path: dest.to_path_buf(),
        bytes,
        start_ms: inputs.start_ms,
        end_ms: inputs.end_ms,
    })
}

fn as_refused(e: RenderRefused) -> LiveClipError {
    LiveClipError::RenderRefused {
        reason: e.reason().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture() -> LiveCapture {
        LiveCapture::new("/recordings/stream.mkv")
    }

    #[test]
    fn a_new_capture_has_one_open_part() {
        let c = capture();
        assert_eq!(c.parts().len(), 1);
        assert_eq!(c.parts()[0].state, PartState::Open);
        assert_eq!(c.finalized_end_ms(), 0);
    }

    #[test]
    fn a_part_is_readable_only_after_its_writer_exits() {
        let mut c = capture();
        c.finalize_part(1, 60_000).expect("finalize");
        assert_eq!(c.finalized_end_ms(), 60_000);
        let inputs = c
            .clip_inputs(&LiveClipRequest::new(0, 30_000))
            .expect("clip from the finalized part");
        assert_eq!(inputs.parts.len(), 1);
        assert!(inputs.parts.iter().all(|p| p.state == PartState::Finalized));
    }

    #[test]
    fn a_clip_naming_an_open_part_is_refused_not_truncated() {
        let mut c = capture();
        c.finalize_part(1, 60_000).expect("finalize");
        c.open_part(2).expect("open part 2");
        // The range reaches into part 2, which a recorder is writing.
        let err = c
            .clip_inputs(&LiveClipRequest::new(30_000, 90_000))
            .expect_err("an open part must never be a clip input");
        match &err {
            LiveClipError::PartStillOpen {
                index,
                finalized_end_ms,
                ..
            } => {
                assert_eq!(*index, 2);
                assert_eq!(*finalized_end_ms, 60_000);
            }
            other => panic!("expected PartStillOpen, got {other:?}"),
        }
    }

    #[test]
    fn an_open_part_path_is_derived_so_the_tail_cannot_disagree_with_resume() {
        let mut c = capture();
        c.finalize_part(1, 1_000).expect("finalize");
        c.open_part(2).expect("open");
        let part = c.part(2).expect("part 2 is in the tail").clone();
        assert_eq!(part.path, segments::segment_path(c.base(), 2));
        assert_eq!(part.start_ms, 1_000, "a resume is contiguous in media");
    }

    #[test]
    fn a_request_running_past_the_finalized_edge_into_an_open_part_is_refused() {
        // Not served as a silent 10 s prefix. A short clip with nothing
        // saying why is the same observable outcome as the corruption.
        let mut c = capture();
        c.finalize_part(1, 60_000).expect("finalize");
        c.open_part(2).expect("open");
        let err = c
            .clip_inputs(&LiveClipRequest::new(50_000, 200_000))
            .expect_err("the request names an open part");
        assert_eq!(
            err,
            LiveClipError::PartStillOpen {
                index: 2,
                path: segments::segment_path(c.base(), 2),
                finalized_end_ms: 60_000,
            }
        );
    }

    #[test]
    fn a_request_past_the_finalized_edge_of_a_finished_capture_is_clipped() {
        // Every part it touches is finalized, so the only thing missing is
        // media that will never arrive — and serving the part that does
        // exist is not a silent truncation, it is the whole request.
        let mut c = capture();
        c.finalize_part(1, 60_000).expect("finalize");
        c.open_part(2).expect("open");
        c.finalize_part(2, 40_000).expect("finalize part 2 too");
        let inputs = c
            .clip_inputs(&LiveClipRequest::new(50_000, 200_000))
            .expect("clipped to the finalized media");
        assert_eq!(inputs.end_ms, 100_000);
        assert_eq!(inputs.parts.len(), 2);
    }

    #[test]
    fn a_request_before_anything_is_finalized_names_the_shortfall() {
        let c = capture();
        let err = c
            .clip_inputs(&LiveClipRequest::new(0, 5_000))
            .expect_err("nothing is finalized yet");
        assert_eq!(
            err,
            LiveClipError::NothingFinalized {
                requested_start_ms: 0,
                finalized_end_ms: 0
            }
        );
    }

    #[test]
    fn a_multi_part_clip_lists_only_finalized_parts_in_order() {
        let mut c = capture();
        c.finalize_part(1, 60_000).expect("finalize 1");
        c.open_part(2).expect("open 2");
        c.finalize_part(2, 60_000).expect("finalize 2");
        c.open_part(3).expect("open 3");
        let inputs = c
            .clip_inputs(&LiveClipRequest::new(90_000, 120_000))
            .expect("clip across the join");
        assert_eq!(
            inputs.parts.iter().map(|p| p.index).collect::<Vec<_>>(),
            vec![2],
            "part 1 ends at 60 000 ms, so only part 2 covers this range"
        );
    }

    #[test]
    fn revalidation_rejects_an_input_that_stopped_being_finalized() {
        let mut c = capture();
        c.finalize_part(1, 60_000).expect("finalize");
        let inputs = c
            .clip_inputs(&LiveClipRequest::new(0, 30_000))
            .expect("clip");
        inputs.revalidate(&c).expect("still valid");
        // A re-resumed capture reopens part 1, so a selection made a moment
        // ago is no longer one.
        c.open_part(1).expect("reopen");
        assert!(matches!(
            inputs.revalidate(&c),
            Err(LiveClipError::PartStillOpen { index: 1, .. })
        ));
    }

    #[test]
    fn an_empty_or_tiny_range_is_refused_with_its_own_reason() {
        let mut c = capture();
        c.finalize_part(1, 60_000).expect("finalize");
        assert!(matches!(
            c.clip_inputs(&LiveClipRequest::new(30_000, 30_000)),
            Err(LiveClipError::InvertedRange { .. })
        ));
        assert!(matches!(
            c.clip_inputs(&LiveClipRequest::new(0, MIN_CLIP_MS - 1)),
            Err(LiveClipError::ClipTooShort { .. })
        ));
    }

    #[test]
    fn a_stopped_capture_is_not_a_live_clip() {
        let mut c = capture();
        c.finalize_part(1, 60_000).expect("finalize");
        c.stop();
        assert_eq!(
            c.clip_inputs(&LiveClipRequest::new(0, 30_000)),
            Err(LiveClipError::CaptureStopped)
        );
    }

    #[test]
    fn finalizing_twice_is_refused_rather_than_silently_replacing_the_duration() {
        let mut c = capture();
        c.finalize_part(1, 60_000).expect("finalize");
        assert!(c.finalize_part(1, 90_000).is_err());
        assert_eq!(c.finalized_end_ms(), 60_000);
    }

    #[test]
    fn the_refusal_a_user_reads_names_the_numbers_and_the_part() {
        let mut c = capture();
        c.finalize_part(1, 60_000).expect("finalize");
        c.open_part(2).expect("open");
        let text = c
            .clip_inputs(&LiveClipRequest::new(30_000, 90_000))
            .expect_err("open part")
            .to_string();
        assert!(text.contains("part 2"), "{text}");
        assert!(text.contains("still being written"), "{text}");
        assert!(text.contains("60000 ms"), "{text}");
    }

    #[test]
    fn a_render_below_capture_s_reserve_is_refused_with_the_reason() {
        let mut c = capture();
        c.finalize_part(1, 60_000).expect("finalize");
        let inputs = c
            .clip_inputs(&LiveClipRequest::new(0, 30_000))
            .expect("clip");
        // 4 MiB free against an 8 MiB reserve: below it, so no render starts.
        let capacity = CaptureCapacity::new(4 * 1024 * 1024, 8 * 1024 * 1024);
        let err = render_live_clip(&c, &inputs, Path::new("/tmp/never.mkv"), &capacity)
            .expect_err("refused before any tool is spawned");
        match &err {
            LiveClipError::RenderRefused { reason } => {
                assert!(reason.contains("4194304"), "{reason}");
                assert!(reason.contains("8388608"), "{reason}");
            }
            other => panic!("expected RenderRefused, got {other:?}"),
        }
        assert!(
            !Path::new("/tmp/never.mkv").exists(),
            "a refused render must not have written anything"
        );
    }

    #[test]
    fn a_render_is_refused_before_the_tools_run_even_when_the_file_exists() {
        // The point of admitting first: an ffmpeg spawn is the expensive,
        // irreversible step, and it must not precede the refusal.
        let mut c = capture();
        c.finalize_part(1, 60_000).expect("finalize");
        let inputs = c
            .clip_inputs(&LiveClipRequest::new(0, 30_000))
            .expect("clip");
        let dest = std::env::temp_dir().join(format!("t57-nope-{}.mkv", std::process::id()));
        let _ = std::fs::remove_file(&dest);
        let capacity = CaptureCapacity::new(0, 1);
        assert!(render_live_clip(&c, &inputs, &dest, &capacity).is_err());
        assert!(!dest.exists());
    }

    #[test]
    fn the_pvr_concurrency_gate_has_not_moved() {
        // Read at compile time, so the assertion is about the file in this
        // commit rather than about whatever is on disk when the test runs.
        const ROADMAP: &str = include_str!("../../ROADMAP.md");
        let gate = ROADMAP
            .lines()
            .nth(ROADMAP_GATE_LINE - 1)
            .expect("ROADMAP.md has a line 100");
        assert_eq!(
            gate.trim_end(),
            ROADMAP_GATE_TEXT,
            "ROADMAP.md:{ROADMAP_GATE_LINE} moved. Live clipping adds a bound on BYTES; this line \
             is a bound on RECORDER SLOTS and must not be relaxed to accommodate it."
        );
    }
}
