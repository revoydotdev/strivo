//! What the migrated schema looks like, and the anti-drift check that keeps
//! the millisecond representation from coming back.

use tempfile::TempDir;

use super::super::schema::{self, MIGRATIONS};
use super::time_shared::{at_version_one, seed_v1_source};

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

/// The files under `dir` whose **code** names `duration_ms`, with doc and
/// line comments stripped. A comment explaining the migration is documentation;
/// a field or a statement using the old name is a second representation.
fn code_naming_ms(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir_path) = stack.pop() {
        for entry in std::fs::read_dir(&dir_path).expect("the module directory is readable") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                // The tests directory holds the research kernel's own
                // fixtures, whose `duration_ms` is a different model on a
                // table this todo does not own. It is reported as a finding
                // in the evidence, not silently folded into this check.
                if path.file_name().is_some_and(|n| n == "tests") {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if !path.extension().is_some_and(|e| e == "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("the file is readable");
            let code = text
                .lines()
                .map(|l| l.split_once("//").map_or(l, |(head, _)| head))
                .collect::<Vec<_>>()
                .join("\n");
            if code.contains("duration_ms") {
                out.push(
                    path.file_name()
                        .expect("a named file")
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    out.sort();
    out
}
#[test]
fn the_creator_module_names_no_millisecond_duration() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/creator");

    let mut naming_ms = Vec::new();
    let mut rust_naming_ms = Vec::new();
    let mut stack = vec![dir.clone()];
    while let Some(entry_path) = stack.pop() {
        for entry in std::fs::read_dir(&entry_path).expect("the module directory is readable") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("the file is readable");
            if !text.contains("duration_ms") {
                continue;
            }
            let name = path
                .file_name()
                .expect("a named file")
                .to_string_lossy()
                .into_owned();
            naming_ms.push(name.clone());
            if path.extension().is_some_and(|e| e == "rs") {
                rust_naming_ms.push(name);
            }
        }
    }

    // Prose may explain the migration, code may not use the field, so a
    // doc comment is stripped before the Rust check.
    assert!(
        code_naming_ms(&dir).is_empty(),
        "no Rust code under src/creator may name duration_ms, found {:?}",
        code_naming_ms(&dir)
    );
    assert!(
        !rust_naming_ms.is_empty(),
        "and the scan is not vacuous: prose in this module does name it, so \
         the strip step is what makes the code check meaningful"
    );
    let mut sql_naming_ms: Vec<String> = naming_ms
        .iter()
        .filter(|n| n.ends_with(".sql"))
        .cloned()
        .collect();
    sql_naming_ms.sort();
    assert_eq!(
        sql_naming_ms,
        vec![
            "schema_v1.sql".to_string(),
            "schema_v2.down.sql".to_string(),
            "schema_v2.sql".to_string(),
        ],
        "only the migrations that create or convert the old column may name it"
    );

    // Version 1's own files are committed history, and version 1 is what a
    // database already at version 1 holds. Its `down` drops whole tables, so
    // it never named the column in the first place — which is how this check
    // can tell "version 1 was left alone" from "version 1 was rewritten to
    // match the new column".
    let v1_down = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/creator/schema_v1.down.sql"),
    )
    .expect("version 1's down is readable");
    assert!(
        !v1_down.contains("duration_ms"),
        "version 1's down must not have been rewritten to mention the new \
         column's predecessor"
    );
}
