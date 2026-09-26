//! Local file and folder import: content identity, deduplication, manifests and
//! relinking. Report item C01
//! (`docs/CREATOR-EDITION-REPORT-2026-09-07.md:280`).
//!
//! ## The one rule this module exists to keep
//!
//! **A path is a resolvable location, never an identity.** Import is where that
//! rule is under the most pressure, because the user hands it paths and the
//! obvious implementation — key the library on the path — is wrong three ways
//! at once: a moved file becomes a second entry, a re-import under a new name is
//! invisible, and a restored file becomes a different asset from the one whose
//! transcript and edits name it. So every decision here keys on **content**
//! ([`fingerprint`]) and the filesystem appears only as a
//! [`SourceAsset::locator`], which [`Store::set_source_locator`] rewrites
//! without touching an identity. The machinery for that already exists —
//! `SourceAsset::availability` and `set_source_locator` from todo 21 — and this
//! module uses it rather than rebuilding it.
//!
//! ## What a manifest is, and what it is not
//!
//! **A manifest is the record of one import operation**: which inputs it was
//! asked to bring in, and what became of each. It is the [`Artifact`] of kind
//! `import` that the import's job completes with, identified by the
//! [`CacheKey`] over the imported inputs' identities — so a manifest with the
//! same inputs is the same manifest, and a re-import is recognisable rather than
//! repeated.
//!
//! Relinking — pointing a source at where its bytes moved to — is
//! [`super::relink`], a sibling rather than a branch here: it decides nothing
//! about whether a source should exist.
//!
//! It is **not** a copy of the media and **not** a copy of the source rows. A
//! manifest carrying the file list, the titles or the durations would be a
//! second source of truth for facts the source rows already hold, and the first
//! writer to update one and not the other would leave the library quietly
//! disagreeing with itself. What the manifest adds, and the source rows cannot,
//! is the per-input outcome — in particular **why** an input did not become a
//! new source, which is "duplicates are explained, not silently dropped" and
//! has no other home.
//!
//! ## Admission is not re-implemented here
//!
//! Every path reaches this module as an [`AdmittedFile`], and the only way to
//! make one is to hand it a path that has already passed the media-input gate
//! (`media_input::admit_path` in the web crate — todo 25). Import is a second
//! way in, and a second way in that skipped the gate could install a source the
//! gate would later refuse: a `file://` URL, a `..` hop out of the chosen root, a
//! directory, a fifo that would block a reader forever. So this module adds no
//! path grammar of its own. `strivo-research` sits below `strivo-web` in the
//! dependency graph and cannot call the gate, which is why the admitted file is a
//! type rather than a `PathBuf`; the parity between the two is proven by a test
//! in the web crate that drives the real admission path and the import surface
//! against the same hostile inputs and asserts the same typed codes.
//!
//! ## Deduplication is content-based and scoped to the project
//!
//! Two files with the same bytes are the same asset; two files with different
//! bytes are two assets, always. The lookup is against the sources of the
//! project being imported into, so importing the same clip into two projects
//! gives each its own source — a shared asset row would couple two projects'
//! relinking and deletion to each other, which is the "silently redefine the
//! project as one recording" the report forbids.
//!
//! ## No probe, no watcher, no new dependency
//!
//! Import runs no `ffprobe`: media facts, readiness and track identification are
//! todo 37's and todo 34's, and duplicating them here would be a second answer
//! to "what is in this file". It does not watch the chosen folder — a missing
//! file is reported `Unresolved` and relinked on request, which is a state a
//! reader can see rather than a background process that has to be trusted. The
//! only digest is the `sha2` this crate already depends on, read as a stream so
//! a multi-gigabyte file is hashed without being loaded.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::entities::{Artifact, SourceAsset};
use super::ids::{ArtifactId, JobId, ProjectId, SourceAssetId};
use super::jobs::{CacheKey, JobInputs, JobState, MAX_ATTEMPTS};
use super::store::Store;
use super::store_jobs::{MoveJob, NewJob};
use crate::Result;

/// The subject an import job records. Constant on purpose: an import's identity
/// is the *set of inputs it carried*, which is what [`CacheKey`] hashes, so a
/// per-import random value here would give every import a different key and make
/// both the cache and duplicate recognition unreachable.
const IMPORT_SUBJECT: &str = "local-media";

/// A path that has already passed the media-input admission gate.
///
/// Deliberately not a `PathBuf`, so the only way to reach the import surface is
/// through a caller that ran the gate, and the precondition is visible at the
/// signature instead of living in a comment. The constructor is public because
/// the gate lives in another crate; what makes it safe is not the constructor
/// but the test in `strivo-web` that drives the real admission path and this
/// surface against the same hostile inputs and asserts the same typed codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedFile {
    locator: PathBuf,
}

impl AdmittedFile {
    /// Wrap a path a gate has already admitted.
    #[must_use]
    pub fn admitted(locator: impl Into<PathBuf>) -> Self {
        Self {
            locator: locator.into(),
        }
    }

    /// Where the bytes are. A location, not an identity.
    #[must_use]
    pub fn locator(&self) -> &Path {
        &self.locator
    }
}

/// What became of one input the import was given.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportOutcome {
    /// A new source was registered for it.
    Imported {
        /// The source that now holds this content.
        asset: SourceAsset,
    },
    /// The project already held a source with this exact content, and that
    /// source was left exactly as it was. Carrying it is the explanation: the
    /// caller is told *which* source matched, not merely that something did.
    Duplicate {
        /// The pre-existing source holding the same content.
        asset: SourceAsset,
    },
}

impl ImportOutcome {
    /// The source this input resolved to, whether it was created or found.
    #[must_use]
    pub fn outcome_asset(&self) -> &SourceAsset {
        match self {
            Self::Imported { asset } | Self::Duplicate { asset } => asset,
        }
    }
}

/// One input, and what became of it.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportEntry {
    /// The file's own name, exactly as it is on disk. Untrusted text bound for
    /// a UI: carried verbatim, never escaped here (escaping is the renderer's
    /// job) and never handed to a shell.
    pub name: String,
    /// The content identity the input resolved to, whatever the outcome. On a
    /// duplicate this is what matched.
    pub fingerprint: String,
    /// Created, or already present.
    pub outcome: ImportOutcome,
}

/// The record of one import operation.
///
/// See the module doc for what this is and is not: it names the inputs and
/// their outcomes, and it is the artifact the import's job completed with. It
/// holds no copy of the media and no copy of the source rows.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportManifest {
    /// The durable job the import ran as, so the operation has a lifecycle, a
    /// retry budget and a history rather than being a side effect of a request.
    pub job: JobId,
    /// The manifest artifact: `kind == "import"`, identified by
    /// [`Self::cache_key`].
    pub artifact: ArtifactId,
    /// Over the identities of the inputs, never their locators. Equal for two
    /// imports of the same content from any location, which is what makes a
    /// duplicate recognisable as the same operation rather than a new one.
    pub cache_key: CacheKey,
    /// One entry per input, in the order they were given.
    pub entries: Vec<ImportEntry>,
}

impl ImportManifest {
    /// How many inputs became a new source. The count an import is judged on.
    #[must_use]
    pub fn imported(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e.outcome, ImportOutcome::Imported { .. }))
            .count()
    }

    /// How many were already there, each naming the source it matched.
    #[must_use]
    pub fn duplicates(&self) -> usize {
        self.entries.len() - self.imported()
    }
}

/// One import, as the caller states it. Grouped because these are not
/// independent parameters: the fingerprint is taken over the files and the job
/// is submitted for the project, so a builder that could be handed one without
/// the other is a builder that can disagree with itself.
#[derive(Debug, Clone, Copy)]
pub struct ImportRequest<'a> {
    /// The project the files join — the same source collection a capture
    /// writes to, which is what makes an imported file and a captured stream
    /// one library rather than two.
    pub project: ProjectId,
    /// Already-admitted inputs. There is no `PathBuf` entry point and no
    /// string-parsing one either.
    pub files: &'a [AdmittedFile],
    /// The import job's scratch root. Nothing is written into it — an import
    /// produces no output bytes — but the job owns one, as every job does.
    pub scratch_root: &'a Path,
    /// When this import happened, as the caller's clock states it. No clock is
    /// read here, so the same request twice produces the same rows.
    pub at: &'a str,
}

/// The content identity of a file: `sha256:<hex>` over its whole contents.
///
/// Not over its size and mtime, which todo 24 removed from the cache key
/// precisely because a rewrite can preserve both. Read as a stream: import is
/// the surface most likely to be handed a multi-gigabyte file, and `fs::read`
/// would put all of it in memory at once.
///
/// A file that cannot be read is an error, not the digest of nothing. A
/// zero-byte fingerprint would be a real value that every missing file collides
/// on, which is worse than having no fingerprint at all.
pub fn fingerprint(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buf = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        digest.update(&buf[..read]);
    }
    let hex = digest
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut hex, b| {
            hex.push_str(&format!("{b:02x}"));
            hex
        });
    Ok(format!("sha256:{hex}"))
}

/// Register a project's existing sources by content, so a duplicate is one
/// lookup rather than a scan of the collection per input.
fn existing_by_content(store: &Store, project: ProjectId) -> Result<HashMap<String, SourceAsset>> {
    Ok(store
        .sources_of(project)?
        .into_iter()
        .map(|s| (s.media_fingerprint.clone(), s))
        .collect())
}

/// Import every admitted file into `request.project`.
///
/// **Every input is read and hashed before a single row is written.** That
/// ordering is what makes the refusals worth anything: a file that cannot be
/// read fails the whole import with nothing registered, so there is no state in
/// which the collection holds some of what was asked for. It is also why this
/// function has no cancellation point — there is nothing to cancel between the
/// decision and the write.
///
/// If the *database* fails part-way through the writes the job is left
/// `running`, which is what the durable lifecycle is for: `recover_jobs`
/// re-queues it on the next open, and re-running the import deduplicates to the
/// same identities rather than importing the same bytes twice.
///
/// Deduplication is by content, scoped to the project. A match is reported, not
/// dropped: the entry names the source that already holds the bytes, and that
/// source is left untouched — not repointed at the new copy, not renamed.
pub fn import_files(store: &Store, request: &ImportRequest<'_>) -> Result<ImportManifest> {
    // A project id that names no project is refused here, before any row is
    // written, rather than surfacing as a foreign-key failure from the first
    // source insert. Found by booting a Creator binary: the import route
    // answered `500 database error: FOREIGN KEY constraint failed`, which names
    // neither the caller's mistake nor the project, and left a submitted job
    // behind for a request that had imported nothing.
    if store.project(request.project)?.is_none() {
        return Err(crate::ResearchError::Validation(format!(
            "no such creator project: {}",
            request.project
        )));
    }
    let mut known = existing_by_content(store, request.project)?;

    // Phase one: decide everything. No writes, so nothing can be half-done.
    let mut planned = Vec::with_capacity(request.files.len());
    for file in request.files {
        planned.push((
            file.locator().to_path_buf(),
            file_name_of(file.locator()),
            fingerprint(file.locator())?,
        ));
    }

    let mut entries = Vec::with_capacity(planned.len());
    let mut ids = Vec::with_capacity(planned.len());
    for (locator, name, digest) in planned {
        let outcome = match known.get(&digest) {
            Some(existing) => ImportOutcome::Duplicate {
                asset: existing.clone(),
            },
            None => {
                let asset = SourceAsset {
                    id: SourceAssetId::generate(),
                    project_id: request.project,
                    // An import is its own kind of source, so it is
                    // distinguishable from a capture at a glance.
                    kind: "import".into(),
                    title: name.clone(),
                    media_fingerprint: digest.clone(),
                    locator: Some(locator),
                    origin: "import".into(),
                    // Rights and consent are todo 38's columns. An empty object
                    // says "not yet recorded", not "public domain".
                    rights_json: "{}".into(),
                    // Media facts — track type, codec, duration — are read by
                    // todo 37's readiness pass, not guessed from an extension
                    // here. Two answers to "what is in this file" is the drift
                    // that pass exists to remove.
                    track_type: String::new(),
                    codec: String::new(),
                    duration: None,
                    created_at: request.at.to_string(),
                };
                known.insert(digest.clone(), asset.clone());
                ImportOutcome::Imported { asset }
            }
        };
        ids.push(outcome.outcome_asset().id);
        entries.push(ImportEntry {
            name,
            fingerprint: digest,
            outcome,
        });
    }

    let cache_key = CacheKey::of(&JobInputs {
        subject_kind: "import",
        subject_id: IMPORT_SUBJECT,
        assets: &ids,
        transcript_revisions: &[],
        composition_revision: None,
        settings: "",
        tool_versions: &[],
    });

    // Phase two: run it as a durable job, so the operation has a lifecycle and
    // a history rather than being a side effect of a request. Submitted before
    // any source is written, so a process that dies mid-import leaves a
    // `running` job that says so.
    let job = store.submit_job(&NewJob {
        id: JobId::generate(),
        project_id: request.project,
        subject_kind: "import".into(),
        subject_id: IMPORT_SUBJECT.into(),
        cache_key: cache_key.clone(),
        scratch_root: request.scratch_root.to_path_buf(),
        max_attempts: MAX_ATTEMPTS,
        at: request.at.to_string(),
    })?;
    store.move_job(&MoveJob {
        job: job.id,
        to: JobState::Running,
        reason: "run",
        error: None,
        at: request.at,
    })?;

    for entry in &entries {
        if let ImportOutcome::Imported { asset } = &entry.outcome {
            store.insert_source_asset(asset)?;
        }
    }

    let manifest = Artifact {
        id: ArtifactId::generate(),
        project_id: request.project,
        variant_id: None,
        render_plan_id: None,
        kind: "import".into(),
        // The manifest is identified by the hash of its inputs and has no bytes
        // of its own, so it has no locator and no size. It is never
        // `validated_at`: there is nothing to validate, and a manifest claiming
        // its bytes had been checked would assert a check that never happened.
        content_hash: cache_key.as_str().to_string(),
        locator: None,
        bytes: None,
        width: None,
        height: None,
        duration: None,
        created_at: request.at.to_string(),
    };
    store.insert_artifact(&manifest)?;
    store.complete(job.id, manifest.id, request.at)?;

    Ok(ImportManifest {
        job: job.id,
        artifact: manifest.id,
        cache_key,
        entries,
    })
}

/// A file's own name, or the whole path when it has none. Never empty: a source
/// with a blank title is not addressable in a library.
fn file_name_of(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}
