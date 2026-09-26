//! Local file and folder import: identity, deduplication and relinking.
//!
//! The load-bearing claim of this whole todo is one sentence from the plan —
//! **moving a source file on disk preserves identity, because a path is a
//! resolvable location and never an identity**. It is asserted here against a
//! real committed media fixture, not a synthetic byte string, so the hash that
//! carries the identity is a hash of bytes ffprobe has read.
//!
//! Ordering matters in this file. The first test is a plain successful import,
//! because every refusal assertion below it would also pass against an import
//! that refuses everything; an import that admits nothing satisfies "no
//! traversal was imported" for free. [`a_valid_import_registers_exactly_one_source`]
//! is the control that makes the refusals mean something.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::super::entities::*;
use super::super::ids::*;
use super::super::import::*;
use super::super::jobs::JobState;
use super::super::relink::*;
use super::super::store::Store;
use super::fixtures::project;

/// A real, committed, redistributable fixture from todo 32's corpus, with the
/// sha256 `fixtures/media/MANIFEST.json` records for it. The manifest is an
/// independent record of the same fact, so asserting against it checks the
/// fingerprint against something this code did not compute.
/// One fixed instant for every import in this file, so the rows a test
/// reads back are ordered by something other than the wall clock.
const AT: &str = "2026-09-26T00:00:00Z";

const LANDMARKS_SHA256: &str = "b52387ef06c23f9ff8fdb9bf3b0d3a87dccba6727f4a55b4c0e52ef19cffa300";
const SILENT_VIDEO_SHA256: &str =
    "b5f143c95f70567aea9c39fe5e2aaca875377927554a898b26e369fb3e238861";

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/media")
        .join(name)
}

/// A copy of a corpus fixture at `dir/name`, parents created. Creating them
/// here rather than at each call site keeps a nested name like
/// `recovered/gone.mkv` from being a different kind of test than a flat one.
fn stage(dir: &Path, name: &str, fixture: &str) -> PathBuf {
    let path = dir.join(name);
    fs::create_dir_all(path.parent().expect("the temp dir")).expect("parent made");
    fs::copy(corpus(fixture), &path).expect("fixture copied");
    path
}

fn admitted(path: &Path) -> AdmittedFile {
    AdmittedFile::admitted(path)
}

fn request<'a>(
    project: ProjectId,
    files: &'a [AdmittedFile],
    scratch: &'a Path,
) -> ImportRequest<'a> {
    ImportRequest {
        project,
        files,
        scratch_root: scratch,
        at: AT,
    }
}

/// The project, the scratch root and a temp dir, which every import needs.
fn stage_project() -> (Store, Project, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    let store = Store::open_in_memory().expect("store opens");
    let p = project("creator import");
    store.insert_project(&p).expect("project inserted");
    (store, p, dir)
}

fn imported_id(outcome: &ImportOutcome) -> SourceAssetId {
    match outcome {
        ImportOutcome::Imported { asset } | ImportOutcome::Duplicate { asset } => asset.id,
    }
}

fn sources_by_name(store: &Store, project: ProjectId) -> HashMap<String, SourceAsset> {
    store
        .sources_of(project)
        .expect("sources listed")
        .into_iter()
        .map(|s| (s.title.clone(), s))
        .collect()
}

// ── the control: a real import succeeds ────────────────────────────────────

/// Given a real media fixture and a project a capture populates
/// When it is imported
/// Then exactly one source is registered, it resolves, and its content
/// identity is the sha256 the corpus manifest independently records.
#[test]
fn a_valid_import_registers_exactly_one_source() {
    let (store, p, dir) = stage_project();
    let path = stage(dir.path(), "stream.mkv", "landmarks.mkv");
    let files = [admitted(&path)];

    let manifest =
        import_files(&store, &request(p.id, &files, dir.path())).expect("import succeeds");

    assert_eq!(manifest.entries.len(), 1, "one input, one entry");
    assert_eq!(manifest.imported(), 1, "and it became a new source");
    assert_eq!(manifest.duplicates(), 0, "with nothing deduplicated");
    let asset = store
        .source_asset(imported_id(&manifest.entries[0].outcome))
        .expect("lookup ok")
        .expect("the source is registered");
    assert_eq!(
        asset.project_id, p.id,
        "it lands in the creator's own project"
    );
    assert_eq!(asset.kind, "import", "an import is its own kind of source");
    assert_eq!(asset.origin, "import", "and says where it came from");
    assert_eq!(
        asset.title, "stream.mkv",
        "the user sees the file's own name"
    );
    assert_eq!(
        asset.media_fingerprint,
        format!("sha256:{LANDMARKS_SHA256}"),
        "content identity is the bytes, checked against the corpus manifest"
    );
    assert_eq!(asset.availability(), Availability::Resolved(path));
    assert_eq!(
        store.sources_of(p.id).expect("sources listed").len(),
        1,
        "a valid import creates a source; every refusal assertion below is \
         meaningless without this one"
    );
    // A capture populates the same table with the same shape, so the two are
    // one library rather than two.
    let captured = SourceAsset {
        id: SourceAssetId::generate(),
        project_id: p.id,
        kind: "recording".into(),
        title: "captured.mkv".into(),
        media_fingerprint: "sha256:captured".into(),
        locator: Some(dir.path().join("captured.mkv")),
        origin: "capture".into(),
        rights_json: "{}".into(),
        track_type: "video".into(),
        codec: "h264".into(),
        duration: None,
        created_at: AT.to_string(),
    };
    store
        .insert_source_asset(&captured)
        .expect("capture inserted");
    let together = store.sources_of(p.id).expect("sources listed");
    assert_eq!(
        together.len(),
        2,
        "an imported file and a captured stream are the same kind of row in the \
         same collection"
    );
}

// ── the load-bearing claim: identity survives a move ───────────────────────

/// Given an imported source that a transcript and an edit both name
/// When the file is moved on disk and relinked
/// Then the asset id, the fingerprint and every link to it are unchanged, the
/// new location resolves, and the render-affecting input identity is byte for
/// byte the same key it was before the move.
#[test]
fn moving_the_file_preserves_identity() {
    let (store, p, dir) = stage_project();
    let scratch = dir.path().join("scratch");
    fs::create_dir_all(&scratch).expect("scratch made");
    let before = stage(dir.path(), "before.mkv", "landmarks.mkv");
    let files = [admitted(&before)];
    let manifest = import_files(&store, &request(p.id, &files, &scratch)).expect("import succeeds");
    let asset_id = imported_id(&manifest.entries[0].outcome);
    let key_before = manifest.cache_key.clone();

    // The two links a moved file must not break: a transcript revision and a
    // composition revision, both naming the asset by identity.
    let transcript = TranscriptRevision {
        id: TranscriptRevisionId::generate(),
        project_id: p.id,
        source_asset_id: asset_id,
        revision: 1,
        provider: "whisper".into(),
        model_version: "large-v3".into(),
        author: "machine".into(),
        body_json: "{}".into(),
        created_at: AT.to_string(),
    };
    store
        .insert_transcript_revision(&transcript)
        .expect("transcript inserted");
    let revision = CompositionRevision {
        id: CompositionRevisionId::generate(),
        project_id: p.id,
        parent_revision_id: None,
        revision: 1,
        author: "operator".into(),
        timeline_json: format!("{{\"source\":\"{asset_id}\"}}"),
        output_profile: "long-form".into(),
        created_at: AT.to_string(),
    };
    store
        .insert_composition_revision(&revision)
        .expect("revision inserted");

    // The move. The old path goes away entirely, so a locator that had
    // survived as the key would now name nothing.
    let moved_dir = dir.path().join("archive").join("2026");
    fs::create_dir_all(&moved_dir).expect("archive made");
    let after = moved_dir.join("after.mkv");
    fs::rename(&before, &after).expect("file moved");

    let relinked = relink(&store, asset_id, &admitted(&after)).expect("relink succeeds");

    assert_eq!(relinked.id, asset_id, "the id did not change");
    assert_eq!(
        relinked.media_fingerprint,
        format!("sha256:{LANDMARKS_SHA256}"),
        "content identity is not the path"
    );
    assert_eq!(
        relinked.locator,
        Some(after.clone()),
        "the new location is recorded"
    );
    assert_eq!(
        relinked.availability(),
        Availability::Resolved(after.clone())
    );
    assert_eq!(
        relinked.created_at,
        manifest.entries[0].outcome.outcome_asset().created_at,
        "the asset was not re-created: it is the same row from the same instant"
    );
    assert_eq!(
        store
            .transcript_revisions_of_source(asset_id)
            .expect("links listed")[0]
            .id,
        transcript.id,
        "the transcript still names the same asset"
    );
    assert!(
        store
            .composition_revisions_of(p.id)
            .expect("revisions listed")[0]
            .timeline_json
            .contains(&asset_id.to_string()),
        "the edit's source span still names the same asset"
    );
    assert_eq!(
        store.sources_of(p.id).expect("sources listed").len(),
        1,
        "a move is not a second source"
    );

    // The render-affecting consequence, stated as the thing it actually is.
    // `CacheKey` is computed by the production function over the *identities* it
    // resolved, so importing the same bytes from the new location must produce
    // the same key: that key is what `reusable` reads to decide an output built
    // from these inputs does not have to be rebuilt. Had the key been computed
    // over locators, this would differ — and the re-import would also report a
    // second `Imported` instead of a `Duplicate`, which is the same defect
    // showing up in a second place.
    let after_files = [admitted(&after)];
    let second = import_files(&store, &request(p.id, &after_files, &scratch))
        .expect("re-importing from the new location succeeds");
    assert_eq!(
        key_before, second.cache_key,
        "the inputs a render depends on are unchanged by a move"
    );
    assert_eq!(
        second.imported(),
        0,
        "so the moved file is its own duplicate, not a second source"
    );
    assert_eq!(
        second.entries[0].outcome.outcome_asset().id,
        asset_id,
        "and it matched the same identity"
    );
}

// ── deduplication: proven, and proven not to over-collapse ─────────────────

/// Given a project already holding one file's content
/// When a second copy of the same bytes is imported under a different name
/// Then the project still holds one source, the existing row is untouched, and
/// the entry says which source it matched and why.
#[test]
fn a_reimport_of_the_same_content_is_deduped_and_explained() {
    let (store, p, dir) = stage_project();
    let first = stage(dir.path(), "session-01.mkv", "landmarks.mkv");
    let files = [admitted(&first)];
    let one = import_files(&store, &request(p.id, &files, dir.path())).expect("first import");
    let original_id = imported_id(&one.entries[0].outcome);

    // A genuine second copy: different path, different name, identical bytes.
    let copy = stage(
        dir.path(),
        "backup of session-01 (copy).mkv",
        "landmarks.mkv",
    );
    assert_ne!(copy, first, "the duplicate really is at another path");
    let files = [admitted(&first), admitted(&copy)];
    let two = import_files(&store, &request(p.id, &files, dir.path())).expect("second import");

    assert_eq!(
        store.sources_of(p.id).expect("sources listed").len(),
        1,
        "the same content imported twice is one source"
    );
    assert!(
        matches!(two.entries[0].outcome, ImportOutcome::Duplicate { .. }),
        "the first input is reported as a duplicate of the source already there"
    );
    assert!(
        matches!(two.entries[1].outcome, ImportOutcome::Duplicate { .. }),
        "and so is the copy"
    );
    assert_eq!(
        two.entries[1].outcome.outcome_asset().id,
        original_id,
        "the explanation names the source that already holds the content"
    );
    assert_eq!(
        two.entries[1].fingerprint,
        format!("sha256:{LANDMARKS_SHA256}"),
        "and says what matched, in content terms"
    );
    assert_eq!(
        two.entries[1].name, "backup of session-01 (copy).mkv",
        "and which input it was, by the name the user recognises"
    );
    assert_eq!(two.imported(), 0, "no input became a second source");
    assert_eq!(two.duplicates(), 2, "both were reported as already present");
    let stored = store
        .source_asset(original_id)
        .expect("lookup ok")
        .expect("still there");
    assert_eq!(
        stored.locator,
        Some(first),
        "a duplicate does not repoint the existing source at the new copy"
    );
    assert_eq!(stored.title, "session-01.mkv", "nor rename it");
}

/// The other half of "dedup works": a dedup that collapses *distinct* files is
/// worse than no dedup, because the second one is silently lost. Two different
/// corpus fixtures differ in content, and both must survive.
#[test]
fn a_genuinely_different_file_is_not_deduped() {
    let (store, p, dir) = stage_project();
    let a = stage(dir.path(), "a.mkv", "landmarks.mkv");
    let b = stage(dir.path(), "b.mkv", "silent-video.mkv");
    let files = [admitted(&a), admitted(&b)];
    assert_ne!(
        fs::read(&a).expect("read").len(),
        fs::read(&b).expect("read").len(),
        "the two fixtures differ in size as well as in content"
    );

    let manifest =
        import_files(&store, &request(p.id, &files, dir.path())).expect("import succeeds");

    assert_eq!(
        store.sources_of(p.id).expect("sources listed").len(),
        2,
        "two different files are two sources; a dedup that collapsed them would \
         have destroyed one"
    );
    assert!(
        manifest
            .entries
            .iter()
            .all(|e| matches!(e.outcome, ImportOutcome::Imported { .. })),
        "neither is reported as a duplicate"
    );
    assert_ne!(
        imported_id(&manifest.entries[0].outcome),
        imported_id(&manifest.entries[1].outcome),
        "and they are two identities, not one"
    );
    assert_eq!(
        manifest.entries[1].fingerprint,
        format!("sha256:{SILENT_VIDEO_SHA256}"),
        "each carries its own content identity"
    );
    let by_name = sources_by_name(&store, p.id);
    assert_eq!(by_name.len(), 2, "both are addressable by the name shown");
}

// ── relinking ─────────────────────────────────────────────────────────────

/// Given an imported source whose file has been deleted
/// When it is read, then relinked to a restored copy at a new path
/// Then it reports `Unresolved` rather than dangling, and relinking restores
/// the same identity at the new location.
#[test]
fn a_deleted_file_is_reported_unresolved_and_can_be_relinked() {
    let (store, p, dir) = stage_project();
    let path = stage(dir.path(), "gone.mkv", "landmarks.mkv");
    let files = [admitted(&path)];
    let manifest =
        import_files(&store, &request(p.id, &files, dir.path())).expect("import succeeds");
    let id = imported_id(&manifest.entries[0].outcome);

    fs::remove_file(&path).expect("file deleted");
    let orphaned = store
        .source_asset(id)
        .expect("lookup ok")
        .expect("the row survives the file");
    assert_eq!(
        orphaned.availability(),
        Availability::Unresolved,
        "a missing file is reported missing, not assumed present"
    );
    assert_eq!(
        unresolved_sources(&store, p.id)
            .expect("unresolved listed")
            .iter()
            .map(|s| s.id)
            .collect::<Vec<_>>(),
        vec![id],
        "and the row is still there to be relinked, named by the query a \
         relink surface is built from"
    );

    let restored = stage(dir.path(), "recovered/gone.mkv", "landmarks.mkv");
    let relinked = relink(&store, id, &admitted(&restored)).expect("relink succeeds");
    assert_eq!(relinked.id, id, "relinking is not re-importing");
    assert_eq!(relinked.availability(), Availability::Resolved(restored));
    assert_eq!(
        unresolved_sources(&store, p.id)
            .expect("unresolved listed")
            .len(),
        0,
        "nothing is missing any more, so a relink surface has nothing to show"
    );
}

/// Given a source registered from one file's content
/// When it is relinked to a file with different content
/// Then the relink is refused with its own code and the identity is untouched.
#[test]
fn relink_refuses_content_that_is_not_the_source() {
    let (store, p, dir) = stage_project();
    let path = stage(dir.path(), "real.mkv", "landmarks.mkv");
    let files = [admitted(&path)];
    let manifest =
        import_files(&store, &request(p.id, &files, dir.path())).expect("import succeeds");
    let id = imported_id(&manifest.entries[0].outcome);

    let impostor = stage(dir.path(), "other.mkv", "silent-video.mkv");
    let refusal =
        relink(&store, id, &admitted(&impostor)).expect_err("a different file is refused");

    assert_eq!(refusal.code(), "import/content-mismatch");
    let after = store
        .source_asset(id)
        .expect("lookup ok")
        .expect("still there");
    assert_eq!(
        after.locator,
        Some(path),
        "a refused relink changed nothing"
    );
    assert_eq!(
        after.media_fingerprint,
        format!("sha256:{LANDMARKS_SHA256}"),
        "and did not adopt the impostor's identity"
    );
}

/// Given an id that names no source
/// When it is relinked
/// Then the refusal is its own class rather than a no-op that looks like a
/// success.
#[test]
fn relinking_an_id_that_names_no_source_is_refused() {
    let (store, _p, dir) = stage_project();
    let path = stage(dir.path(), "x.mkv", "landmarks.mkv");
    let missing = SourceAssetId::generate();
    let refusal = relink(&store, missing, &admitted(&path)).expect_err("no such source");
    assert_eq!(refusal.code(), "import/no-such-source");
}

// ── a filename is untrusted data ───────────────────────────────────────────

/// Given a file whose name carries markup and shell metacharacters
/// When it is imported
/// Then the name round-trips verbatim as data, and nothing interpreted it: the
/// stored title is the file's own bytes and no file was created by the name.
#[test]
fn a_filename_is_carried_as_data_and_never_interpreted() {
    let (store, p, dir) = stage_project();
    // No `/`: a name is one path component, and a separator is a different
    // thing entirely. What reaches a UI unescaped from a legal file name is
    // markup, quotes, `;`, `|`, `&`, `$()` and backticks — so those are what is
    // here.
    let hostile =
        "<img src=x onerror=alert(1)>; rm -rf ~ $(id) `whoami` && echo 'pwned' | tee .mkv";
    let path = stage(dir.path(), hostile, "silent-video.mkv");
    let files = [admitted(&path)];

    let manifest =
        import_files(&store, &request(p.id, &files, dir.path())).expect("import succeeds");

    assert_eq!(manifest.entries[0].name, hostile, "the name is data");
    let asset = store
        .source_asset(imported_id(&manifest.entries[0].outcome))
        .expect("lookup ok")
        .expect("registered");
    assert_eq!(
        asset.title, hostile,
        "stored verbatim: no escaping is a UI's job and no shell ever sees it"
    );
    assert_eq!(
        store.sources_of(p.id).expect("sources listed").len(),
        1,
        "one hostile name is still one file"
    );
    // Nothing was created by the name: the only entry under the temp dir is
    // the file the test itself copied in.
    let created: Vec<_> = fs::read_dir(dir.path())
        .expect("readable")
        .filter_map(std::result::Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        created,
        vec![hostile.to_string()],
        "the name created nothing"
    );
}

// ── the durable lifecycle ──────────────────────────────────────────────────

/// Given an import
/// When it finishes
/// Then it has run through todo 24's lifecycle — submitted, running, done —
/// with every state change in the append-only log, and the manifest is the
/// artifact the job points at.
#[test]
fn an_import_runs_through_the_durable_job_lifecycle() {
    let (store, p, dir) = stage_project();
    let scratch = dir.path().join("scratch");
    fs::create_dir_all(&scratch).expect("scratch made");
    let path = stage(dir.path(), "stream.mkv", "landmarks.mkv");
    let files = [admitted(&path)];

    let manifest = import_files(&store, &request(p.id, &files, &scratch)).expect("import succeeds");

    let job = store
        .job(manifest.job)
        .expect("lookup ok")
        .expect("the job is there");
    assert_eq!(job.state, JobState::Done, "a finished import is done");
    assert_eq!(
        job.subject_kind, "import",
        "and says what kind of job it was"
    );
    assert_eq!(job.project_id, p.id, "in the project it imported into");
    assert_eq!(
        job.cache_key, manifest.cache_key,
        "the cache key is the manifest's"
    );
    assert_eq!(job.attempt, 0, "nothing failed, so no attempt was consumed");
    assert_eq!(
        job.artifact_id,
        Some(manifest.artifact),
        "the job points at its manifest"
    );

    let log = store.transitions(manifest.job).expect("log listed");
    let states: Vec<Option<JobState>> = log.iter().map(|t| t.from).collect();
    assert_eq!(
        states,
        vec![None, Some(JobState::Pending), Some(JobState::Running)],
        "the log is complete from submission, so no state is observable that \
         the history does not record"
    );
    assert_eq!(log.last().expect("a final row").to, JobState::Done);
    assert_eq!(
        log.last().expect("a final row").reason,
        "complete",
        "and the final row says which operation finished it"
    );

    let artifact = store
        .artifact(manifest.artifact)
        .expect("lookup ok")
        .expect("the manifest artifact is there");
    assert_eq!(
        artifact.kind, "import",
        "it is an import manifest, not a render output"
    );
    assert_eq!(artifact.project_id, p.id);
    assert_eq!(
        artifact.content_hash,
        manifest.cache_key.as_str(),
        "a manifest is identified by the content hash of its inputs"
    );
    assert_eq!(artifact.locator, None, "and by nothing on disk");
}

/// Given a folder of files
/// When it is imported
/// Then every admitted file is registered under its own name, and the
/// fingerprint reads a real file end to end — the value is a hash of bytes,
/// not of a size and an mtime, which is what todo 24 replaced.
#[test]
fn a_whole_folder_imports_under_its_own_names() {
    let (store, p, dir) = stage_project();
    let folder = dir.path().join("session");
    fs::create_dir_all(&folder).expect("folder made");
    let one = stage(&folder, "cam.mkv", "landmarks.mkv");
    let two = stage(&folder, "mic.mkv", "silent-video.mkv");
    let files = [admitted(&one), admitted(&two)];

    let manifest =
        import_files(&store, &request(p.id, &files, dir.path())).expect("import succeeds");

    assert_eq!(manifest.entries.len(), 2, "both files are in the manifest");
    let by_name = sources_by_name(&store, p.id);
    assert!(by_name.contains_key("cam.mkv"), "cam.mkv is addressable");
    assert!(by_name.contains_key("mic.mkv"), "mic.mkv is addressable");
    assert_eq!(
        by_name["cam.mkv"].media_fingerprint,
        format!("sha256:{LANDMARKS_SHA256}"),
        "each carries the content identity of its own bytes"
    );
    assert_eq!(
        manifest.entries.len(),
        2,
        "one entry per input, none collapsed"
    );
}

/// Given a fingerprint request
/// When it is asked for a path that is not there
/// Then it is an error rather than the hash of nothing — a silent zero-byte
/// fingerprint would make every missing file look like one real file.
#[test]
fn a_fingerprint_of_a_missing_file_is_an_error() {
    let dir = TempDir::new().expect("tempdir");
    let missing = dir.path().join("nope.mkv");
    assert!(
        fingerprint(&missing).is_err(),
        "a file that is not there has no content identity"
    );
}

/// Given an import that has already completed
/// When it is run again over the same inputs
/// Then it is idempotent: the same source set, the same job cache key, and a
/// second job that deduplicates rather than importing the bytes twice. This is
/// the property that makes a re-queued import (`recover_jobs` after a crash)
/// safe to run again.
#[test]
fn re_running_a_completed_import_is_idempotent() {
    let (store, p, dir) = stage_project();
    let a = stage(dir.path(), "a.mkv", "landmarks.mkv");
    let b = stage(dir.path(), "b.mkv", "silent-video.mkv");
    let files = [admitted(&a), admitted(&b)];

    let first = import_files(&store, &request(p.id, &files, dir.path())).expect("first import");
    let after_first: Vec<SourceAssetId> = store
        .sources_of(p.id)
        .expect("sources listed")
        .iter()
        .map(|s| s.id)
        .collect();

    let second = import_files(&store, &request(p.id, &files, dir.path())).expect("second import");

    let after_second: Vec<SourceAssetId> = store
        .sources_of(p.id)
        .expect("sources listed")
        .iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(
        after_first, after_second,
        "the same two identities, in the same order"
    );
    assert_eq!(second.imported(), 0, "the second run created nothing");
    assert_eq!(
        second.duplicates(),
        2,
        "it recognised both as already present"
    );
    assert_eq!(
        first.cache_key, second.cache_key,
        "and it is the same operation: the inputs' identities are unchanged"
    );
    assert_ne!(
        first.job, second.job,
        "each run is still its own durable job"
    );
}

/// Given an import naming a project that does not exist
/// When it is attempted
/// Then it is refused by name, with nothing registered and no job left behind.
/// Found by booting a Creator binary: without this the refusal was a
/// `FOREIGN KEY constraint failed` from the first insert, which named neither
/// the caller's mistake nor the project and still left a submitted job.
#[test]
fn an_import_into_a_project_that_does_not_exist_is_refused_by_name() {
    let store = Store::open_in_memory().expect("store opens");
    let dir = TempDir::new().expect("tempdir");
    let path = stage(dir.path(), "stream.mkv", "landmarks.mkv");
    let files = [admitted(&path)];
    let ghost = ProjectId::generate();

    let refusal = import_files(&store, &request(ghost, &files, dir.path()))
        .expect_err("a project that does not exist refuses the import");

    assert!(
        refusal.to_string().contains(&ghost.to_string()),
        "the refusal names the project it could not find: {refusal}"
    );
    assert_eq!(
        store.sources_of(ghost).expect("sources listed").len(),
        0,
        "nothing was registered"
    );
    assert!(
        store.jobs(ghost).expect("jobs listed").is_empty(),
        "and no job was submitted for an import that imported nothing"
    );
}
