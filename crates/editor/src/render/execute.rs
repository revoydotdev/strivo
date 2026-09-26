//! The two callers, and the only two.
//!
//! [`preview`] and [`export`] both call [`compile`](super::compile) and differ
//! in what they **do** with the plan, never in the plan they get. That is the
//! whole claim this module makes, and it is checkable: [`CompiledPlan`] has no
//! public constructor, so a caller cannot reach a plan except through here.
//!
//! A preview creates no file and spawns nothing, so a preview cannot leave a
//! scratch directory behind — which is the difference between the two callers
//! that is about behaviour rather than about the plan.

use std::path::Path;
use std::process::Command;

use super::approximate::{approximations, Approximation};
use super::compile::compile;
use super::error::RenderError;
use super::invoke;
use super::plan::CompiledPlan;
use super::request::{CaptionMode, RenderRequest};
use crate::{CutKind, Edl};

/// A preview: the plan, what a preview cannot show about it, and how to get
/// the thing that can.
#[derive(Debug, Clone, PartialEq)]
pub struct Preview {
    /// The same value [`export`] compiles. Not a projection of it.
    pub plan: CompiledPlan,
    /// Derived from the plan by
    /// [`approximations`](super::approximations) — never stored on the plan,
    /// so preview and export cannot disagree about what is approximate.
    pub approximate: Vec<Approximation>,
    /// The route that renders this exact plan, for a user who needs the real
    /// pixels rather than the labelled approximation.
    pub rendered_preview: String,
}

/// A completed export.
#[derive(Debug, Clone, PartialEq)]
pub struct Export {
    /// The same value [`preview`] compiles.
    pub plan: CompiledPlan,
    pub output: std::path::PathBuf,
    pub bytes: u64,
}

/// Compile a request without touching the filesystem or a media tool.
///
/// The preview caller. Its whole job is to answer "what does this render as?"
/// honestly, which means returning the plan *and* naming the stages a
/// non-executing view cannot be exact about.
pub fn preview(req: &RenderRequest) -> Result<Preview, RenderError> {
    let plan = compile(req)?;
    Ok(Preview {
        rendered_preview: format!("/api/v1/plugins/editor/{}/render", req.subject),
        approximate: approximations(&plan),
        plan,
    })
}

/// Compile a request and render it.
///
/// The export caller. Everything below `compile` is execution: the plan has
/// already decided what the output is, and this function only does it.
pub fn export(
    req: &RenderRequest,
    edl: &Edl,
    output: &Path,
    subtitle: Option<&Path>,
) -> Result<Export, RenderError> {
    let plan = compile(req)?;
    if plan.caption_mode == CaptionMode::BurnedIn {
        if subtitle.is_none() {
            return Err(RenderError::BurnWithoutSubtitle);
        }
        if !super::invoke::video_is_passthrough(&plan.video_filter_complex) {
            return Err(RenderError::BurnOverVideoGraph);
        }
    }
    let bytes = execute(&plan, edl, output, subtitle)?;
    Ok(Export {
        plan,
        output: output.to_path_buf(),
        bytes,
    })
}

/// Removes the scratch directory on drop, including on an early `?` return or
/// a panic — so a failed render never leaves per-clip sub-renders behind.
struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn io(detail: impl std::fmt::Display) -> RenderError {
    RenderError::Execute {
        detail: detail.to_string(),
    }
}

fn execute(
    plan: &CompiledPlan,
    edl: &Edl,
    output: &Path,
    subtitle: Option<&Path>,
) -> Result<u64, RenderError> {
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(io)?;
    // Named after the plan's subject, not a fixed directory: two exports of two
    // revisions at once must not share a scratch directory, and a fixed name
    // would be a second render path's race waiting to happen.
    let scratch = parent.join(format!(".plan-{}", scrub(&plan.subject)));
    let _scratch = Scratch(scratch.clone());
    std::fs::create_dir_all(&scratch).map_err(io)?;

    let mut clips = Vec::with_capacity(plan.clips.len());
    for (index, clip) in plan.clips.iter().enumerate() {
        let entry = edl.cuts.get(clip.cut).ok_or_else(|| {
            io(format!(
                "plan names cut {}, which the revision has no entry for",
                clip.cut
            ))
        })?;
        let source = match &entry.kind {
            CutKind::Source { source_path } => source_path.clone(),
            CutKind::Broll { broll_path } => broll_path.clone(),
        };
        let path = scratch.join(format!("clip_{index:03}.mkv"));
        let args = invoke::clip_args(plan, clip, &source, &path);
        run(&args)?;
        clips.push(path);
    }

    let list_path = scratch.join("concat.txt");
    let listing: String = clips
        .iter()
        .map(|p| format!("file '{}'\n", p.display()))
        .collect();
    std::fs::write(&list_path, listing).map_err(io)?;
    run(&invoke::concat_args(plan, &list_path, output, subtitle))?;

    if let Some(target) = &plan.loudness {
        let measured = measure(output, target)?;
        // ffmpeg refuses to write where it reads ("Output … same as Input"),
        // so the second pass lands in the scratch directory and is moved into
        // place. Writing in place would fail the whole render at exit 234.
        let normalised = scratch.join("normalised.mkv");
        run(&invoke::normalize_args(
            output,
            &normalised,
            target,
            &measured,
        ))?;
        std::fs::rename(&normalised, output).map_err(io)?;
    }
    std::fs::metadata(output).map(|m| m.len()).map_err(io)
}

/// A filename-safe stem for a scratch directory. A recording id is already
/// filename-safe, but a plan's subject may be a project id, and a path
/// separator in a scratch path would write outside the output directory.
fn scrub(subject: &str) -> String {
    subject
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn run(args: &[String]) -> Result<(), RenderError> {
    let status = Command::new("ffmpeg").args(args).status().map_err(io)?;
    if status.success() {
        Ok(())
    } else {
        Err(io(format!("ffmpeg exited {status}")))
    }
}

/// `loudnorm` is two-pass: the second pass needs the first pass's readings,
/// which only exist once the signal has been analysed. The plan carries the
/// **target**; this runs the measurement and re-runs the render with it, which
/// is what makes the loudness control observable in the decoded artifact
/// rather than only in the plan.
fn measure(output: &Path, target: &super::plan::Loudness) -> Result<String, RenderError> {
    let filter = invoke::measure_filter(target);
    let out = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-nostats",
            "-i",
            &output.to_string_lossy(),
            "-af",
            &filter,
            "-f",
            "null",
            "-",
        ])
        .output()
        .map_err(io)?;
    invoke::parse_measurements(&String::from_utf8_lossy(&out.stderr))
}
