//! Backup census and restore reconciliation: the record of *what a backup
//! contained*, and the check of *what came back*.
//!
//! Report item O01's first clause is "restore projects and their source links
//! from a backup", and its `Must NOT` is "restore report success for a project
//! whose sources did not come back". Both need one thing the previous backup
//! surface did not have: **a record of what was in the backup**, taken when the
//! backup was made. Restoring a database file and calling it a success is a
//! claim about bytes on disk, not about a project — a backup that was taken
//! before a project existed restores cleanly and restores nothing.
//!
//! ## The census is taken from the identity, not from the paths
//!
//! A [`Census`] names every project and every source **by id and content
//! fingerprint** — todo 33's rule, that a path is a locator and content is the
//! identity. So a restore onto a box where every file has moved still reconciles
//! correctly: each source is found by id, and the fingerprint proves the row
//! that came back is the same recording rather than a different one that
//! happened to be written under the same id.
//!
//! ## Four outcomes, and only one of them is success
//!
//! | outcome | meaning |
//! |---|---|
//! | [`SourceOutcome::Restored`] | the row came back **and** its bytes resolve |
//! | [`SourceOutcome::Unresolved`] | the row came back with the right fingerprint, but nothing is at its locator |
//! | [`SourceOutcome::MissingRow`] | no row with that id — the link is gone |
//! | [`SourceOutcome::IdentityMismatch`] | a row is there under that id with **different** content |
//!
//! [`RestoreReport::complete`] is true only when the census was supplied and
//! every source in it is [`SourceOutcome::Restored`]. A caller that wants a
//! weaker bar reads [`RestoreReport::projects`] itself; a caller that reads only
//! `complete` cannot be told a lie. Note that `Unresolved` blocks completeness:
//! restoring onto a new box restores the *rows* and not the *recordings*, and
//! reporting that as a clean success is the exact failure this module exists to
//! prevent. The remedy is todo 33's `relink`, which this report says out loud
//! rather than leaving the operator to discover.
//!
//! ## A backup with no census is unverified, not empty
//!
//! Backups taken before this version carry no census. [`reconcile`] with no
//! census returns a report with `verified == false` and `complete == false`, and
//! no project entries — because "there is nothing wrong" and "we cannot tell"
//! must not produce the same answer, and the previous surface's
//! `{"restored": ["jobs.db"]}` is precisely that conflation.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::entities::Availability;
use super::ids::{ProjectId, SourceAssetId};
use super::store::Store;
use crate::{ResearchError, Result};

/// The name a backup's census is written and read under, inside the backup
/// directory. One name, so a backup is self-describing: a directory holding
/// this file is a backup that can be checked.
pub const CENSUS_FILE: &str = "manifest.json";

/// One source as the backup saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCensus {
    pub id: SourceAssetId,
    pub title: String,
    /// The content identity at backup time. A restored row carrying a different
    /// fingerprint is a different recording, which is
    /// [`SourceOutcome::IdentityMismatch`] rather than a success.
    pub fingerprint: String,
}

/// One project as the backup saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectCensus {
    pub id: ProjectId,
    pub name: String,
    pub sources: Vec<SourceCensus>,
}

/// What a backup contained, in identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Census {
    /// When the census was taken, as the caller's clock stated it. No clock is
    /// read here, so the same store twice gives the same census.
    pub taken_at: String,
    pub projects: Vec<ProjectCensus>,
}

impl Census {
    /// Every source the backup recorded, across projects.
    #[must_use]
    pub fn source_count(&self) -> usize {
        self.projects.iter().map(|p| p.sources.len()).sum()
    }

    /// The record of what `store` holds right now.
    ///
    /// Reads the source rows through [`SourceAsset::availability`] and stores
    /// **identity only** — id, title, fingerprint. The locator is deliberately
    /// absent: a backup that recorded where the bytes were would reconcile a
    /// restore against paths, which is the mistake todo 33 exists to prevent,
    /// and on any other machine every path in it is wrong.
    pub fn snapshot(store: &Store, taken_at: &str) -> Result<Self> {
        let mut projects = Vec::new();
        for project in store.projects()? {
            let mut sources = Vec::new();
            for source in store.sources_of(project.id)? {
                sources.push(SourceCensus {
                    id: source.id,
                    title: source.title.clone(),
                    fingerprint: source.media_fingerprint.clone(),
                });
            }
            projects.push(ProjectCensus {
                id: project.id,
                name: project.name,
                sources,
            });
        }
        Ok(Self {
            taken_at: taken_at.to_string(),
            projects,
        })
    }

    /// Write the census into a backup directory, parent directories created.
    ///
    /// Written through a temporary file and renamed, so a census that was
    /// interrupted is an absent census — which reconciles as `unverified` —
    /// rather than a truncated JSON file that makes the whole restore fail to
    /// parse.
    pub fn write(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        let body = serde_json::to_vec_pretty(self)
            .map_err(|e| ResearchError::Validation(format!("census does not serialise: {e}")))?;
        let final_path = dir.join(CENSUS_FILE);
        let staging = dir.join(format!("{CENSUS_FILE}.partial"));
        std::fs::write(&staging, &body)?;
        std::fs::rename(&staging, &final_path)?;
        Ok(())
    }

    /// Read a census out of a backup directory, or `None` when the backup
    /// carries none.
    ///
    /// A file that exists but does not parse is an **error**, not a `None`: a
    /// corrupt census is a backup that cannot be checked, and reporting it as
    /// "no census" would let an incomplete restore be described as unverified
    /// rather than as broken.
    pub fn read(dir: &Path) -> Result<Option<Self>> {
        let path = dir.join(CENSUS_FILE);
        if !path.is_file() {
            return Ok(None);
        }
        let body = std::fs::read(&path)?;
        let census: Self = serde_json::from_slice(&body)
            .map_err(|e| ResearchError::Validation(format!("{CENSUS_FILE} does not parse: {e}")))?;
        Ok(Some(census))
    }
}

/// What became of one source the backup recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceOutcome {
    /// The row came back with the recorded content identity and its bytes
    /// resolve. The only outcome that is a success.
    Restored,
    /// The row came back with the recorded content identity, but nothing is at
    /// its locator. The link is intact; the bytes are not here. Todo 33's
    /// `relink` is the remedy.
    Unresolved,
    /// No row with that id. The source link did not come back at all.
    MissingRow,
    /// A row is there under that id, carrying **different** content. A restore
    /// that produced this did not restore the recording; reporting it as
    /// `Restored` would be a false success with the worst possible cause.
    IdentityMismatch,
}

impl SourceOutcome {
    /// Whether this source came back whole.
    #[must_use]
    pub fn is_restored(self) -> bool {
        matches!(self, Self::Restored)
    }
}

/// One source's fate, named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRestore {
    pub id: SourceAssetId,
    /// The title the backup recorded, so a report names the file a person
    /// recognises rather than only a uuid.
    pub title: String,
    pub fingerprint: String,
    pub outcome: SourceOutcome,
    /// Where the bytes are now, when they are. Present for a caller that wants
    /// to say where to look; never a substitute for a resolved outcome.
    #[serde(default)]
    pub locator: Option<String>,
}

/// One project's fate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRestore {
    pub id: ProjectId,
    pub name: String,
    /// How many sources the backup recorded. The denominator every rate is read
    /// against, so a report cannot quietly compare a smaller number to itself.
    pub expected: usize,
    /// How many came back whole.
    pub restored: usize,
    /// Whether the project row itself came back. A project that did not is
    /// reported with every one of its sources as missing, so the report names
    /// what is gone rather than only that a project is.
    pub present: bool,
    pub sources: Vec<SourceRestore>,
}

impl ProjectRestore {
    /// The sources that did not come back whole, in census order.
    #[must_use]
    pub fn missing(&self) -> Vec<&SourceRestore> {
        self.sources
            .iter()
            .filter(|s| !s.outcome.is_restored())
            .collect()
    }
}

/// The whole answer to "did the restore work".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreReport {
    /// Whether a census was available to check against. `false` means **nothing
    /// about projects was verified**, which is not the same as "nothing is
    /// wrong" and must never be rendered as a success.
    pub verified: bool,
    /// Every recorded source came back whole. The single field a caller should
    /// gate its success message on.
    pub complete: bool,
    pub projects: Vec<ProjectRestore>,
}

impl RestoreReport {
    /// A report for a backup that carried no census: nothing verified, nothing
    /// claimed.
    #[must_use]
    pub fn unverified() -> Self {
        Self {
            verified: false,
            complete: false,
            projects: Vec::new(),
        }
    }

    /// Every source that did not come back whole, across projects — the lines a
    /// refusal has to name.
    #[must_use]
    pub fn missing(&self) -> Vec<(String, &SourceRestore)> {
        self.projects
            .iter()
            .flat_map(|p| p.missing().into_iter().map(move |s| (p.name.clone(), s)))
            .collect()
    }

    /// How many sources the census recorded, across projects.
    #[must_use]
    pub fn expected_sources(&self) -> usize {
        self.projects.iter().map(|p| p.expected).sum()
    }
}

/// Check a restored store against the census the backup recorded.
///
/// A project the census names and the store does not have is reported as absent
/// with **every one of its sources listed as [`SourceOutcome::MissingRow`]** —
/// naming the recordings that are gone is the whole point of the report, and a
/// bare "project missing" would leave the operator to enumerate them by hand.
///
/// A source the store has but the census did not record is not reported: it
/// came from somewhere else, and this check answers one question — did what the
/// backup promised come back.
pub fn reconcile(census: &Census, store: &Store) -> Result<RestoreReport> {
    let mut projects = Vec::with_capacity(census.projects.len());
    let mut complete = true;
    for recorded in &census.projects {
        let present = store.project(recorded.id)?.is_some();
        if !present {
            complete = false;
        }
        let mut sources = Vec::with_capacity(recorded.sources.len());
        let mut restored = 0;
        for want in &recorded.sources {
            let row = if present {
                store.source_asset(want.id)?
            } else {
                None
            };
            let (outcome, locator) = match &row {
                None => (SourceOutcome::MissingRow, None),
                Some(asset) if asset.media_fingerprint != want.fingerprint => {
                    (SourceOutcome::IdentityMismatch, None)
                }
                Some(asset) => match asset.availability() {
                    Availability::Resolved(path) => {
                        (SourceOutcome::Restored, Some(path.display().to_string()))
                    }
                    Availability::Unresolved => (SourceOutcome::Unresolved, None),
                },
            };
            if outcome.is_restored() {
                restored += 1;
            } else {
                complete = false;
            }
            sources.push(SourceRestore {
                id: want.id,
                title: want.title.clone(),
                fingerprint: want.fingerprint.clone(),
                outcome,
                locator,
            });
        }
        projects.push(ProjectRestore {
            id: recorded.id,
            name: recorded.name.clone(),
            expected: recorded.sources.len(),
            restored,
            present,
            sources,
        });
    }
    Ok(RestoreReport {
        verified: true,
        complete,
        projects,
    })
}

/// [`reconcile`] against a census that may not exist.
///
/// The absent case is the one the old restore surface got wrong: it restored a
/// database file, listed the file, and called that a success. There is no census
/// to check against, so nothing is checked and nothing is claimed — see
/// [`RestoreReport::unverified`].
pub fn reconcile_opt(census: &Option<Census>, store: &Store) -> Result<RestoreReport> {
    match census {
        Some(census) => reconcile(census, store),
        None => Ok(RestoreReport::unverified()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;
    use crate::creator::entities::{Project, SourceAsset};

    const AT: &str = "2026-09-26T00:00:00Z";

    struct Stage {
        store: Store,
        project: Project,
        /// Held only so the temp dir outlives the bytes the fixture wrote.
        #[allow(dead_code)]
        dir: TempDir,
        present: SourceAsset,
        absent: SourceAsset,
    }

    fn project(name: &str) -> Project {
        Project {
            id: ProjectId::generate(),
            name: name.into(),
            description: String::new(),
            channel: String::new(),
            series: String::new(),
            settings_json: "{}".into(),
            created_at: AT.into(),
            updated_at: AT.into(),
        }
    }

    /// One source whose bytes are really on disk, inside this test's own temp
    /// dir. The path is per-test on purpose: a shared location would let one
    /// test's `remove_file` decide another test's outcome, which is a race that
    /// reads as a reconciliation failure.
    fn source(dir: &Path, project: ProjectId, name: &str, fingerprint: &str) -> SourceAsset {
        let locator = dir.join(name);
        std::fs::write(&locator, name.as_bytes()).expect("bytes written");
        SourceAsset {
            id: SourceAssetId::generate(),
            project_id: project,
            kind: "recording".into(),
            title: name.into(),
            media_fingerprint: fingerprint.into(),
            locator: Some(locator),
            origin: "capture".into(),
            rights_json: "{}".into(),
            track_type: "video".into(),
            codec: "h264".into(),
            duration: None,
            created_at: AT.into(),
        }
    }

    /// One project with two sources, both present and both resolvable.
    fn stage() -> Stage {
        let dir = TempDir::new().expect("tempdir");
        let store = Store::open_in_memory().expect("store opens");
        let project = project("restored work");
        store.insert_project(&project).expect("project");
        let present = source(dir.path(), project.id, "a.mkv", "sha256:a");
        let absent = source(dir.path(), project.id, "b.mkv", "sha256:b");
        store.insert_source_asset(&present).expect("present");
        store.insert_source_asset(&absent).expect("absent");
        Stage {
            store,
            project,
            dir,
            present,
            absent,
        }
    }

    // ── the round trip ───────────────────────────────────────────────────────

    /// The acceptance criterion: a backup and a restore return a project with
    /// every source link resolving.
    #[test]
    fn a_round_trip_returns_a_project_with_every_source_link_resolving() {
        let stage = stage();
        let census = Census::snapshot(&stage.store, AT).expect("census");
        assert_eq!(census.projects.len(), 1, "one project");
        assert_eq!(census.source_count(), 2, "with both its sources");

        // The restore: the same rows in a store opened from the backup's copy.
        let restored = Store::open_in_memory().expect("store");
        restored.insert_project(&stage.project).expect("project");
        restored
            .insert_source_asset(&stage.present)
            .expect("present");
        restored.insert_source_asset(&stage.absent).expect("absent");

        let report = reconcile(&census, &restored).expect("reconcile");

        assert!(report.verified, "there was a census to check against");
        assert!(
            report.complete,
            "every recorded source came back whole: {report:?}"
        );
        let project = &report.projects[0];
        assert!(project.present, "the project row came back");
        assert_eq!(project.expected, 2, "against what the backup recorded");
        assert_eq!(project.restored, 2, "both sources resolved");
        assert!(
            project.missing().is_empty(),
            "so nothing is reported missing"
        );
    }

    /// The `Must NOT`, as a model-level assertion: a project whose source did
    /// not come back cannot be reported as a success, and the report names the
    /// source that is gone.
    #[test]
    fn a_restore_missing_one_source_is_not_reported_as_a_success() {
        let stage = stage();
        let census = Census::snapshot(&stage.store, AT).expect("census");

        // What a restore that lost one row looks like: the project and one of
        // its two sources.
        let restored = Store::open_in_memory().expect("store");
        restored.insert_project(&stage.project).expect("project");
        restored
            .insert_source_asset(&stage.present)
            .expect("present");

        let report = reconcile(&census, &restored).expect("reconcile");

        assert!(
            !report.complete,
            "a project whose sources did not come back is never a success"
        );
        let missing = report.missing();
        assert_eq!(missing.len(), 1, "exactly the one that is gone");
        assert_eq!(missing[0].1.id, stage.absent.id, "named by identity");
        assert_eq!(
            missing[0].1.title, "b.mkv",
            "and by the name a person recognises, which is what a UI shows"
        );
        assert_eq!(
            missing[0].1.outcome,
            SourceOutcome::MissingRow,
            "as a missing row"
        );
        assert_eq!(
            missing[0].0, "restored work",
            "under the project it belonged to"
        );
        assert_eq!(report.projects[0].restored, 1, "one of two came back");
        assert_eq!(report.projects[0].expected, 2, "against what was recorded");
    }

    /// Restoring the rows is not restoring the recordings. A row whose bytes are
    /// not on this machine is not a success, and the report says the link is
    /// intact so the operator knows a relink is the remedy.
    #[test]
    fn a_source_whose_bytes_did_not_come_back_is_not_a_success() {
        let stage = stage();
        let census = Census::snapshot(&stage.store, AT).expect("census");
        std::fs::remove_file(stage.absent.locator.as_ref().expect("locator"))
            .expect("the bytes went, as they would on another machine");

        let report = reconcile(&census, &stage.store).expect("reconcile");

        assert!(!report.complete, "the row came back; the recording did not");
        let missing = report.missing();
        assert_eq!(missing.len(), 1, "one source is not whole");
        assert_eq!(
            missing[0].1.outcome,
            SourceOutcome::Unresolved,
            "reported as unresolved, which is a distinct fact from a missing row"
        );
        assert_eq!(missing[0].1.id, stage.absent.id, "and named");
        // Looked up by id rather than by index: `sources_of` orders by
        // (created_at, id) and the fixture gives both sources one timestamp, so
        // position is the random uuid and an index here would be a coin flip.
        let survivor = report.projects[0]
            .sources
            .iter()
            .find(|s| s.id == stage.present.id)
            .expect("the source that did come back is still in the report");
        assert_eq!(
            survivor.outcome,
            SourceOutcome::Restored,
            "the other one is fine, and is reported as fine"
        );
    }

    /// A row that came back under the right id with different bytes is not the
    /// recording the backup recorded. Reporting it as restored would be a false
    /// success with the worst possible cause.
    #[test]
    fn a_row_whose_content_changed_is_not_the_recording_that_was_backed_up() {
        let stage = stage();
        let census = Census::snapshot(&stage.store, AT).expect("census");
        // Same id, different recording: a restore that wrote somebody else's
        // file over the top of the row.
        stage
            .store
            .raw_execute(
                "UPDATE creator_source_assets SET media_fingerprint=?2 WHERE id=?1",
                &[stage.absent.id.to_string(), "sha256:someone-else".into()],
            )
            .expect("content changed");

        let report = reconcile(&census, &stage.store).expect("reconcile");

        assert!(!report.complete, "a different recording is not a restore");
        assert_eq!(
            report.missing()[0].1.outcome,
            SourceOutcome::IdentityMismatch,
            "and it is its own outcome, not conflated with a missing row"
        );
    }

    /// A project the backup recorded and the restore lost is reported with every
    /// source it had, so the operator is not left to enumerate them by hand.
    #[test]
    fn a_project_that_did_not_come_back_names_every_source_it_had() {
        let stage = stage();
        let census = Census::snapshot(&stage.store, AT).expect("census");
        let empty = Store::open_in_memory().expect("store");

        let report = reconcile(&census, &empty).expect("reconcile");

        assert!(!report.complete, "nothing came back");
        let project = &report.projects[0];
        assert!(
            !project.present,
            "and the report says the project is absent"
        );
        assert_eq!(project.missing().len(), 2, "naming both of its sources");
        assert!(project
            .missing()
            .iter()
            .all(|s| s.outcome == SourceOutcome::MissingRow));
    }

    // ── unverified is not empty ─────────────────────────────────────────────

    /// A backup with no census cannot be checked, and "cannot be checked" must
    /// not read as "nothing is wrong". The previous surface's
    /// `{"restored": ["jobs.db"]}` is exactly that conflation.
    #[test]
    fn a_backup_with_no_census_is_unverified_rather_than_clean() {
        let stage = stage();
        let dir = TempDir::new().expect("tempdir");
        assert!(
            Census::read(dir.path()).expect("read").is_none(),
            "the fixture really has no census"
        );

        let report = reconcile_opt(&None, &stage.store).expect("reconcile");

        assert!(!report.verified, "nothing about projects was checked");
        assert!(!report.complete, "so nothing is claimed either");
        assert!(
            report.projects.is_empty(),
            "and no project is reported complete"
        );
    }

    #[test]
    fn a_census_survives_a_write_and_read_as_identities() {
        let stage = stage();
        let census = Census::snapshot(&stage.store, AT).expect("census");
        let dir = TempDir::new().expect("tempdir");

        census.write(dir.path()).expect("written");

        let read = Census::read(dir.path())
            .expect("read")
            .expect("a census is there");
        assert_eq!(read, census, "and it is the same record");
        assert_eq!(
            read.taken_at, AT,
            "stamped by the caller, not a clock read here"
        );
    }

    /// A census that exists but does not parse is a broken backup, not a backup
    /// with nothing in it. Reporting it as "no census" would let an incomplete
    /// restore be described as merely unverified.
    #[test]
    fn a_corrupt_census_is_an_error_rather_than_an_absent_one() {
        let dir = TempDir::new().expect("tempdir");
        std::fs::write(dir.path().join(CENSUS_FILE), b"{ not json").expect("written");

        assert!(
            Census::read(dir.path()).is_err(),
            "a corrupt census is refused, not read as no census"
        );
    }
}
