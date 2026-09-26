//! The SPA module sequence guard rejects what a silent reorder looks like.
//!
//! `build.rs` calls `assert_sequence_ok` on both module directories before it
//! copies anything, so each case below is the build failing for real — proven
//! against the real tree by `cargo build` and by the last test here, and
//! against synthetic directories by the rest, because mutating the real tree
//! to watch a build fail is not something a test may do.
//!
//! The non-vacuity point matters more than the case list: a guard over an
//! empty subject, or one whose "duplicate" branch is unreachable because the
//! filename grammar already makes duplicates impossible, passes everything
//! and catches nothing. The duplicate case is therefore a *reachable* shape —
//! an unpadded `12-pvr.js` beside `012-pvr.js`, which is one slot with two
//! filenames and two different sort positions.

#[path = "../build/sequence.rs"]
mod sequence;

use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

/// Run the guard over a scratch directory holding `names`, and return the
/// message it panicked with — empty when it accepted them.
fn verdict(names: &[&str]) -> String {
    let dir = tempfile::tempdir().expect("scratch module directory");
    for name in names {
        fs::write(dir.path().join(name), "").expect("scratch module file");
    }
    match catch_unwind(AssertUnwindSafe(|| {
        sequence::assert_sequence_ok(dir.path())
    })) {
        Ok(()) => String::new(),
        Err(payload) => payload
            .downcast_ref::<String>()
            .cloned()
            .expect("the guard fails by panicking with a message, not by returning"),
    }
}

fn real_module_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name)
}

#[test]
fn a_gap_is_rejected_and_names_the_missing_number() {
    // When 001 is absent, 002 still assembles — and now runs first, silently.
    let message = verdict(&["000-pvr.js", "002-pvr.js", "003-pvr.js"]);
    assert!(
        message.contains("gap: no module at sequence 1 (0..=3 expected"),
        "{message}"
    );
}

#[test]
fn a_long_gap_is_reported_as_a_range_not_a_number_dump() {
    // `020` deleted leaves 021..=040 with no module; listing all 20 numbers
    // buries the edit, and the `0..=40` beside it is what names the culprit.
    let message = verdict(&["000-pvr.js", "001-pvr.js", "021-pvr.js"]);
    assert!(
        message.contains("gap: no module at sequence 2-20 (0..=21 expected"),
        "{message}"
    );
}

#[test]
fn a_duplicate_slot_is_rejected_and_names_both_files() {
    // 12-pvr.js sorts after 019-creator.js, so this is a reorder, not a no-op.
    let message = verdict(&["012-pvr.js", "12-pvr.js", "013-creator.js"]);
    assert!(message.contains("duplicate"), "{message}");
    assert!(message.contains("012-pvr.js"), "{message}");
    assert!(message.contains("12-pvr.js"), "{message}");
}

#[test]
fn a_module_the_assembler_would_drop_is_rejected() {
    // `.JS` never matches the assembler's extension filter and `.bak` is not a
    // module at all; either way the module silently stops shipping.
    for name in [
        "001-pvr.JS",
        "001-pvr.js.bak",
        "spa.js",
        "notes.md",
        "pvr.js",
    ] {
        let message = verdict(&["000-pvr.js", name]);
        assert!(message.contains("malformed"), "{name}: {message}");
        assert!(message.contains(name), "{name}: {message}");
    }
}

#[test]
fn a_pvr_module_may_share_its_number_with_a_creator_sibling() {
    // The 1-byte spacers reserve a slot for a Creator sibling that does not
    // exist yet; a PVR-only build must not read that as a duplicate.
    assert_eq!(verdict(&["000-pvr.js", "001-pvr.js", "001-creator.js"]), "");
}

#[test]
fn letter_suffixed_sub_slots_are_distinct_from_their_number() {
    // `001-pvr.js` and `001a-pvr.js` are one sequence number in two slots, not
    // a duplicate. The real tree's `010`/`012`/`012a`/`012h` shape — whose sort
    // order the object-literal seam 002->008 and the render() switch
    // 008->010->011->012 depend on — is covered by the last test here.
    assert_eq!(
        verdict(&["000-pvr.js", "001-pvr.js", "001a-pvr.js", "001h-pvr.js"]),
        ""
    );
}

#[test]
fn a_gap_free_sequence_is_accepted() {
    assert_eq!(verdict(&["000-pvr.js", "001-creator.js", "002-pvr.js"]), "");
}

#[test]
fn an_empty_module_directory_is_rejected_rather_than_passing() {
    // A guard over nothing satisfies every assertion it could make.
    assert!(verdict(&[]).contains("no module files"));
}

#[test]
fn the_real_module_directories_are_gap_free_and_duplicate_free() {
    sequence::assert_sequence_ok(&real_module_dir("spa"));
    sequence::assert_sequence_ok(&real_module_dir("spa-css"));
}
