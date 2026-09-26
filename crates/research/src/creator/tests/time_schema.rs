//! Two halves of the anti-drift guarantee that no Creator timestamp is a
//! millisecond duration.
//!
//! **The enforcement is `build.rs`, not this file.** A test that walks the
//! package's own source tree reads a tree that other lanes in this plan are
//! writing at the same time, and that became a reproducible red gate: under
//! `cargo test --workspace`, which runs every test binary in parallel, the
//! walk hit `ENOENT` on a directory another lane was mid-way through
//! replacing, while `cargo test -p strivo-research` passed every time.
//! `--test-threads=1` did not help, because the concurrency was between
//! binaries rather than within one. Cargo builds every test binary before it
//! runs any of them, so a build script has no such window — and a build
//! failure is the stronger gate anyway: it cannot be filtered out by test
//! name, and it runs in `cargo build`, `cargo test` and `cargo clippy` alike.
//!
//! So the split is: `build.rs` fails the build when any Rust **code** under
//! `src/creator` names `duration_ms`, and the tests here cover the two
//! properties checkable without touching the filesystem — the migrated
//! schema's columns, and the fact that the build-time scan is live and
//! actually saw the module's files. The second is what stops the check
//! becoming a vacuous pass: a scan that quietly found nothing would leave an
//! empty inventory, and an empty inventory is a failure here.

use tempfile::TempDir;

use super::super::schema::{self, MIGRATIONS};
use super::time_shared::{at_version_one, seed_v1_source};

/// Every file `build.rs` reported scanning, from the `cargo:rustc-env` value
/// it set. Compile-time, so a build script that stops running is a **compile**
/// failure rather than a test that quietly passes.
fn scanned_creator_files() -> Vec<&'static str> {
    match env!("STRIVO_CREATOR_RS_SCANNED") {
        "" => Vec::new(),
        list => list.split(',').collect(),
    }
}

/// Given a version 1 database migrated to version 2
/// When its two duration columns are inspected
/// Then `duration_us` is present and `duration_ms` is gone, and the
/// microsecond column refuses a negative value in raw SQL just as
/// `MediaTime` does in Rust.
#[test]
fn the_migrated_schema_carries_no_millisecond_column() {
    let dir = TempDir::new().expect("tempdir");
    let mut conn = at_version_one(&dir.path().join("research.db"));
    seed_v1_source(&conn, "asset-a", "project-a", Some(1_000));
    schema::migrate(&mut conn, MIGRATIONS).expect("version 2 applies");

    for table in ["creator_source_assets", "creator_artifacts"] {
        let mut stmt = conn
            .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
            .expect("prepare");
        let columns: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .expect("query")
            .collect::<std::result::Result<_, _>>()
            .expect("rows");
        assert!(
            columns.contains(&"duration_us".to_string()),
            "{table} has the microsecond column, found {columns:?}"
        );
        assert!(
            !columns.contains(&"duration_ms".to_string()),
            "{table} has no millisecond column left, found {columns:?}"
        );
    }

    let negative = conn.execute(
        "UPDATE creator_source_assets SET duration_us=-1 WHERE id='asset-a'",
        [],
    );
    assert!(
        negative.is_err(),
        "a negative microsecond duration is refused by the schema, not stored"
    );
}

/// Given the build-time `duration_ms` check
/// When the inventory it reported is read
/// Then it is non-empty and names the module's files, so a scan that silently
/// stopped finding anything cannot pass as a clean result.
///
/// The names here are the ones that carry the time model. `entities.rs` holds
/// the two `Option<MediaTime>` durations, `store_sources.rs` and
/// `store_delivery.rs` the statements that read and write them, and `mod.rs`
/// documents the replacement — which is why a `duration_ms` in any of them is
/// the defect, and why the check has to have seen all of them.
#[test]
fn the_build_time_millisecond_check_saw_the_creator_module() {
    let scanned = scanned_creator_files();

    assert!(
        !scanned.is_empty(),
        "the build-time scan reported no files. An empty inventory means the \
         check passed because it looked at nothing, which is the vacuous-pass \
         shape this plan has hit repeatedly."
    );
    for required in [
        "src/creator/entities.rs",
        "src/creator/mod.rs",
        "src/creator/schema.rs",
        "src/creator/store_sources.rs",
        "src/creator/store_delivery.rs",
        "src/creator/store_time.rs",
    ] {
        assert!(
            scanned.contains(&required),
            "the build-time scan did not cover {required}, which is how a new \
             caller could name duration_ms unnoticed. Scanned: {scanned:?}"
        );
    }

    // The tests directory is excluded on purpose, and the exclusion is
    // asserted rather than trusted: its fixtures name `duration_ms` for the
    // research kernel's own `sources` table, a different model on a table
    // this crate does not own.
    assert_eq!(
        env!("STRIVO_CREATOR_RS_EXCLUDED"),
        "tests",
        "the documented exclusion must be the one the build script applies"
    );
    assert!(
        !scanned.iter().any(|p| p.contains("/tests/")),
        "and nothing under the excluded tests directory may appear in the \
         inventory, found {scanned:?}"
    );
}
