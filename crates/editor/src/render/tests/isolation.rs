//! One render attempt must not be able to damage another's files.
//!
//! `docs/CREATOR-EDITION-REPORT-2026-09-07.md:88` says the report **did not
//! reproduce** the scratch collision, so the claim under test here was inferred
//! rather than observed. These tests are the observation: the first drives two
//! real renders of one revision into one output directory at the same time and
//! asserts what survives on disk.
//!
//! Every assertion reads the **filesystem**, not a return value. An export that
//! reports `bytes: 0` and leaves nothing behind has already lost, and a test
//! that read the return value would call that a pass.
//!
//! The path arithmetic these depend on is `tests::attempt_identity`; what is
//! here needs ffmpeg and a real artifact.

use std::sync::{Arc, Barrier};

use super::*;
use crate::{CutKind, Edl};

/// Two renders of one recording, launched together, into one output parent.
///
/// The reported race: the executor named its scratch directory after the
/// *subject*, so two attempts at the same subject are the same directory, and
/// `Scratch::drop` removes it whoever gets there last. One render's per-cut
/// sub-clips therefore overwrite the other's mid-flight and the first to finish
/// deletes the second's working directory out from under it. That reproduced on
/// five runs out of five against the shipped code.
///
/// The barrier matters. Without it the threads serialise by accident often
/// enough to hide the collision, and a race that reproduces one time in five is
/// not a race that was fixed.
#[test]
fn two_concurrent_exports_of_one_revision_both_survive() {
    let Some((req, revision, dir)) = on_wide_fixture("concurrent") else {
        return;
    };
    // Same parent, so a shared scratch path is a real possibility rather than
    // two directories that were never going to meet.
    let out_a = dir.join("a.mkv");
    let out_b = dir.join("b.mkv");
    assert_eq!(
        out_a.parent(),
        out_b.parent(),
        "the two outputs must share a parent or this test proves nothing"
    );

    let gate = Arc::new(Barrier::new(2));
    let render = |req: RenderRequest, edl: Edl, out: std::path::PathBuf| {
        let gate = Arc::clone(&gate);
        std::thread::spawn(move || {
            gate.wait();
            let result = export(&req, &edl, &out, None);
            (out, result)
        })
    };
    let first = render(req.clone(), revision.clone(), out_a);
    let second = render(req, revision, out_b);
    let (path_a, result_a) = first.join().expect("first render thread");
    let (path_b, result_b) = second.join().expect("second render thread");

    let report = |label: &str, path: &std::path::Path, result: &Result<Export, RenderError>| {
        format!("{label}: {path:?} -> {result:?}")
    };
    assert!(result_a.is_ok(), "{}", report("first", &path_a, &result_a));
    assert!(result_b.is_ok(), "{}", report("second", &path_b, &result_b));

    for (label, path, result) in [("a", &path_a, &result_a), ("b", &path_b, &result_b)] {
        let done = result.as_ref().expect("checked above");
        let on_disk = std::fs::metadata(path).unwrap_or_else(|e| {
            panic!("render {label} reported success but left no artifact: {e}");
        });
        assert_eq!(
            on_disk.len(),
            done.bytes,
            "render {label} reported {} bytes and left {} on disk",
            done.bytes,
            on_disk.len()
        );
        assert!(
            on_disk.len() > 0,
            "render {label} left a zero-byte artifact at {path:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A render aimed at a path that already holds an artifact must be refused, not
/// allowed to overwrite it.
///
/// This is the enforcement of "a new render writes a new artifact id". A caller
/// that hands the executor a stable per-recording filename — which is exactly
/// what the render route did — gets a typed failure instead of a silently
/// destroyed artifact.
#[test]
fn a_render_will_not_overwrite_an_existing_artifact() {
    let Some((req, revision, dir)) = on_fixture("immutable") else {
        return;
    };
    let out = dir.join("out.mkv");
    let first = export(&req, &revision, &out, None).expect("the first render");
    let approved = std::fs::read(&out).expect("the first artifact is on disk");

    let second = export(&req, &revision, &out, None);
    assert!(
        matches!(&second, Err(RenderError::OutputExists { path }) if path == &out),
        "a second render at the same path must be refused, got {second:?}"
    );
    let afterwards = std::fs::read(&out).expect("the prior artifact is still readable");
    assert!(
        same_bytes(&approved, &afterwards),
        "the refused render changed the prior artifact's bytes: {} before, {} after",
        digest(&approved),
        digest(&afterwards)
    );
    assert_eq!(
        std::fs::metadata(&out).expect("stat").len(),
        first.bytes,
        "the prior artifact's size changed under a refused render"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A failure **after** the scratch directory exists and after real work landed
/// in it must leave nothing behind and change nothing that was already there.
///
/// Both halves, because either alone is satisfied for free. A render that
/// refuses its input up front never creates a scratch directory, so "no scratch
/// left" alone also passes for a render that never started.
///
/// The **control** is what rules that out: the first `export` in this test is
/// the same request and the same revision with both cuts pointing at the real
/// fixture, and it renders. The only field changed between the control and the
/// failing render is cut 1's source path, and `execute` walks its clips in order
/// returning on the first failure — so a control that succeeds and a variant
/// that fails on cut 1 places the failure in the second ffmpeg pass, after the
/// scratch directory exists and after cut 0 has been written into it.
#[test]
fn a_mid_render_failure_leaves_no_scratch_and_an_untouched_prior_artifact() {
    let Some((req, mut revision, dir)) = on_fixture("midfail") else {
        return;
    };
    assert!(
        revision.cuts.len() > 1,
        "this test needs a later cut to break; the fixture has {}",
        revision.cuts.len()
    );
    let out = dir.join("out.mkv");
    let approved = export(&req, &revision, &out, None).expect("the control render");
    let approved_bytes = std::fs::read(&out).expect("the approved artifact");

    // Cut 0 still points at the real fixture; cut 1 does not exist.
    revision.cuts[1].kind = CutKind::Source {
        source_path: dir
            .join("not-a-recording.mkv")
            .to_string_lossy()
            .into_owned(),
    };
    let second = dir.join("second.mkv");
    let failure = export(&req, &revision, &second, None)
        .expect_err("a cut that names a missing source must fail the render");
    assert!(
        matches!(&failure, RenderError::Execute { .. }),
        "expected the executor's own refusal, not a compile refusal, got {failure:?}"
    );

    let leftovers: Vec<String> = std::fs::read_dir(&dir)
        .expect("output parent")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".plan-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "a mid-render failure left scratch behind: {leftovers:?}"
    );
    assert!(
        !second.exists(),
        "a failed render left a partial artifact at {second:?}"
    );
    let afterwards = std::fs::read(&out).expect("the approved artifact survives");
    assert!(
        same_bytes(&approved_bytes, &afterwards),
        "the failed render modified the previously approved artifact: {} before, \
         {} after",
        digest(&approved_bytes),
        digest(&afterwards)
    );
    assert_eq!(
        std::fs::metadata(&out).expect("stat").len(),
        approved.bytes,
        "the failed render resized the previously approved artifact"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two successive renders of the same revision are two addressable artifacts.
///
/// The immutable path derivation, end to end: a second render must not land on
/// the first one's bytes, must not refuse for colliding, and must produce its
/// own artifact — so a user who re-renders an unchanged edit can tell the two
/// apart and the first is still there.
#[test]
fn two_successive_renders_produce_two_addressable_artifacts() {
    let Some((req, revision, dir)) = on_fixture("two-artifacts") else {
        return;
    };
    let first = attempt::output_path(&dir, "stream-01", &AttemptId::mint(), "mkv");
    let second = attempt::output_path(&dir, "stream-01", &AttemptId::mint(), "mkv");
    assert_ne!(
        first, second,
        "two renders of one revision addressed the same artifact"
    );

    let one = export(&req, &revision, &first, None).expect("the first render");
    let after_first = std::fs::read(&first).expect("read the first artifact");
    let two = export(&req, &revision, &second, None).expect("the second render");
    assert_eq!(two.output, second);
    assert_ne!(
        two.attempt, one.attempt,
        "one export produced two attempt ids"
    );
    for (label, path, done) in [("first", &first, &one), ("second", &second, &two)] {
        let on_disk = std::fs::metadata(path)
            .unwrap_or_else(|e| panic!("the {label} artifact is not on disk: {e}"));
        assert_eq!(
            on_disk.len(),
            done.bytes,
            "the {label} reported size is wrong"
        );
        assert!(on_disk.len() > 0, "the {label} artifact is zero bytes");
    }
    // The bytes of the first artifact, not the contents of the second: Matroska
    // writes a per-render segment UID, so two renders of one revision are not
    // byte-identical and asserting that would be asserting a container detail
    // instead of the claim. What must hold is that the second render left the
    // first alone — which is what "a new render writes a new artifact id" means
    // for a user who already approved the first.
    let afterwards = std::fs::read(&first).expect("the first artifact is still readable");
    assert!(
        same_bytes(&after_first, &afterwards),
        "the second render modified the first render's artifact: {} bytes/{} \
         before, {} bytes/{} after",
        after_first.len(),
        digest(&after_first),
        afterwards.len(),
        digest(&afterwards)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A fingerprint that prints short. `assert_eq!` on two `Vec<u8>` dumps every
/// byte, and a four-kilobyte diff of an opaque container tells a reader nothing.
fn digest(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

fn same_bytes(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && digest(left) == digest(right)
}
