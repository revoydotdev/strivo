//! strivo-editor — edit-decision-list (EDL) model + ffmpeg render.
//!
//! The DAW-vision capability: non-destructive editing. The EDL is an
//! ordered list of [`Cut`]s; each cut points into a source file and
//! says "use this start..end slice". The streamer drops cuts, ripple-
//! deletes ranges, inserts B-roll clips between cuts, and renders the
//! whole thing to one final video — without touching the source.
//!
//! This iteration ships:
//!
//!   * The EDL data model: [`Edl`], [`Cut`], [`CutKind::{Source, Broll}`].
//!   * Five edit operations (all pure, fully unit-tested):
//!       - [`Edl::from_source`] — open a fresh EDL covering the whole file
//!       - [`Edl::split_at`]    — split the cut covering `t` into two
//!       - [`Edl::delete_range`] — ripple-delete a [lo, hi] span
//!       - [`Edl::insert_broll`] — drop a B-roll cut at index `i`
//!       - [`Edl::total_duration`] — output length after all edits
//!   * [`render_edl`] — calls ffmpeg with the concat demuxer to bake the
//!     final file. Pure passthrough re-encode (-c copy on simple cuts,
//!     -c:v libx264 -c:a aac on cross-codec mixes).
//!   * [`store`] — SQLite cache keyed on `recording_id` so the SPA can
//!     reload a draft EDL across page refreshes.
//!
//! Storage is in [`store`]; the editor crate is pure-data + thin
//! ffmpeg wrapper.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub mod render;
pub mod store;
pub mod time;
pub mod timeline;
pub mod transcript;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CutKind {
    /// A slice of the source recording.
    Source { source_path: String },
    /// A B-roll insert pointing at a different file.
    Broll { broll_path: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cut {
    pub start_sec: f32,
    pub end_sec: f32,
    pub kind: CutKind,
    /// Optional fade in/out duration in seconds (applied at render).
    #[serde(default)]
    pub fade_in_sec: f32,
    #[serde(default)]
    pub fade_out_sec: f32,
}

impl Cut {
    pub fn duration(&self) -> f32 {
        (self.end_sec - self.start_sec).max(0.0)
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Edl {
    pub recording_id: String,
    pub cuts: Vec<Cut>,
}

impl Edl {
    /// Open a fresh EDL covering the entire source.
    pub fn from_source(recording_id: &str, source_path: &str, duration_sec: f32) -> Self {
        Self {
            recording_id: recording_id.to_string(),
            cuts: vec![Cut {
                start_sec: 0.0,
                end_sec: duration_sec.max(0.0),
                kind: CutKind::Source {
                    source_path: source_path.to_string(),
                },
                fade_in_sec: 0.0,
                fade_out_sec: 0.0,
            }],
        }
    }

    /// Total output duration after all edits.
    pub fn total_duration(&self) -> f32 {
        self.cuts.iter().map(|c| c.duration()).sum()
    }

    /// Split the cut covering output-time `t` into two halves at that
    /// boundary. No-op when `t` falls outside the EDL or on an exact
    /// boundary. Returns the index of the newly created right half.
    pub fn split_at(&mut self, t: f32) -> Option<usize> {
        if t < 0.0 {
            return None;
        }
        let mut elapsed = 0.0_f32;
        for i in 0..self.cuts.len() {
            let cut = &self.cuts[i];
            let cut_dur = cut.duration();
            let cut_end = elapsed + cut_dur;
            if t > elapsed + 0.001 && t < cut_end - 0.001 {
                let offset = t - elapsed;
                let mut right = cut.clone();
                // The right half inherits a fresh fade_out and loses
                // fade_in (it's mid-stream now); the left half inherits
                // fade_in and loses fade_out.
                let left_fade_out = right.fade_out_sec;
                right.fade_in_sec = 0.0;
                let new_split = cut.start_sec + offset;
                right.start_sec = new_split;
                let left_end = new_split;
                self.cuts[i].end_sec = left_end;
                self.cuts[i].fade_out_sec = 0.0;
                right.fade_out_sec = left_fade_out;
                self.cuts.insert(i + 1, right);
                return Some(i + 1);
            }
            elapsed = cut_end;
        }
        None
    }

    /// Ripple-delete the output-time span `[lo, hi]`. Cuts wholly
    /// inside the span are removed; partial overlaps are trimmed.
    /// Returns the number of cuts removed (post-trim).
    pub fn delete_range(&mut self, lo: f32, hi: f32) -> usize {
        if hi <= lo {
            return 0;
        }
        // Walk forward, mapping each cut's output range and adjusting.
        let mut elapsed = 0.0_f32;
        let mut new_cuts: Vec<Cut> = Vec::with_capacity(self.cuts.len());
        let mut removed = 0;
        for cut in self.cuts.iter() {
            let cut_dur = cut.duration();
            let out_lo = elapsed;
            let out_hi = elapsed + cut_dur;
            elapsed = out_hi;
            // Fully outside the kill range → keep.
            if out_hi <= lo || out_lo >= hi {
                new_cuts.push(cut.clone());
                continue;
            }
            // Fully inside → drop.
            if out_lo >= lo && out_hi <= hi {
                removed += 1;
                continue;
            }
            // Partial overlap from the left.
            if out_lo < lo && out_hi <= hi {
                let mut trimmed = cut.clone();
                trimmed.end_sec = cut.start_sec + (lo - out_lo);
                new_cuts.push(trimmed);
                continue;
            }
            // Partial overlap from the right.
            if out_lo >= lo && out_hi > hi {
                let mut trimmed = cut.clone();
                trimmed.start_sec = cut.start_sec + (hi - out_lo);
                new_cuts.push(trimmed);
                continue;
            }
            // Span sits in the middle → split into left + right.
            let mut left = cut.clone();
            left.end_sec = cut.start_sec + (lo - out_lo);
            let mut right = cut.clone();
            right.start_sec = cut.start_sec + (hi - out_lo);
            // Re-evaluate fade boundaries: middle fades inherited by
            // neither half (they're now interior to a delete event).
            left.fade_out_sec = 0.0;
            right.fade_in_sec = 0.0;
            new_cuts.push(left);
            new_cuts.push(right);
        }
        self.cuts = new_cuts;
        removed
    }

    /// Insert a B-roll cut at position `at_idx`. If `at_idx` is past
    /// the end, append.
    pub fn insert_broll(&mut self, at_idx: usize, broll_path: &str, start_sec: f32, end_sec: f32) {
        let cut = Cut {
            start_sec: start_sec.max(0.0),
            end_sec: end_sec.max(start_sec),
            kind: CutKind::Broll {
                broll_path: broll_path.to_string(),
            },
            fade_in_sec: 0.0,
            fade_out_sec: 0.0,
        };
        if at_idx >= self.cuts.len() {
            self.cuts.push(cut);
        } else {
            self.cuts.insert(at_idx, cut);
        }
    }

    /// Set per-cut fades. Clamps to non-negative.
    pub fn set_fades(&mut self, at_idx: usize, fade_in: f32, fade_out: f32) -> bool {
        if let Some(cut) = self.cuts.get_mut(at_idx) {
            cut.fade_in_sec = fade_in.max(0.0);
            cut.fade_out_sec = fade_out.max(0.0);
            true
        } else {
            false
        }
    }

    /// Drop empty / inverted cuts.
    pub fn compact(&mut self) {
        self.cuts.retain(|c| c.duration() > 0.001);
    }
}

/// Render the EDL to `output` using ffmpeg's concat demuxer.
///
/// Strategy: write a temporary concat list, then `ffmpeg -f concat
/// -safe 0 -i list.txt -c copy out.<ext>` for single-source EDLs
/// (fast path) or `-c:v libx264 -c:a aac` when B-roll mixes codecs.
/// We pass each cut as a sub-clip via the concat list.
pub fn render_edl(edl: &Edl, output: &Path) -> Result<u64> {
    render_edl_with_filter(edl, output, None)
}

/// Same as [`render_edl`], but applies an extra `-filter_complex` chain on
/// the final concat step. The chain must consume `[0:v]` (the concat output)
/// and produce `[vout]`; we pass `-map [vout] -map 0:a?` to keep audio.
/// The branding crate emits exactly that shape.
pub fn render_edl_with_filter(
    edl: &Edl,
    output: &Path,
    extra_filter_complex: Option<&str>,
) -> Result<u64> {
    render_edl_with_filters(edl, output, extra_filter_complex, None)
}

/// Same as [`render_edl_with_filter`] but also accepts an audio-only
/// filter chain (e.g. the asendcmd-driven volume automation the
/// strivo-automation crate emits). When supplied, the concat path swaps
/// from `-c copy` to a transcode with the audio filter applied.
pub fn render_edl_with_filters(
    edl: &Edl,
    output: &Path,
    extra_filter_complex: Option<&str>,
    extra_audio_filter: Option<&str>,
) -> Result<u64> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let edl_dir = output
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    // Per attempt, like the plan executor's scratch. A fixed `.edl-temp` under
    // the output parent was a shared directory: two renders into one output
    // directory overwrote each other's per-cut sub-clips, and the first to
    // finish deleted the other's working directory. Nothing in production calls
    // this renderer any more, but it is `pub`, so a new caller would have
    // inherited the same defect.
    let temp_dir = render::attempt::Scratch(render::attempt::scratch_dir(
        &edl_dir,
        &edl.recording_id,
        &render::AttemptId::mint(),
    ));
    std::fs::create_dir_all(&*temp_dir).ok();

    // Strategy: for each cut, render a temporary sub-clip with seek
    // + duration, then concat the sub-clips. Lossless on the copy
    // path; transcodes when fades are present.
    let mut sub_clip_paths: Vec<std::path::PathBuf> = Vec::new();
    for (i, cut) in edl.cuts.iter().enumerate() {
        let path = match &cut.kind {
            CutKind::Source { source_path } => source_path,
            CutKind::Broll { broll_path } => broll_path,
        };
        let sub_clip_path = temp_dir.join(format!("clip_{i:03}.mkv"));
        let start = format!("{:.3}", cut.start_sec);
        let dur = format!("{:.3}", cut.duration().max(0.001));
        let needs_transcode = cut.fade_in_sec > 0.0 || cut.fade_out_sec > 0.0;
        if needs_transcode {
            let vfilter = format!(
                "fade=t=in:st=0:d={:.3},fade=t=out:st={:.3}:d={:.3}",
                cut.fade_in_sec,
                (cut.duration() - cut.fade_out_sec).max(0.0),
                cut.fade_out_sec
            );
            let status = Command::new("ffmpeg")
                .args(["-y", "-hide_banner", "-loglevel", "error"])
                .args(["-ss", &start])
                .arg("-i")
                .arg(path)
                .args([
                    "-t", &dur, "-vf", &vfilter, "-c:v", "libx264", "-c:a", "aac",
                ])
                .arg(&sub_clip_path)
                .status()
                .context("ffmpeg sub-clip (fade)")?;
            if !status.success() {
                anyhow::bail!("ffmpeg sub-clip exited {status}");
            }
        } else {
            let status = Command::new("ffmpeg")
                .args(["-y", "-hide_banner", "-loglevel", "error"])
                .args(["-ss", &start])
                .arg("-i")
                .arg(path)
                .args(["-t", &dur, "-c", "copy"])
                .arg(&sub_clip_path)
                .status()
                .context("ffmpeg sub-clip")?;
            if !status.success() {
                anyhow::bail!("ffmpeg sub-clip exited {status}");
            }
        }
        sub_clip_paths.push(sub_clip_path);
    }
    let list_path = temp_dir.join("concat.txt");
    let mut list = String::new();
    for p in &sub_clip_paths {
        list.push_str(&format!("file '{}'\n", p.display()));
    }
    std::fs::write(&list_path, list).context("write concat list")?;
    let mut concat = Command::new("ffmpeg");
    concat
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args(["-f", "concat", "-safe", "0"])
        .arg("-i")
        .arg(&list_path);
    // Passthrough detectors. strivo-automation emits "anull" + strivo-
    // branding emits `[0:v]copy[vout]` when their input is empty; both
    // round-trip to the fast `-c copy` path here.
    let audio_pass = extra_audio_filter
        .map(|a| a.trim() == "anull" || a.trim().is_empty())
        .unwrap_or(true);
    let video_pass = extra_filter_complex
        .map(|fc| fc.trim() == "[0:v]copy[vout]")
        .unwrap_or(true);
    if video_pass && audio_pass {
        concat.args(["-c", "copy"]);
    } else {
        // Build a unified filter_complex so video + audio are routed via
        // named labels and we don't have to worry about -af/-map ordering.
        let video_chain = if video_pass {
            "[0:v]copy[vout]".to_string()
        } else {
            extra_filter_complex.unwrap().to_string()
        };
        let audio_chain = if audio_pass {
            "[0:a]anull[aout]".to_string()
        } else {
            format!("[0:a]{}[aout]", extra_audio_filter.unwrap())
        };
        let combined = format!("{video_chain};{audio_chain}");
        concat
            .args(["-filter_complex", &combined])
            .args(["-map", "[vout]"])
            .args(["-map", "[aout]"])
            .args(["-c:v", "libx264", "-c:a", "aac"]);
    }
    let status = concat.arg(output).status().context("ffmpeg concat")?;
    if !status.success() {
        anyhow::bail!("ffmpeg concat exited {status}");
    }
    // `temp_dir` is a `TempDirGuard`; its `Drop` removes the scratch
    // directory here on the success path, and also on any early `?`
    // return or panic above, so a failed render never leaks sub-clips.
    let bytes = std::fs::metadata(output).context("stat output")?.len();
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every scratch directory still under `parent`, whatever it is called.
    fn scratch_entries(parent: &std::path::Path) -> Vec<String> {
        std::fs::read_dir(parent)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".plan-") || name.starts_with(".edl-temp"))
            .collect()
    }

    fn src_cut(start: f32, end: f32) -> Cut {
        Cut {
            start_sec: start,
            end_sec: end,
            kind: CutKind::Source {
                source_path: "/x".into(),
            },
            fade_in_sec: 0.0,
            fade_out_sec: 0.0,
        }
    }

    #[test]
    fn fresh_edl_covers_full_source() {
        let e = Edl::from_source("r1", "/x", 120.0);
        assert_eq!(e.cuts.len(), 1);
        assert_eq!(e.total_duration(), 120.0);
    }

    #[test]
    fn split_at_midpoint_creates_two_cuts() {
        let mut e = Edl::from_source("r1", "/x", 120.0);
        let new_idx = e.split_at(60.0).unwrap();
        assert_eq!(new_idx, 1);
        assert_eq!(e.cuts.len(), 2);
        assert_eq!(e.cuts[0].end_sec, 60.0);
        assert_eq!(e.cuts[1].start_sec, 60.0);
        assert_eq!(e.total_duration(), 120.0);
    }

    #[test]
    fn split_at_boundary_is_noop() {
        let mut e = Edl::from_source("r1", "/x", 120.0);
        assert!(e.split_at(0.0).is_none());
        assert!(e.split_at(120.0).is_none());
        assert_eq!(e.cuts.len(), 1);
    }

    #[test]
    fn delete_range_inside_cut_splits_around_hole() {
        let mut e = Edl::from_source("r1", "/x", 120.0);
        let removed = e.delete_range(30.0, 60.0);
        assert_eq!(removed, 0); // no full cut removed; only trimmed
        assert_eq!(e.cuts.len(), 2);
        assert_eq!(e.cuts[0].end_sec, 30.0);
        assert_eq!(e.cuts[1].start_sec, 60.0);
        assert_eq!(e.total_duration(), 90.0);
    }

    #[test]
    fn delete_range_drops_fully_inside_cuts() {
        let mut e = Edl {
            recording_id: "r1".into(),
            cuts: vec![src_cut(0.0, 30.0), src_cut(30.0, 60.0), src_cut(60.0, 90.0)],
        };
        let removed = e.delete_range(30.0, 60.0);
        assert_eq!(removed, 1);
        assert_eq!(e.cuts.len(), 2);
        assert_eq!(e.total_duration(), 60.0);
    }

    #[test]
    fn delete_range_left_overlap_trims_end() {
        let mut e = Edl {
            recording_id: "r1".into(),
            cuts: vec![src_cut(0.0, 60.0), src_cut(60.0, 120.0)],
        };
        let _ = e.delete_range(40.0, 80.0);
        // First cut trimmed: keep 0..40; second cut trimmed: keep 80..120 (i.e. 60+20 onward).
        assert_eq!(e.cuts[0].end_sec, 40.0);
        assert_eq!(e.cuts[1].start_sec, 80.0);
        assert!((e.total_duration() - 80.0).abs() < 1e-3);
    }

    #[test]
    fn delete_inverted_range_is_noop() {
        let mut e = Edl::from_source("r1", "/x", 60.0);
        assert_eq!(e.delete_range(50.0, 10.0), 0);
        assert_eq!(e.cuts.len(), 1);
    }

    #[test]
    fn insert_broll_at_index() {
        let mut e = Edl::from_source("r1", "/x", 60.0);
        e.insert_broll(1, "/broll.mkv", 0.0, 5.0);
        assert_eq!(e.cuts.len(), 2);
        match &e.cuts[1].kind {
            CutKind::Broll { broll_path } => assert_eq!(broll_path, "/broll.mkv"),
            _ => panic!("expected B-roll"),
        }
        assert_eq!(e.total_duration(), 65.0);
    }

    #[test]
    fn insert_broll_past_end_appends() {
        let mut e = Edl::from_source("r1", "/x", 60.0);
        e.insert_broll(999, "/broll.mkv", 0.0, 5.0);
        assert_eq!(e.cuts.len(), 2);
        assert_eq!(e.cuts.last().unwrap().duration(), 5.0);
    }

    #[test]
    fn set_fades_clamps_negative_to_zero() {
        let mut e = Edl::from_source("r1", "/x", 60.0);
        assert!(e.set_fades(0, -1.0, -2.0));
        assert_eq!(e.cuts[0].fade_in_sec, 0.0);
        assert_eq!(e.cuts[0].fade_out_sec, 0.0);
    }

    #[test]
    fn compact_drops_zero_duration_cuts() {
        let mut e = Edl {
            recording_id: "r1".into(),
            cuts: vec![src_cut(0.0, 30.0), src_cut(30.0, 30.0), src_cut(30.0, 60.0)],
        };
        e.compact();
        assert_eq!(e.cuts.len(), 2);
    }

    #[test]
    fn total_duration_handles_mixed_kinds() {
        let mut e = Edl::from_source("r1", "/x", 60.0);
        e.insert_broll(1, "/broll.mkv", 10.0, 25.0); // 15s broll
        assert!((e.total_duration() - 75.0).abs() < 1e-3);
    }

    #[test]
    fn failed_render_leaves_no_scratch_directory() {
        // Point at a source file that does not exist: the first sub-clip
        // ffmpeg pass fails and render_edl_with_filters bails out via `?`
        // before ever reaching the concat step or the old post-concat
        // cleanup line. This is hermetic — it needs ffmpeg on PATH (same
        // as every other render test would), but not any real media file.
        let dir = std::env::temp_dir().join(format!(
            "strivo-editor-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let output = dir.join("out.mkv");
        let edl = Edl {
            recording_id: "r1".into(),
            cuts: vec![Cut {
                start_sec: 0.0,
                end_sec: 5.0,
                kind: CutKind::Source {
                    source_path: dir.join("does-not-exist.mkv").display().to_string(),
                },
                fade_in_sec: 0.0,
                fade_out_sec: 0.0,
            }],
        };

        let result = render_edl(&edl, &output);
        assert!(
            result.is_err(),
            "expected render to fail on a missing source file"
        );

        // Every scratch entry, not one named path: the directory is named per
        // attempt, so asserting a fixed name is gone passes whether or not the
        // render ever made one.
        assert_eq!(
            scratch_entries(&dir),
            Vec::<String>::new(),
            "scratch left behind after a failed render"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression test for the scenario the finding was actually written
    /// about: a *multi-cut* render where an earlier cut succeeds (writing
    /// a real sub-clip file to the `.edl-temp` scratch dir) and a later
    /// cut fails. The single-cut test above only proves the guard fires
    /// against an empty scratch dir (ffmpeg fails on the very first
    /// sub-clip, before anything is written); this proves it also cleans
    /// up real sub-clip files left behind by cuts that already succeeded.
    #[test]
    fn failed_render_after_partial_success_leaves_no_scratch_directory() {
        let dir = std::env::temp_dir().join(format!(
            "strivo-editor-test-multi-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        // Synthesize a small, real, valid source video with ffmpeg's
        // testsrc generator rather than committing a binary fixture.
        let source = dir.join("source.mp4");
        let gen_status = Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error"])
            .args([
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=2:size=128x72:rate=10",
            ])
            .args(["-pix_fmt", "yuv420p"])
            .arg(&source)
            .status()
            .expect("ffmpeg must be on PATH to generate the test fixture");
        assert!(gen_status.success(), "failed to synthesize source fixture");
        assert!(source.exists(), "source fixture was not written");

        // Pre-flight proof (not part of the render under test): run the
        // *exact* sub-clip command render_edl_with_filters issues for cut
        // 0 — same flags, same source, same range — into a scratch
        // location of our own, and confirm it succeeds and produces a
        // real, non-empty file. This establishes empirically that this
        // specific ffmpeg invocation, against this fixture, does succeed
        // and does write sub-clip output; combined with the fact that the
        // render loop is a strictly sequential `for` over
        // `Command::status()` calls (each one blocks until the child
        // process exits before the next iteration starts), this proves
        // cut 0's sub-clip is fully written to disk before cut 1 (which
        // points at a nonexistent source and fails) is even attempted —
        // there is no concurrency in the loop that could reorder this.
        let preflight_clip = dir.join("preflight_clip_000.mkv");
        let preflight_status = Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error"])
            .args(["-ss", "0.000"])
            .arg("-i")
            .arg(&source)
            .args(["-t", "1.000", "-c", "copy"])
            .arg(&preflight_clip)
            .status()
            .unwrap();
        assert!(
            preflight_status.success(),
            "the exact sub-clip command cut 0 will run must itself succeed against this fixture"
        );
        let preflight_meta = std::fs::metadata(&preflight_clip)
            .expect("preflight sub-clip should exist and be readable");
        assert!(
            preflight_meta.len() > 0,
            "preflight sub-clip should be a real, non-empty file"
        );
        let _ = std::fs::remove_file(&preflight_clip);

        let output = dir.join("out.mkv");
        let edl = Edl {
            recording_id: "r1".into(),
            cuts: vec![
                // Cut 0: real source, valid range — succeeds and writes
                // .edl-temp/clip_000.mkv.
                Cut {
                    start_sec: 0.0,
                    end_sec: 1.0,
                    kind: CutKind::Source {
                        source_path: source.display().to_string(),
                    },
                    fade_in_sec: 0.0,
                    fade_out_sec: 0.0,
                },
                // Cut 1: nonexistent source — ffmpeg fails partway
                // through the render, after cut 0's sub-clip already
                // exists on disk.
                Cut {
                    start_sec: 0.0,
                    end_sec: 1.0,
                    kind: CutKind::Source {
                        source_path: dir.join("does-not-exist.mkv").display().to_string(),
                    },
                    fade_in_sec: 0.0,
                    fade_out_sec: 0.0,
                },
            ],
        };

        let result = render_edl(&edl, &output);
        assert!(
            result.is_err(),
            "expected render to fail on cut 1's missing source file"
        );

        assert_eq!(
            scratch_entries(&dir),
            Vec::<String>::new(),
            "scratch left behind after a failed multi-cut render, cut 0's \
             sub-clip included"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
