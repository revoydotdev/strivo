//! Build-time check: no Rust **code** under `src/creator` may name the
//! millisecond duration column that schema version 2 replaced.
//!
//! This used to be a test, and it was the wrong place for it. A test that
//! walks the package's own source tree reads a tree that other lanes in this
//! plan are writing at the same time, and a directory that exists when the
//! walk enumerates it can be gone by the time the walk reads it. That
//! surfaced as a reproducible red gate: under `cargo test --workspace`, which
//! runs every test binary in parallel, the walk hit `ENOENT` on a directory
//! another lane was mid-way through replacing, while `cargo test -p
//! strivo-research` — fewer binaries, a narrower window — passed every time.
//! `--test-threads=1` did not help, because the concurrency was between
//! binaries rather than within one.
//!
//! A build script does not have that problem. Cargo builds every test binary
//! **before it runs any of them**, so no test can be mutating the tree while
//! this walks it, and the window is a few milliseconds rather than minutes.
//! A build failure is also the stronger gate: it cannot be filtered out by
//! test name, and it runs identically in `cargo build`, `cargo test` and
//! `cargo clippy`.
//!
//! The check is deliberately not a fixed file list. A list would go stale the
//! moment someone adds a module, and a stale list passes — which is the
//! vacuous-pass shape this plan has hit repeatedly. This discovers the files,
//! so a new one is covered the moment it exists.
//!
//! `src/creator/tests/` is excluded, and the exclusion is emitted so a test
//! can assert it rather than trust it: those fixtures name `duration_ms` for
//! the *research kernel's* `sources` table, which is a different model on a
//! table this crate does not own, and which is recorded as an open finding.

use std::path::{Path, PathBuf};

/// The name schema version 2 replaced. A comment explaining the migration may
/// say it; a field or a statement may not use it.
const FORBIDDEN: &str = "duration_ms";

const MODULE_DIR: &str = "src/creator";
const EXCLUDED_DIR: &str = "tests";

fn main() {
    let root = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set for a build script"),
    );
    let module = root.join(MODULE_DIR);
    // Track the whole source tree, not just the module, and say so: once a
    // build script emits any `rerun-if-changed`, cargo stops using its
    // default of "rerun when anything in the package changes", so a narrower
    // path here would make this check silently go stale.
    println!("cargo:rerun-if-changed={}", root.join("src").display());
    println!("cargo:rerun-if-changed=build.rs");

    let mut scanned = Vec::new();
    let mut offenders = Vec::new();
    let mut read_errors = Vec::new();
    walk(
        &root,
        &module,
        &mut scanned,
        &mut offenders,
        &mut read_errors,
    );

    // A directory this build script cannot read is a build environment that
    // cannot be trusted, and it is reported by path rather than swallowed —
    // swallowing it would make the check pass because it scanned nothing.
    if !read_errors.is_empty() {
        fail(&format!(
            "the Creator module could not be read in full, so the \
             {FORBIDDEN} check did not run over all of it:\n  {}",
            read_errors.join("\n  ")
        ));
    }
    if !offenders.is_empty() {
        fail(&format!(
            "no Rust code under {MODULE_DIR} may name `{FORBIDDEN}`. Found it in:\n  {}\n\
             A comment explaining the migration is fine; a field or a statement \
             using the old name is a second representation of time, which is \
             what schema version 2 exists to remove. Convert the caller, do not \
             keep both columns.",
            offenders.join("\n  ")
        ));
    }

    scanned.sort();
    println!(
        "cargo:rustc-env=STRIVO_CREATOR_RS_SCANNED={}",
        scanned.join(",")
    );
    println!("cargo:rustc-env=STRIVO_CREATOR_RS_EXCLUDED={EXCLUDED_DIR}");
}

fn walk(
    root: &Path,
    dir: &Path,
    scanned: &mut Vec<String>,
    offenders: &mut Vec<String>,
    read_errors: &mut Vec<String>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            read_errors.push(format!("{}: {e}", dir.display()));
            return;
        }
    };
    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(e) => {
                read_errors.push(format!("{}: {e}", dir.display()));
                continue;
            }
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if path.is_dir() {
            if name == EXCLUDED_DIR {
                continue;
            }
            walk(root, &path, scanned, offenders, read_errors);
            continue;
        }
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                read_errors.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        // Reported relative to the crate root, so the message reads the same
        // on every machine and a test can name the files without knowing where
        // the checkout lives.
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        for (number, line) in text.lines().enumerate() {
            // A comment explaining the migration is documentation; code is not.
            let code = line.split_once("//").map_or(line, |(head, _)| head);
            if code.contains(FORBIDDEN) {
                let text = code.trim();
                offenders.push(format!("{relative}:{}: {text}", number + 1));
            }
        }
        scanned.push(relative);
    }
}

fn fail(message: &str) -> ! {
    panic!("\nbuild.rs: the millisecond-duration check failed.\n\n{message}\n");
}
