//! The release package and publication store: the only writers of package
//! content and of a publication's state.
//!
//! **Why the writes live here and not in `store_delivery`.** `store_delivery`
//! is the insert-only chain todo 21 built — every one of its methods is a
//! single `INSERT` with no `get` and no rule. The rules this todo adds are
//! *constraints on an existing row*: an edit that revokes an approval, a
//! transition that must consult the transition table, a gate re-run before
//! bytes leave. A file of bare inserts cannot express any of them, and putting
//! a rule next to the insert it guards is how two answers to "may this
//! package be approved" appear.
//!
//! **Every transition writes two rows or neither.** The state change and its
//! `creator_publication_events` row go in one transaction, so a publication can
//! never be observed in a state its own log does not record — and that log is
//! what makes an ambiguous upload recoverable, since re-checking a provider is
//! only possible if the local record says what was attempted.

use rusqlite::params;

use super::ids::{
    ArtifactId, CompositionRevisionId, ProjectId, PublicationId, ReleasePackageId, VariantId,
};
use super::publication::{
    PackageContent, PackageState, PublicationEvent, PublicationState, PublishError,
};
use super::rights::CalendarDate;
use super::store::Store;

/// What a caller supplies to draft a package. The id and the fingerprint are
/// the store's, so a caller cannot name a package that already exists or
/// claim a content hash it did not compute.
pub struct NewPackage {
    pub project_id: ProjectId,
    pub content: PackageContent,
    pub created_at: String,
}

/// A release package, read back with the full input set and the approval
/// record. Typed, unlike [`entities::ReleasePackage`], whose `state` is a
/// `String` — a loose string is how a UI ends up rendering a status the store
/// cannot represent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub id: ReleasePackageId,
    pub project_id: ProjectId,
    pub content: PackageContent,
    pub state: PackageState,
    /// Empty when no approval stands. A cleared approver is a revocation, not
    /// an anonymous approval.
    pub approved_by: String,
    pub approved_at: Option<String>,
    /// The content fingerprint the approval was given for. `None` means the
    /// package has never been approved — or that it was approved before this
    /// version recorded a fingerprint, which is the same honest answer.
    pub approved_fingerprint: Option<String>,
    pub created_at: String,
}

impl Package {
    /// Whether a person approved **this** content.
    ///
    /// A state flag alone is not enough: the approval is a claim about a
    /// fingerprint, so it is only true while that fingerprint is still the
    /// content's. An approval recorded against content that has since changed
    /// reads as not approved, which is the answer that keeps a later
    /// publication from being authorised by a stale sign-off.
    #[must_use]
    pub fn is_approved(&self) -> bool {
        self.state == PackageState::Approved
            && self.approved_at.is_some()
            && self.approved_fingerprint.as_deref() == Some(self.content.fingerprint().as_str())
    }
}

/// One destination posting, read back with the full vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posting {
    pub id: PublicationId,
    pub project_id: ProjectId,
    pub package_id: ReleasePackageId,
    pub destination: String,
    pub account: String,
    pub state: PublicationState,
    pub scheduled_at: Option<String>,
    pub timezone: String,
    pub remote_id: Option<String>,
    /// The package content fingerprint this posting was authorised against,
    /// copied when it left `draft` for `needs_review`. A later edit to the
    /// package cannot move it, which is what "a scheduled package is not
    /// silently altered" means in practice.
    pub approved_fingerprint: Option<String>,
    pub last_error: Option<String>,
    pub created_at: String,
}

pub struct NewPosting {
    pub project_id: ProjectId,
    pub package_id: ReleasePackageId,
    pub destination: String,
    pub account: String,
    pub timezone: String,
    pub created_at: String,
}

/// The extra facts a transition may need. `detail` is untrusted remote or
/// caller text, carried verbatim into the log and never interpreted.
#[derive(Debug, Clone, Default)]
pub struct Advance<'a> {
    pub detail: &'a str,
    pub scheduled_at: Option<&'a str>,
    pub remote_id: Option<&'a str>,
}

const PACKAGE_COLUMNS: &str = "id,project_id,artifact_id,composition_revision_id,variant_id,\
     metadata_json,source_ids_json,content_fingerprint,approved_by,approved_at,\
     approved_fingerprint,state,created_at";

impl Store {
    /// Draft a package. The store mints the id and the fingerprint, so the
    /// approval record can only ever describe content the store hashed.
    pub fn create_release_package(&self, req: NewPackage) -> Result<Package, PublishError> {
        let content = normalise(req.content);
        let fingerprint = content.fingerprint();
        let sources =
            serde_json::to_string(&content.sources).map_err(|e| PublishError::GateUnavailable {
                reason: format!("source set does not serialise: {e}"),
            })?;
        // The artifact must exist: a package naming an id no row holds would
        // approve nothing, and it would do so successfully.
        if self.artifact(content.artifact_id)?.is_none() {
            return Err(PublishError::GateUnavailable {
                reason: format!("no artifact {}", content.artifact_id),
            });
        }
        let id = ReleasePackageId::generate();
        self.conn.execute(
            "INSERT INTO creator_release_packages
             (id,project_id,artifact_id,composition_revision_id,variant_id,metadata_json,
              source_ids_json,content_fingerprint,approved_by,approved_at,approved_fingerprint,
              state,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'',NULL,NULL,'draft',?9)",
            params![
                id.to_string(),
                req.project_id.to_string(),
                content.artifact_id.to_string(),
                content.composition_revision_id.to_string(),
                content.variant_id.to_string(),
                content.metadata_json,
                sources,
                fingerprint,
                req.created_at,
            ],
        )?;
        self.package(id)
    }

    /// One package by its identity.
    pub fn package(&self, id: ReleasePackageId) -> Result<Package, PublishError> {
        let row = self
            .conn
            .query_row(
                &format!("SELECT {PACKAGE_COLUMNS} FROM creator_release_packages WHERE id=?1"),
                [id.to_string()],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, String>(7)?,
                        r.get::<_, String>(8)?,
                        r.get::<_, Option<String>>(9)?,
                        r.get::<_, Option<String>>(10)?,
                        r.get::<_, String>(11)?,
                        r.get::<_, String>(12)?,
                    ))
                },
            )
            .ok();
        let Some(row) = row else {
            return Err(PublishError::UnknownPackage { id: id.to_string() });
        };
        Ok(Package {
            id,
            project_id: ProjectId::from_uuid(uuid(&row.1, 1)?),
            content: PackageContent {
                artifact_id: ArtifactId::from_uuid(uuid(&row.2, 2)?),
                // A pre-version-9 row has no revision or variant recorded. That
                // is reported as absent rather than guessed backwards from the
                // artifact, because a guess would be an approval of an edit
                // nobody re-reviewed.
                composition_revision_id: match &row.3 {
                    Some(v) => CompositionRevisionId::from_uuid(uuid(v, 3)?),
                    None => {
                        return Err(PublishError::GateUnavailable {
                            reason: format!("package {id} records no composition revision"),
                        })
                    }
                },
                variant_id: match &row.4 {
                    Some(v) => VariantId::from_uuid(uuid(v, 4)?),
                    None => {
                        return Err(PublishError::GateUnavailable {
                            reason: format!("package {id} records no variant"),
                        })
                    }
                },
                metadata_json: row.5,
                sources: parse_sources(&row.6)?,
            },
            state: row.11.parse::<PackageState>()?,
            approved_by: row.8,
            approved_at: row.9,
            approved_fingerprint: row.10,
            created_at: row.12,
        })
    }

    /// Replace a package's content.
    ///
    /// **Two refusals, and they are different questions.**
    ///
    /// * A package some publication has already carried out of `approved` is
    ///   [`PublishError::PackageLocked`]. This is the structural half of
    ///   immutability: a *scheduled* posting cannot be altered by an edit, by
    ///   any caller, because the store will not perform the edit.
    /// * A `superseded` package is [`PublishError::NotEditable`]. A
    ///   supersession is a statement about the draft; rewriting the history it
    ///   replaced would be rewriting the record.
    ///
    /// An **approved** package *is* editable, and the edit **revokes the
    /// approval in the same transaction**: the row returns to `draft` with
    /// `approved_by`, `approved_at` and `approved_fingerprint` cleared. The
    /// report requires editing an approved draft to invalidate its approval,
    /// and doing it in one transaction is what stops a reader from seeing the
    /// new content under the old approval.
    pub fn edit_release_package(
        &self,
        id: ReleasePackageId,
        content: PackageContent,
    ) -> Result<Package, PublishError> {
        let current = self.package(id)?;
        if current.state == PackageState::Superseded {
            return Err(PublishError::NotEditable {
                state: current.state,
            });
        }
        if let Some(state) = self.locking_publication_state(id)? {
            return Err(PublishError::PackageLocked { state });
        }
        let content = normalise(content);
        if self.artifact(content.artifact_id)?.is_none() {
            return Err(PublishError::GateUnavailable {
                reason: format!("no artifact {}", content.artifact_id),
            });
        }
        let fingerprint = content.fingerprint();
        let sources =
            serde_json::to_string(&content.sources).map_err(|e| PublishError::GateUnavailable {
                reason: format!("source set does not serialise: {e}"),
            })?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE creator_release_packages
             SET artifact_id=?2, composition_revision_id=?3, variant_id=?4, metadata_json=?5,
                 source_ids_json=?6, content_fingerprint=?7,
                 approved_by='', approved_at=NULL, approved_fingerprint=NULL, state='draft'
             WHERE id=?1",
            params![
                id.to_string(),
                content.artifact_id.to_string(),
                content.composition_revision_id.to_string(),
                content.variant_id.to_string(),
                content.metadata_json,
                sources,
                fingerprint,
            ],
        )?;
        tx.commit()?;
        self.package(id)
    }

    /// Approve a package: run the rights gate over its recorded source set and,
    /// if it clears, record the approval against the content fingerprint.
    ///
    /// The gate is [`super::Store::publication_report`] over the notes as they
    /// stand **now** — a verdict is derived, never stored, so re-approving after
    /// a withdrawal refuses where the earlier approval would have passed.
    pub fn approve_release_package(
        &self,
        id: ReleasePackageId,
        approver: &str,
        as_of: &CalendarDate,
        at: &str,
    ) -> Result<Package, PublishError> {
        let package = self.package(id)?;
        if package.state != PackageState::Draft {
            return Err(PublishError::NotEditable {
                state: package.state,
            });
        }
        let report = self.publication_report(&package.content.sources, as_of)?;
        if !report.is_publishable() {
            return Err(PublishError::Rights(report));
        }
        self.conn.execute(
            "UPDATE creator_release_packages
             SET state='approved', approved_by=?2, approved_at=?3, approved_fingerprint=?4
             WHERE id=?1",
            params![id.to_string(), approver, at, package.content.fingerprint()],
        )?;
        self.package(id)
    }

    /// Mark a package superseded. Never touches its publications: what was
    /// scheduled against it stays scheduled against it.
    pub fn supersede_release_package(&self, id: ReleasePackageId) -> Result<Package, PublishError> {
        let package = self.package(id)?;
        if package.state == PackageState::Superseded {
            return Ok(package);
        }
        self.conn.execute(
            "UPDATE creator_release_packages SET state='superseded' WHERE id=?1",
            [id.to_string()],
        )?;
        self.package(id)
    }

    /// Create a posting in `draft`. Nothing has been shown to a provider and
    /// nothing needs approving to get here.
    pub fn create_posting(&self, req: NewPosting) -> Result<Posting, PublishError> {
        // The package must exist. An id belonging to another project is
        // reported as absent, so a caller cannot post against a neighbour.
        let package = self.package(req.package_id)?;
        if package.project_id != req.project_id {
            return Err(PublishError::UnknownPackage {
                id: req.package_id.to_string(),
            });
        }
        let id = PublicationId::generate();
        self.conn.execute(
            "INSERT INTO creator_publications
             (id,project_id,release_package_id,destination,account,state,scheduled_at,timezone,
              remote_id,uploaded_at,published_at,last_error,created_at,approved_fingerprint)
             VALUES(?1,?2,?3,?4,?5,'draft',NULL,?6,NULL,NULL,NULL,NULL,?7,NULL)",
            params![
                id.to_string(),
                req.project_id.to_string(),
                req.package_id.to_string(),
                req.destination,
                req.account,
                req.timezone,
                req.created_at,
            ],
        )?;
        self.posting(id)
    }

    /// One posting by its identity.
    pub fn posting(&self, id: PublicationId) -> Result<Posting, PublishError> {
        let row = self
            .conn
            .query_row(
                "SELECT id,project_id,release_package_id,destination,account,state,scheduled_at,
                        timezone,remote_id,approved_fingerprint,last_error,created_at
                 FROM creator_publications WHERE id=?1",
                [id.to_string()],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        r.get::<_, String>(7)?,
                        r.get::<_, Option<String>>(8)?,
                        r.get::<_, Option<String>>(9)?,
                        r.get::<_, Option<String>>(10)?,
                        r.get::<_, String>(11)?,
                    ))
                },
            )
            .ok();
        let Some(row) = row else {
            return Err(PublishError::UnknownPublication { id: id.to_string() });
        };
        Ok(Posting {
            id,
            project_id: ProjectId::from_uuid(uuid(&row.1, 1)?),
            package_id: ReleasePackageId::from_uuid(uuid(&row.2, 2)?),
            destination: row.3,
            account: row.4,
            state: row.5.parse::<PublicationState>()?,
            scheduled_at: row.6,
            timezone: row.7,
            remote_id: row.8,
            approved_fingerprint: row.9,
            last_error: row.10,
            created_at: row.11,
        })
    }

    /// The furthest state any of a package's postings has reached, when that
    /// state locks the package's content. `None` when nothing has.
    ///
    /// Locking rather than refusing on the *state* alone is deliberate: a
    /// posting that is `cancelled` or `removed` reached nowhere, so a package
    /// whose only posting was abandoned stays editable.
    pub fn locking_publication_state(
        &self,
        package: ReleasePackageId,
    ) -> Result<Option<PublicationState>, PublishError> {
        let mut stmt = self.conn.prepare(
            "SELECT state FROM creator_publications WHERE release_package_id=?1 ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map([package.to_string()], |r| r.get::<_, String>(0))?;
        let mut locking: Option<PublicationState> = None;
        for row in rows {
            let state = row?.parse::<PublicationState>()?;
            if state.locks_package() {
                locking = Some(state);
                break;
            }
        }
        Ok(locking)
    }

    /// Move a posting to `to`, if the transition table admits it.
    ///
    /// Four refusals beyond the table, each one a fact a provider can contradict
    /// and a database cannot:
    ///
    /// * `published` needs a `remote_id` ([`PublishError::MissingRemoteId`]) —
    ///   "live" with nothing to point at is the claim this module exists to
    ///   prevent.
    /// * `scheduled` needs a time ([`PublishError::MissingSchedule`]).
    /// * `approved` needs an approved package, and stamps the package's
    ///   fingerprint onto the posting so a later edit cannot reach it.
    /// * anything crossing into a remote state re-runs the rights gate against
    ///   `as_of`, so a note withdrawn after approval blocks the bytes leaving
    ///   rather than the approval being trusted forward.
    pub fn advance_publication(
        &self,
        id: PublicationId,
        to: PublicationState,
        advance: &Advance<'_>,
        as_of: &CalendarDate,
        at: &str,
    ) -> Result<Posting, PublishError> {
        let posting = self.posting(id)?;
        let from = posting.state;
        if !super::publication::is_legal(from, to) {
            return Err(PublishError::IllegalTransition { from, to });
        }
        if to == PublicationState::Published && advance.remote_id.is_none() {
            return Err(PublishError::MissingRemoteId);
        }
        if to == PublicationState::Scheduled && advance.scheduled_at.is_none() {
            return Err(PublishError::MissingSchedule);
        }
        let package = self.package(posting.package_id)?;
        if to == PublicationState::Approved && !package.is_approved() {
            return Err(PublishError::PackageNotApproved {
                state: package.state,
            });
        }
        if to.is_remote() && !from.is_remote() {
            // A note that was never written, was withdrawn, or has expired
            // stops the bytes here — the same fail-closed reading approval got.
            let report = self.publication_report(&package.content.sources, as_of)?;
            if !report.is_publishable() {
                return Err(PublishError::Rights(report));
            }
        }
        let approved_fingerprint = match (posting.approved_fingerprint.clone(), to) {
            (None, PublicationState::Approved) => Some(package.content.fingerprint()),
            (existing, _) => existing,
        };
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE creator_publications
             SET state=?2,
                 scheduled_at=COALESCE(?3, scheduled_at),
                 remote_id=COALESCE(?4, remote_id),
                 uploaded_at=CASE WHEN ?2='uploading' THEN ?5 ELSE uploaded_at END,
                 published_at=CASE WHEN ?2='published' THEN ?5 ELSE published_at END,
                 last_error=CASE WHEN ?2 IN ('failed','needs_reauthorization','unknown_remote_state')
                                 THEN ?6 ELSE last_error END,
                 approved_fingerprint=?7
             WHERE id=?1",
            params![
                id.to_string(),
                to.as_str(),
                advance.scheduled_at,
                advance.remote_id,
                at,
                advance.detail,
                approved_fingerprint,
            ],
        )?;
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM creator_publication_events WHERE publication_id=?1",
            [id.to_string()],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO creator_publication_events
             (publication_id,seq,from_state,to_state,detail,recorded_at)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                id.to_string(),
                seq,
                from.as_str(),
                to.as_str(),
                advance.detail,
                at,
            ],
        )?;
        tx.commit()?;
        self.posting(id)
    }

    /// A posting's action log, oldest first. Append-only: nothing in this
    /// module updates or deletes one of these rows.
    pub fn publication_events(
        &self,
        id: PublicationId,
    ) -> Result<Vec<PublicationEvent>, PublishError> {
        let mut stmt = self.conn.prepare(
            "SELECT id,publication_id,seq,from_state,to_state,detail,recorded_at
             FROM creator_publication_events WHERE publication_id=?1 ORDER BY seq",
        )?;
        let rows = stmt.query_map([id.to_string()], |r| {
            let from: Option<String> = r.get(3)?;
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                from,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (eid, pid, seq, from, to, detail, recorded_at) = row?;
            out.push(PublicationEvent {
                id: eid,
                publication_id: PublicationId::from_uuid(uuid(&pid, 1)?),
                seq,
                from: from.map(|s| s.parse::<PublicationState>()).transpose()?,
                to: to.parse::<PublicationState>()?,
                detail,
                recorded_at,
            });
        }
        Ok(out)
    }

    /// The statuses this store can hold, for the surface that renders them.
    ///
    /// One function, so a UI's vocabulary and the schema's are the same list by
    /// construction. The acceptance criterion "the UI never shows a draft
    /// status the store cannot represent" is this function's test.
    #[must_use]
    pub fn representable_statuses() -> Vec<&'static str> {
        let mut names: Vec<&'static str> = PublicationState::ALL
            .iter()
            .map(|s| s.as_str())
            .chain(PackageState::ALL.iter().map(|s| s.as_str()))
            .collect();
        names.sort_unstable();
        names
    }
}

/// Sort and dedup the source set, so a fingerprint cannot depend on the order
/// a caller happened to build the list in.
fn normalise(mut content: PackageContent) -> PackageContent {
    content.sources.sort();
    content.sources.dedup();
    content
}

fn parse_sources(raw: &str) -> Result<Vec<super::ids::SourceAssetId>, PublishError> {
    serde_json::from_str(raw).map_err(|e| PublishError::GateUnavailable {
        reason: format!("recorded source set does not parse: {e}"),
    })
}

fn uuid(value: &str, column: usize) -> Result<uuid::Uuid, PublishError> {
    uuid::Uuid::parse_str(value).map_err(|e| PublishError::GateUnavailable {
        reason: format!("column {column} is not an id ({value:?}): {e}"),
    })
}
