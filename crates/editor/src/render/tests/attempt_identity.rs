//! What one attempt's identity derives, and what it refuses.
//!
//! The path arithmetic and the artifact gate, tested without a media tool: two
//! attempts of one subject get two directories, one attempt's cleanup touches
//! nothing but its own, a caller-supplied subject cannot place a file outside
//! the output directory, an interrupted render still tidies up, and a tool that
//! reports success over no file is a failure.
//!
//! The behavioural half — two real renders at once, a refused overwrite, a
//! mid-render failure, two successive artifacts — is `tests::isolation`, because
//! those need ffmpeg and a decoded artifact to say anything true.

use super::attempt;
use super::fixture::scratch;
use super::*;

/// The path derivation, asserted rather than inferred.
///
/// The behavioural test in `isolation` can only observe a collision that
/// actually happens to interleave. This one states the property directly: two
/// attempts of one subject get two scratch directories, in one process, with
/// nothing scheduled. It is the assertion that stays true when the machine is
/// idle.
#[test]
fn two_attempts_of_one_subject_get_two_scratch_directories() {
    let parent = std::path::Path::new("/tmp");
    let first = AttemptId::mint();
    let second = AttemptId::mint();
    assert_ne!(
        first, second,
        "two attempts of one recording must not share an identity"
    );
    let a = attempt::scratch_dir(parent, "stream-01", &first);
    let b = attempt::scratch_dir(parent, "stream-01", &second);
    assert_ne!(
        a, b,
        "two concurrent renders of one subject shared the scratch directory {a:?}"
    );
    assert_eq!(
        a.parent(),
        Some(parent),
        "scratch must stay under the output parent, or a subject can write outside it"
    );
}

/// A subject is caller-supplied, so it is untrusted input at a path boundary.
///
/// Both derived paths embed it. A subject carrying a separator or a parent
/// reference must not be able to place a scratch directory or an artifact
/// outside the output directory the caller chose.
#[test]
fn a_hostile_subject_cannot_escape_the_output_directory() {
    let root = scratch("escape");
    for subject in ["../../etc", "a/b", "..", "/absolute", "x\0y"] {
        for path in [
            attempt::scratch_dir(&root, subject, &AttemptId::mint()),
            attempt::output_path(&root, subject, &AttemptId::mint(), "mkv"),
        ] {
            assert!(
                path.starts_with(&root) && path.parent() == Some(root.as_path()),
                "subject {subject:?} placed a file at {path:?}, outside {root:?}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// The scratch directory a second render cannot delete is the whole claim, so
/// the drop that removes it must only ever remove its own.
///
/// Asserted by making the *other* attempt's directory and checking it is still
/// there after this one is dropped. A shared directory fails this on the first
/// run, with no timing involved.
#[test]
fn one_attempts_cleanup_leaves_another_attempts_scratch_alone() {
    let parent = std::path::Path::new("/tmp");
    let mine = AttemptId::mint();
    let theirs = AttemptId::mint();
    let mine_dir = attempt::scratch_dir(parent, "stream-01", &mine);
    let theirs_dir = attempt::scratch_dir(parent, "stream-01", &theirs);
    std::fs::create_dir_all(&mine_dir).expect("scratch for this attempt");
    std::fs::create_dir_all(&theirs_dir).expect("scratch for the other attempt");
    std::fs::write(theirs_dir.join("clip_000.mkv"), b"in flight").expect("seed");

    let guard = attempt::Scratch(mine_dir.clone());
    drop(guard);

    assert!(
        !mine_dir.exists(),
        "an attempt must remove its own scratch directory"
    );
    assert!(
        theirs_dir.join("clip_000.mkv").exists(),
        "one attempt's cleanup removed {} while another render was using it",
        theirs_dir.display()
    );
    let _ = std::fs::remove_dir_all(&theirs_dir);
}

/// An interrupted render still tidies up.
///
/// "Cancelled" is todo 41/65's to wire, but the half this todo owns is that the
/// scratch directory is removed on the way out of a render that never returns
/// normally — an early `?`, a `return`, and a panic. `catch_unwind` is what
/// makes the third one observable here.
#[test]
fn an_interrupted_render_still_removes_its_scratch() {
    let root = scratch("interrupted");
    let dir = attempt::scratch_dir(&root, "stream-01", &AttemptId::mint());
    std::fs::create_dir_all(&dir).expect("scratch");
    std::fs::write(dir.join("clip_000.mkv"), b"half written").expect("partial sub-render");

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _scratch = attempt::Scratch(dir.clone());
        std::fs::write(dir.join("clip_001.mkv"), b"interrupted").expect("write");
        panic!("the render was cancelled mid-flight");
    }));
    assert!(outcome.is_err(), "the panic must have propagated");
    assert!(
        !dir.exists(),
        "an interrupted render left {} behind, including its half-written \
         sub-renders",
        dir.display()
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A tool that exits zero without writing a file has not rendered anything.
///
/// ffmpeg on this toolchain was measured not to produce a zero-byte Matroska
/// even when asked for zero frames and zero duration, so this state is not
/// reachable through a working renderer — which is the point. It is exactly
/// what a substituted or stubbed tool leaves behind, and "the tool said OK" is
/// not evidence that an artifact exists.
#[test]
fn a_zero_byte_artifact_is_a_failure_not_a_success() {
    let dir = scratch("empty-artifact");
    let empty = dir.join("nothing.mkv");
    std::fs::write(&empty, b"").expect("a zero-byte file at the artifact path");

    let refusal = attempt::validate_output(&empty)
        .expect_err("a zero-byte output must not be reported as a rendered artifact");
    assert!(
        matches!(&refusal, RenderError::EmptyOutput { path } if path == &empty),
        "expected the empty-output refusal, got {refusal:?}"
    );
    assert!(
        empty.exists(),
        "reporting the failure must not delete the evidence of it"
    );

    let missing = dir.join("absent.mkv");
    assert!(
        matches!(
            attempt::validate_output(&missing),
            Err(RenderError::MissingOutput { .. })
        ),
        "a tool that wrote nothing at all must be reported as a failure"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
