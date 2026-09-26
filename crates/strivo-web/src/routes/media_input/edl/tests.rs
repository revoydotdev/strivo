//! Stage 3 against real files and no child process.
//!
//! `admit_edl` canonicalises the paths it is given, so these tests need files
//! that exist — but nothing here spawns `ffprobe`. The durations are supplied
//! by hand in the `admitted` map, which is what makes each refusal attributable
//! to the range check alone: a test that failed because the probe said
//! something else would be testing stage 2 under stage 3's name.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use strivo_editor::time::{MediaTime, TimeError};
use strivo_editor::{Cut, CutKind, Edl};

use super::{admit_edl, AdmittedMedia};
use crate::routes::media_input::{MediaRejection, PathRejection};

/// A root holding `names`, each a 4-byte regular file.
fn root_with(names: &[&str]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("recordings");
    std::fs::create_dir_all(&root).expect("root");
    for name in names {
        std::fs::write(root.join(name), b"mkv!").expect("fixture");
    }
    (dir, root)
}

fn source_cut(path: &str, start_sec: f32, end_sec: f32) -> Cut {
    Cut {
        start_sec,
        end_sec,
        kind: CutKind::Source {
            source_path: path.to_string(),
        },
        fade_in_sec: 0.0,
        fade_out_sec: 0.0,
    }
}

fn broll_cut(path: &str, start_sec: f32, end_sec: f32) -> Cut {
    Cut {
        start_sec,
        end_sec,
        kind: CutKind::Broll {
            broll_path: path.to_string(),
        },
        fade_in_sec: 0.0,
        fade_out_sec: 0.0,
    }
}

fn edl_of(cuts: Vec<Cut>) -> Edl {
    Edl {
        recording_id: "11111111-1111-1111-1111-111111111111".into(),
        cuts,
    }
}

fn admitted(root: &Path, name: &str, duration_us: i64) -> HashMap<PathBuf, AdmittedMedia> {
    let real = root.join(name).canonicalize().expect("canonical");
    let mut map = HashMap::new();
    map.insert(
        real.clone(),
        AdmittedMedia {
            path: real,
            bytes: 4,
            duration: MediaTime::from_micros(duration_us).expect("positive"),
        },
    );
    map
}

fn admitted_4s(root: &Path) -> HashMap<PathBuf, AdmittedMedia> {
    admitted(root, "a.mkv", 4_000_000)
}

#[test]
fn an_edit_with_no_cuts_is_refused_by_name() {
    let (_dir, root) = root_with(&["a.mkv"]);
    let err = admit_edl(&root, &edl_of(vec![]), &HashMap::new()).expect_err("empty edl");
    assert_eq!(err, MediaRejection::NoCuts);
    assert_eq!(err.code(), "media-input/no-cuts");
}

#[test]
fn a_cut_past_the_end_of_its_source_is_refused_with_both_durations() {
    let (_dir, root) = root_with(&["a.mkv"]);
    let real = root.join("a.mkv").canonicalize().expect("canonical");
    let edl = edl_of(vec![source_cut(real.to_str().expect("utf8"), 1.0, 9.0)]);
    let err = admit_edl(&root, &edl, &admitted_4s(&root)).expect_err("out of range");
    assert_eq!(
        err,
        MediaRejection::CutOutOfRange {
            cut: 0,
            start_us: 1_000_000,
            end_us: 9_000_000,
            source_us: 4_000_000,
        }
    );
    assert_eq!(err.code(), "media-input/cut-out-of-range");
}

#[test]
fn the_offending_cut_index_is_reported_not_the_first_one() {
    let (_dir, root) = root_with(&["a.mkv", "b.mkv"]);
    let a = root.join("a.mkv").canonicalize().expect("canonical");
    let b = root.join("b.mkv").canonicalize().expect("canonical");
    let mut map = admitted_4s(&root);
    map.insert(
        b.clone(),
        AdmittedMedia {
            path: b.clone(),
            bytes: 4,
            duration: MediaTime::from_micros(1_000_000).expect("positive"),
        },
    );
    let edl = edl_of(vec![
        source_cut(a.to_str().expect("utf8"), 0.0, 1.0),
        source_cut(b.to_str().expect("utf8"), 0.0, 4.0),
    ]);
    let err = admit_edl(&root, &edl, &map).expect_err("second cut is out of range");
    assert_eq!(
        err,
        MediaRejection::CutOutOfRange {
            cut: 1,
            start_us: 0,
            end_us: 4_000_000,
            source_us: 1_000_000,
        }
    );
}

#[test]
fn a_cut_ending_exactly_on_the_source_end_is_inside_it() {
    let (_dir, root) = root_with(&["a.mkv"]);
    let real = root.join("a.mkv").canonicalize().expect("canonical");
    let edl = edl_of(vec![source_cut(real.to_str().expect("utf8"), 0.0, 4.0)]);
    let admitted = admit_edl(&root, &edl, &admitted_4s(&root)).expect("boundary is legal");
    assert_eq!(
        admitted.output,
        MediaTime::from_micros(4_000_000).expect("positive")
    );
}

#[test]
fn a_cut_whose_own_coordinates_are_not_media_time_is_refused_by_time_error() {
    for (start, end, expected) in [
        (f32::NAN, 1.0, TimeError::NotFinite { field: "start_sec" }),
        (
            0.0,
            f32::INFINITY,
            TimeError::NotFinite { field: "end_sec" },
        ),
        (-1.0, 1.0, TimeError::NegativeSeconds { field: "start_sec" }),
    ] {
        let (_dir, root) = root_with(&["a.mkv"]);
        let real = root.join("a.mkv").canonicalize().expect("canonical");
        let edl = edl_of(vec![source_cut(real.to_str().expect("utf8"), start, end)]);
        let err = admit_edl(&root, &edl, &admitted_4s(&root)).expect_err("not a media coordinate");
        assert_eq!(
            err,
            MediaRejection::CutRange {
                cut: 0,
                error: expected
            },
            "start={start} end={end}"
        );
        assert_eq!(err.code(), "media-input/cut-range");
    }
}

#[test]
fn an_inverted_cut_is_refused_rather_than_read_as_a_negative_length() {
    let (_dir, root) = root_with(&["a.mkv"]);
    let real = root.join("a.mkv").canonicalize().expect("canonical");
    let edl = edl_of(vec![source_cut(real.to_str().expect("utf8"), 3.0, 1.0)]);
    let err = admit_edl(&root, &edl, &admitted_4s(&root)).expect_err("inverted");
    assert_eq!(
        err,
        MediaRejection::CutRange {
            cut: 0,
            error: TimeError::Inverted {
                start: 3_000_000,
                end: 1_000_000
            },
        }
    );
}

#[test]
fn a_zero_length_cut_is_refused_because_it_reads_no_frames() {
    let (_dir, root) = root_with(&["a.mkv"]);
    let real = root.join("a.mkv").canonicalize().expect("canonical");
    let edl = edl_of(vec![source_cut(real.to_str().expect("utf8"), 2.0, 2.0)]);
    let err = admit_edl(&root, &edl, &admitted_4s(&root)).expect_err("empty span");
    assert_eq!(
        err,
        MediaRejection::CutRange {
            cut: 0,
            error: TimeError::Empty { at: 2_000_000 }
        }
    );
}

#[test]
fn a_cut_naming_a_path_outside_the_root_is_refused_even_when_it_would_decode() {
    let (dir, root) = root_with(&["a.mkv"]);
    let outside = dir.path().join("passwd");
    std::fs::write(&outside, b"not media").expect("fixture");
    let edl = edl_of(vec![source_cut(outside.to_str().expect("utf8"), 0.0, 1.0)]);
    let err = admit_edl(&root, &edl, &HashMap::new()).expect_err("outside root");
    assert_eq!(
        err,
        MediaRejection::CutPath {
            cut: 0,
            source: PathRejection::OutsideRoot
        }
    );
    assert_eq!(err.code(), "media-input/outside-root");
}

#[test]
fn a_symlink_inside_the_root_that_points_outside_is_refused() {
    let (dir, root) = root_with(&["a.mkv"]);
    let outside = dir.path().join("elsewhere.mkv");
    std::fs::write(&outside, b"not media").expect("fixture");
    let link = root.join("innocent.mkv");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).expect("symlink");
    #[cfg(unix)]
    {
        let edl = edl_of(vec![source_cut(link.to_str().expect("utf8"), 0.0, 1.0)]);
        let err = admit_edl(&root, &edl, &HashMap::new()).expect_err("symlink escape");
        assert_eq!(
            err,
            MediaRejection::CutPath {
                cut: 0,
                source: PathRejection::OutsideRoot
            },
            "a symlink must be resolved before containment, or the root is decorative"
        );
    }
}

#[test]
fn a_cut_whose_source_was_never_admitted_is_refused_not_assumed_in_range() {
    let (_dir, root) = root_with(&["a.mkv", "b.mkv"]);
    let a = root.join("a.mkv").canonicalize().expect("canonical");
    let b = root.join("b.mkv").canonicalize().expect("canonical");
    let edl = edl_of(vec![
        source_cut(a.to_str().expect("utf8"), 0.0, 1.0),
        source_cut(b.to_str().expect("utf8"), 0.0, 1.0),
    ]);
    // Only `a.mkv` is in the map. Treating the missing entry as "in range"
    // would let any cut through whenever the map was built one source short.
    let err = admit_edl(&root, &edl, &admitted_4s(&root)).expect_err("unadmitted source");
    assert!(
        matches!(err, MediaRejection::Undecodable { .. }),
        "expected Undecodable, got {err:?}"
    );
}

#[test]
fn distinct_sources_are_listed_once_in_first_reference_order() {
    let (_dir, root) = root_with(&["a.mkv", "b.mkv"]);
    let a = root.join("a.mkv").canonicalize().expect("canonical");
    let b = root.join("b.mkv").canonicalize().expect("canonical");
    let mut map = admitted_4s(&root);
    map.insert(
        b.clone(),
        AdmittedMedia {
            path: b.clone(),
            bytes: 4,
            duration: MediaTime::from_micros(2_000_000).expect("positive"),
        },
    );
    let at = a.to_str().expect("utf8").to_string();
    let bt = b.to_str().expect("utf8").to_string();
    let edl = edl_of(vec![
        broll_cut(&bt, 0.0, 1.0),
        source_cut(&at, 0.0, 1.0),
        source_cut(&at, 1.0, 3.0),
    ]);
    let admitted = admit_edl(&root, &edl, &map).expect("admitted");
    assert_eq!(admitted.sources, vec![b, a]);
    // 1s of b-roll, then 1s and 2s of source: the concatenation is the output.
    assert_eq!(
        admitted.output,
        MediaTime::from_micros(4_000_000).expect("positive")
    );
}
