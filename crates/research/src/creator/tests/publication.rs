//! The release package and the publication lifecycle.
//!
//! ## Why the transition test is a table and not a spot-check
//!
//! `every_state_pair_is_classified_exactly_as_the_table_says` drives all
//! 14 × 14 = 196 ordered pairs through [`is_legal`] and compares each answer to
//! the table, then asserts the number of legal edges equals the table's length
//! and that the length is the number this module claims. A spot-check of the
//! happy path would pass against a table that had quietly lost
//! `unknown_remote_state → published`, which is one of the edges that makes
//! reconciliation possible. `every_state_is_reachable_from_draft` closes the
//! other direction: a state nothing reaches is a status a UI could render and
//! the store could never hold.
//!
//! ## The two structural claims
//!
//! `editing_an_approved_draft_revokes_the_approval` is the QA scenario: the
//! approval is not merely ignored, it is cleared in the same transaction, and a
//! publication can no longer be authorised by it.
//! `a_scheduled_package_cannot_be_altered_by_a_later_edit` is the other half:
//! the store refuses the edit outright, and the posting keeps the fingerprint it
//! was authorised against.
//!
//! ## The honesty state
//!
//! `an_ambiguous_upload_is_a_third_outcome` asserts `unknown_remote_state` is
//! reachable, is neither `failed` nor `published`, carries no remote id, and has
//! **no** edge back to `uploading` — re-posting is the one thing it must not
//! offer, because the request may well have succeeded.

use super::super::ids::{ArtifactId, CompositionRevisionId, ProjectId, SourceAssetId, VariantId};
use super::super::publication::{
    is_legal, PackageContent, PackageState, PublicationState, PublishError, TRANSITIONS,
};
use super::super::rights::{CalendarDate, Consent, Licence, Ownership, SourceNote};
use super::super::store::Store;
use super::super::{Advance, NewPackage, NewPosting};
use super::fixtures;

/// Everything a package names, so a test states only the field its assertion is
/// about. `_dir` is held so the database outlives the fixture.
struct Fixture {
    store: Store,
    _dir: tempfile::TempDir,
    project: ProjectId,
    revision: CompositionRevisionId,
    variant: VariantId,
    artifact: ArtifactId,
    source: SourceAssetId,
}

fn day(text: &str) -> CalendarDate {
    CalendarDate::parse(text).expect("a calendar date")
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("research.db")).expect("open");
    let project = fixtures::project("streamer");
    store.insert_project(&project).expect("project inserted");

    let mut source = fixtures::source(project.id, None);
    source.media_fingerprint = "sha256:release-fixture".into();
    store.insert_source_asset(&source).expect("source inserted");
    // A note the creator could reasonably have written, so a test about the
    // lifecycle is not accidentally a test about the gate. The gate has its
    // own tests below.
    store
        .record_note(
            &SourceNote {
                source: source.id,
                ownership: Ownership::CreatorOwned,
                licence: Licence::Granted {
                    grant: "my own capture".into(),
                },
                consent: Consent::NotApplicable,
                disclosure: String::new(),
                recorded_at: "2026-09-07T00:00:00Z".into(),
                recorded_by: "operator".into(),
            }
            .validated()
            .expect("a complete note validates"),
        )
        .expect("note recorded");

    let revision = fixtures::revision(project.id, 1, None);
    store
        .insert_composition_revision(&revision)
        .expect("revision inserted");
    let variant = fixtures::variant(project.id, revision.id);
    store.insert_variant(&variant).expect("variant inserted");
    let artifact = fixtures::artifact(project.id, variant.id);
    store.insert_artifact(&artifact).expect("artifact inserted");

    Fixture {
        store,
        _dir: dir,
        project: project.id,
        revision: revision.id,
        variant: variant.id,
        artifact: artifact.id,
        source: source.id,
    }
}

impl Fixture {
    fn content(&self) -> PackageContent {
        self.content_titled("stream highlights")
    }

    fn content_titled(&self, title: &str) -> PackageContent {
        PackageContent {
            composition_revision_id: self.revision,
            variant_id: self.variant,
            artifact_id: self.artifact,
            metadata_json: format!(r#"{{"title":"{title}","privacy":"public"}}"#),
            sources: vec![self.source],
        }
    }

    fn draft(&self) -> super::super::Package {
        self.draft_with(self.content())
    }

    fn draft_with(&self, content: PackageContent) -> super::super::Package {
        self.store
            .create_release_package(NewPackage {
                project_id: self.project,
                content,
                created_at: "2026-09-07T00:00:03Z".into(),
            })
            .expect("package drafted")
    }

    fn approved(&self) -> super::super::Package {
        let package = self.draft();
        self.store
            .approve_release_package(
                package.id,
                "operator",
                &day("2026-09-07"),
                "2026-09-07T01:00:00Z",
            )
            .expect("the gate clears and the package is approved")
    }

    fn new_posting(&self, package: &super::super::Package) -> super::super::Posting {
        self.store
            .create_posting(NewPosting {
                project_id: package.project_id,
                package_id: package.id,
                destination: "youtube".into(),
                account: "creator@example".into(),
                timezone: "UTC".into(),
                created_at: "2026-09-07T02:00:00Z".into(),
            })
            .expect("posting drafted")
    }

    fn step(
        &self,
        posting: &super::super::Posting,
        to: PublicationState,
        advance: Advance<'_>,
    ) -> super::super::Posting {
        self.store
            .advance_publication(
                posting.id,
                to,
                &advance,
                &day("2026-09-07"),
                "2026-09-07T03:00:00Z",
            )
            .unwrap_or_else(|e| panic!("{} -> {to}: {e}", posting.state))
    }

    /// A posting walked the length of the documented happy path to `target`.
    fn posting_at(
        &self,
        package: &super::super::Package,
        target: PublicationState,
    ) -> super::super::Posting {
        let path: &[PublicationState] = match target {
            PublicationState::NeedsReview => &[PublicationState::NeedsReview],
            PublicationState::Approved => {
                &[PublicationState::NeedsReview, PublicationState::Approved]
            }
            PublicationState::Rendering => &[
                PublicationState::NeedsReview,
                PublicationState::Approved,
                PublicationState::Rendering,
            ],
            PublicationState::Ready => &[
                PublicationState::NeedsReview,
                PublicationState::Approved,
                PublicationState::Rendering,
                PublicationState::Ready,
            ],
            PublicationState::Uploading => &[
                PublicationState::NeedsReview,
                PublicationState::Approved,
                PublicationState::Rendering,
                PublicationState::Ready,
                PublicationState::Uploading,
            ],
            PublicationState::Processing => &[
                PublicationState::NeedsReview,
                PublicationState::Approved,
                PublicationState::Rendering,
                PublicationState::Ready,
                PublicationState::Uploading,
                PublicationState::Processing,
            ],
            PublicationState::Scheduled => &[
                PublicationState::NeedsReview,
                PublicationState::Approved,
                PublicationState::Rendering,
                PublicationState::Ready,
                PublicationState::Uploading,
                PublicationState::Processing,
                PublicationState::Scheduled,
            ],
            PublicationState::Published => &[
                PublicationState::NeedsReview,
                PublicationState::Approved,
                PublicationState::Rendering,
                PublicationState::Ready,
                PublicationState::Uploading,
                PublicationState::Processing,
                PublicationState::Published,
            ],
            other => panic!("no documented path to {other} in this fixture"),
        };
        let mut current = self.new_posting(package);
        for to in path {
            let advance = match to {
                PublicationState::Scheduled => Advance {
                    detail: "accepted with a publish time",
                    scheduled_at: Some("2026-09-08T09:00:00Z"),
                    remote_id: None,
                },
                PublicationState::Published => Advance {
                    detail: "provider reports it live",
                    scheduled_at: None,
                    remote_id: Some("yt-abc123"),
                },
                _ => Advance {
                    detail: "advanced",
                    ..Default::default()
                },
            };
            current = self.step(&current, *to, advance);
        }
        current
    }
}

// ------------------------------------------- the state enumeration is complete

/// Given the fourteen states and the transition table
/// When every ordered pair of them is classified
/// Then the answer matches the table for all 196 pairs, the legal count equals
/// the table's length, and that length is the number this module claims.
#[test]
fn every_state_pair_is_classified_exactly_as_the_table_says() {
    let mut legal: Vec<(PublicationState, PublicationState)> = Vec::new();
    let mut pairs = 0usize;
    for from in PublicationState::ALL {
        for to in PublicationState::ALL {
            pairs += 1;
            let in_table = TRANSITIONS.contains(&(from, to));
            assert_eq!(
                is_legal(from, to),
                in_table,
                "{from} -> {to} disagrees with the table",
            );
            if in_table {
                legal.push((from, to));
            }
        }
    }
    assert_eq!(pairs, 196, "all 14 x 14 ordered pairs were driven");
    assert_eq!(
        legal.len(),
        TRANSITIONS.len(),
        "the table either lists an edge twice or omits one it admits",
    );
    assert_eq!(
        legal.len(),
        46,
        "46 legal transitions, as this module claims"
    );
    let mut seen = legal.clone();
    seen.sort_by_key(|(from, to)| (from.as_str(), to.as_str()));
    seen.dedup();
    assert_eq!(seen.len(), legal.len(), "no edge is listed twice");
    for (from, to) in &legal {
        assert!(
            PublicationState::ALL.contains(from) && PublicationState::ALL.contains(to),
            "{from} -> {to} names a state outside ALL",
        );
    }
}

/// Given the state list
/// When each name is rendered and parsed back
/// Then every state has a distinct, round-tripping, lower-case spelling — the
/// three things a UI, a column CHECK and a URL all depend on.
#[test]
fn every_state_has_a_distinct_round_tripping_name() {
    let mut names: Vec<&str> = Vec::new();
    for state in PublicationState::ALL {
        let name = state.as_str();
        assert_eq!(name, name.to_lowercase(), "{state} is not lower case");
        assert_eq!(
            name.parse::<PublicationState>().expect("round trip"),
            state,
            "{state} does not parse back to itself",
        );
        assert_eq!(state.to_string(), name, "{state} Display disagrees");
        names.push(name);
    }
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    assert_eq!(names.len(), before, "two states share a spelling");
    assert_eq!(before, 14, "and there are fourteen of them");
}

/// Given the transition table
/// When every state's outgoing edges are followed from `draft`
/// Then all fourteen states are reachable — a state nothing reaches is a status
/// the UI could show and the store could never represent.
#[test]
fn every_state_is_reachable_from_draft() {
    let mut seen = vec![PublicationState::Draft];
    let mut frontier = 0;
    while frontier < seen.len() {
        let from = seen[frontier];
        frontier += 1;
        for to in PublicationState::ALL {
            if is_legal(from, to) && !seen.contains(&to) {
                seen.push(to);
            }
        }
    }
    for state in PublicationState::ALL {
        assert!(seen.contains(&state), "{state} is unreachable from draft");
    }
    assert_eq!(seen.len(), 14, "and nothing extra was invented");
}

// ---------------------------------------- the UI cannot show an absent status

/// Given the two vocabularies this store holds
/// When they are listed as a surface would render them
/// Then every name parses back to a real state, and each state is offered.
#[test]
fn every_status_the_ui_can_render_is_representable() {
    let names = Store::representable_statuses();
    assert_eq!(
        names.len(),
        PublicationState::ALL.len() + PackageState::ALL.len(),
        "the surface is exactly the two vocabularies",
    );
    for name in &names {
        assert!(
            name.parse::<PublicationState>().is_ok() || name.parse::<PackageState>().is_ok(),
            "the surface offers {name}, which neither state type can hold",
        );
    }
    for name in PublicationState::ALL
        .iter()
        .map(|s| s.as_str())
        .chain(PackageState::ALL.iter().map(|s| s.as_str()))
    {
        assert!(names.contains(&name), "{name} is holdable but not offered",);
    }
}

// ------------------------------------------------- QA: the approval is revoked

/// Given a package a person approved
/// When its metadata is edited
/// Then the approval is **revoked** — cleared, not merely ignored — the package
/// is a draft again, and a publication can no longer be authorised by it.
#[test]
fn editing_an_approved_draft_revokes_the_approval() {
    let f = fixture();
    let approved = f.approved();
    assert!(approved.is_approved(), "the approval stands to begin with");
    let approved_fingerprint = approved.content.fingerprint();
    assert_eq!(
        approved.approved_fingerprint.as_deref(),
        Some(approved_fingerprint.as_str()),
        "the approval names the content it approved",
    );

    // When the metadata is edited
    let edited = f
        .store
        .edit_release_package(approved.id, f.content_titled("a different title"))
        .expect("an approved package with nothing scheduled is still editable");

    // Then the approval is gone, in every field that carried it
    assert_eq!(edited.state, PackageState::Draft, "back to a draft");
    assert!(edited.approved_at.is_none(), "the approval time is cleared");
    assert_eq!(edited.approved_by, "", "the approver is cleared");
    assert_eq!(
        edited.approved_fingerprint, None,
        "the approved fingerprint is cleared",
    );
    assert!(!edited.is_approved(), "and it does not read as approved");
    assert_ne!(
        edited.content.fingerprint(),
        approved_fingerprint,
        "the content really did change",
    );

    // And a publication cannot lean on the revoked approval
    let posting = f.new_posting(&edited);
    f.step(&posting, PublicationState::NeedsReview, Advance::default());
    assert_eq!(
        f.store
            .advance_publication(
                posting.id,
                PublicationState::Approved,
                &Advance::default(),
                &day("2026-09-07"),
                "2026-09-07T02:06:00Z",
            )
            .expect_err("a revoked approval authorises nothing"),
        PublishError::PackageNotApproved {
            state: PackageState::Draft,
        },
    );
}

// ---------------------------------------- a scheduled package cannot be edited

/// Given a package approved and a posting already scheduled against it
/// When somebody edits the package
/// Then the store refuses, names the state that froze it, and the posting keeps
/// the fingerprint it was authorised against.
#[test]
fn a_scheduled_package_cannot_be_altered_by_a_later_edit() {
    let f = fixture();
    let approved = f.approved();
    let approved_fingerprint = approved.content.fingerprint();
    let scheduled = f.posting_at(&approved, PublicationState::Scheduled);
    assert_eq!(scheduled.state, PublicationState::Scheduled);
    assert_eq!(
        scheduled.approved_fingerprint.as_deref(),
        Some(approved_fingerprint.as_str()),
        "the posting was authorised against the approved content",
    );

    let refused = f
        .store
        .edit_release_package(scheduled.package_id, f.content_titled("a later edit"))
        .expect_err("a scheduled package is frozen");

    assert_eq!(
        refused,
        PublishError::PackageLocked {
            state: PublicationState::Scheduled,
        },
        "the refusal names the state that froze the content",
    );

    // Nothing moved: the package, the posting and the log are all as they were.
    let package = f.store.package(scheduled.package_id).expect("read back");
    assert!(package.is_approved(), "the approval still stands");
    assert_eq!(
        package.content.fingerprint(),
        approved_fingerprint,
        "and still describes the content the posting was authorised against",
    );
    assert_eq!(
        package.content.metadata_json, approved.content.metadata_json,
        "the metadata is untouched",
    );
    let after = f.store.posting(scheduled.id).expect("read back");
    assert_eq!(after, scheduled, "and the whole posting row is unchanged");
    let log = f.store.publication_events(scheduled.id).expect("log");
    assert_eq!(log.len(), 7, "seven transitions, and no eighth");
    assert_eq!(
        log.last().expect("an entry").to,
        PublicationState::Scheduled,
    );
}

/// Given a package whose only posting was abandoned before anything left
/// When it is edited
/// Then the edit is allowed — a posting that reached nowhere froze nothing.
#[test]
fn a_cancelled_posting_does_not_freeze_the_package() {
    let f = fixture();
    let approved = f.approved();
    let posting = f.new_posting(&approved);
    f.step(
        &posting,
        PublicationState::Cancelled,
        Advance {
            detail: "changed my mind",
            ..Default::default()
        },
    );

    let edited = f
        .store
        .edit_release_package(approved.id, f.content_titled("after the change of heart"))
        .expect("nothing was scheduled, so the edit stands");
    assert_eq!(
        edited.state,
        PackageState::Draft,
        "and the approval is gone"
    );
    assert!(!edited.is_approved());
}

// ------------------------------------------------ the honesty state, third way

/// Given an upload whose response never arrived
/// When it is recorded
/// Then it is neither `failed` nor `published`, it holds no remote id, it is not
/// an ending, and it offers no way back to `uploading` — the one thing it must
/// not suggest is that the request is safe to repeat.
#[test]
fn an_ambiguous_upload_is_a_third_outcome() {
    let f = fixture();
    let approved = f.approved();
    let uploading = f.posting_at(&approved, PublicationState::Uploading);

    let ambiguous = f.step(
        &uploading,
        PublicationState::UnknownRemoteState,
        Advance {
            detail: "the request left; no answer before the deadline",
            ..Default::default()
        },
    );

    // A third outcome, not a failure and not a success
    assert_eq!(ambiguous.state, PublicationState::UnknownRemoteState);
    assert_ne!(ambiguous.state, PublicationState::Failed);
    assert_ne!(ambiguous.state, PublicationState::Published);
    assert!(
        ambiguous.remote_id.is_none(),
        "nothing was confirmed, so no id is claimed",
    );
    assert!(
        !ambiguous.state.is_final(),
        "it is not an ending — it is an unanswered question",
    );
    assert!(
        ambiguous.state.is_remote(),
        "it is a remote fact, not a local one",
    );

    // No edge repeats the upload
    assert!(
        !is_legal(
            PublicationState::UnknownRemoteState,
            PublicationState::Uploading
        ),
        "re-posting from an unknown state is the duplicate-posting bug",
    );
    assert!(
        !is_legal(PublicationState::Uploading, PublicationState::Published),
        "an upload the provider acknowledged has published nothing",
    );

    // Its only exits are reconciliations: each establishes a fact
    let mut exits: Vec<PublicationState> = PublicationState::ALL
        .into_iter()
        .filter(|to| is_legal(PublicationState::UnknownRemoteState, *to))
        .collect();
    exits.sort_by_key(|s| s.as_str());
    let mut expected = vec![
        PublicationState::Processing,
        PublicationState::Published,
        PublicationState::Failed,
        PublicationState::NeedsReauthorization,
        PublicationState::Removed,
    ];
    expected.sort_by_key(|s| s.as_str());
    assert_eq!(
        exits, expected,
        "exactly the states a status check can establish, and nothing else",
    );

    // And reconciling to `published` still demands the provider's own id
    assert_eq!(
        f.store
            .advance_publication(
                ambiguous.id,
                PublicationState::Published,
                &Advance::default(),
                &day("2026-09-07"),
                "2026-09-07T04:00:00Z",
            )
            .expect_err("reconciliation is not a claim you may skip the id for"),
        PublishError::MissingRemoteId,
    );
    let published = f.step(
        &ambiguous,
        PublicationState::Published,
        Advance {
            detail: "the provider lists it",
            scheduled_at: None,
            remote_id: Some("yt-reconciled"),
        },
    );
    assert_eq!(published.state, PublicationState::Published);
    assert_eq!(published.remote_id.as_deref(), Some("yt-reconciled"));
}

// ------------------------------------------------------------ the preconditions

/// Given a posting the provider is already transcoding
/// When it is marked `scheduled` with no time, or `published` with no remote id
/// Then both are refused, because neither claim can be made from local state.
#[test]
fn published_needs_a_remote_id_and_scheduled_needs_a_time() {
    let f = fixture();
    let approved = f.approved();
    let processing = f.posting_at(&approved, PublicationState::Processing);

    assert_eq!(
        f.store
            .advance_publication(
                processing.id,
                PublicationState::Scheduled,
                &Advance::default(),
                &day("2026-09-07"),
                "2026-09-07T03:12:00Z",
            )
            .expect_err("a schedule with no time is not a schedule"),
        PublishError::MissingSchedule,
    );
    assert_eq!(
        f.store
            .advance_publication(
                processing.id,
                PublicationState::Published,
                &Advance::default(),
                &day("2026-09-07"),
                "2026-09-07T03:13:00Z",
            )
            .expect_err("live with nothing to point at is not live"),
        PublishError::MissingRemoteId,
    );
    // The refused attempts left no trace in the state, and no row in the log.
    assert_eq!(
        f.store.posting(processing.id).expect("read back").state,
        PublicationState::Processing,
    );
    assert_eq!(
        f.store
            .publication_events(processing.id)
            .expect("log")
            .len(),
        6,
        "six transitions were made and only those six are logged",
    );
}

// ------------------------------------------------------------- the rights gate

/// Given a package whose source list includes a source nobody ever reviewed
/// When approval is attempted
/// Then it is refused and the report names the blocking source, because a
/// missing note is undecided on every axis and therefore not publishable.
#[test]
fn a_package_over_an_unnoted_source_is_refused() {
    let f = fixture();
    let mut unnoted = fixtures::source(f.project, None);
    unnoted.media_fingerprint = "sha256:never-reviewed".into();
    f.store
        .insert_source_asset(&unnoted)
        .expect("source inserted");

    let mut content = f.content();
    content.sources.push(unnoted.id);
    let package = f.draft_with(content);

    match f.store.approve_release_package(
        package.id,
        "operator",
        &day("2026-09-07"),
        "2026-09-07T01:00:00Z",
    ) {
        Err(PublishError::Rights(report)) => {
            assert!(!report.is_publishable());
            assert_eq!(
                report.blocking_sources(),
                vec![unnoted.id],
                "the report names the blocking source",
            );
        }
        other => panic!("expected a rights refusal, got {other:?}"),
    }
    let after = f.store.package(package.id).expect("read back");
    assert_eq!(after.state, PackageState::Draft, "and it stayed a draft");
    assert!(!after.is_approved());
}

/// Given a package approved while its source was permitted
/// When the note is withdrawn and the bytes are about to leave
/// Then the transition is refused at the boundary, because a verdict is derived
/// and never carried forward from the approval.
#[test]
fn a_note_withdrawn_after_approval_stops_the_upload() {
    let f = fixture();
    let approved = f.approved();
    let ready = f.posting_at(&approved, PublicationState::Ready);

    f.store
        .record_note(
            &SourceNote {
                source: f.source,
                ownership: Ownership::CreatorOwned,
                licence: Licence::Granted {
                    grant: "my own capture".into(),
                },
                consent: Consent::Refused {
                    reason: "the guest withdrew".into(),
                },
                disclosure: String::new(),
                recorded_at: "2026-09-07T03:20:00Z".into(),
                recorded_by: "operator".into(),
            }
            .validated()
            .expect("a complete note validates"),
        )
        .expect("the withdrawal is recorded");

    match f.store.advance_publication(
        ready.id,
        PublicationState::Uploading,
        &Advance::default(),
        &day("2026-09-07"),
        "2026-09-07T03:21:00Z",
    ) {
        Err(PublishError::Rights(report)) => {
            assert_eq!(report.blocking_sources(), vec![f.source]);
        }
        other => panic!("expected a rights refusal, got {other:?}"),
    }
    assert_eq!(
        f.store.posting(ready.id).expect("read back").state,
        PublicationState::Ready,
        "the bytes never left",
    );
}

/// Given a draft
/// When it is created over an artifact id no row holds
/// Then it is refused — a package approving nothing must not succeed.
#[test]
fn a_package_over_a_missing_artifact_is_refused() {
    let f = fixture();
    let mut content = f.content();
    content.artifact_id = ArtifactId::generate();
    let refused = f
        .store
        .create_release_package(NewPackage {
            project_id: f.project,
            content,
            created_at: "2026-09-07T00:00:03Z".into(),
        })
        .expect_err("there is no such artifact");
    assert!(
        matches!(refused, PublishError::GateUnavailable { .. }),
        "{refused:?} is a gate failure, not a package",
    );
}

// ------------------------------------------------------------- the action log

/// Given a posting walked the length of the lifecycle
/// When its log is read
/// Then every transition is one row, the chain is unbroken, `seq` starts at 1,
/// the caller's words are carried verbatim, and `published` is not final.
#[test]
fn the_action_log_records_every_transition_in_order() {
    let f = fixture();
    let approved = f.approved();
    let published = f.posting_at(&approved, PublicationState::Published);

    let log = f.store.publication_events(published.id).expect("log");
    let states: Vec<PublicationState> = log.iter().map(|e| e.to).collect();
    assert_eq!(
        states,
        vec![
            PublicationState::NeedsReview,
            PublicationState::Approved,
            PublicationState::Rendering,
            PublicationState::Ready,
            PublicationState::Uploading,
            PublicationState::Processing,
            PublicationState::Published,
        ],
    );
    for (n, entry) in log.iter().enumerate() {
        assert_eq!(entry.seq, n as i64 + 1, "seq is dense and starts at 1");
        assert_eq!(entry.publication_id, published.id);
        if n == 0 {
            assert_eq!(entry.from, Some(PublicationState::Draft));
        } else {
            assert_eq!(
                entry.from,
                Some(log[n - 1].to),
                "entry {n} does not continue entry {}",
                n - 1,
            );
        }
    }
    assert_eq!(
        log.last().expect("an entry").detail,
        "provider reports it live",
        "the caller's words survive verbatim",
    );
    assert!(
        !PublicationState::Published.is_final(),
        "a takedown is still possible, so published is not final",
    );
    assert!(PublicationState::Removed.is_final());
    assert!(PublicationState::Cancelled.is_final());
}

// --------------------------------------------------------- package identity

/// Given the same content submitted twice
/// When both are fingerprinted
/// Then the fingerprint ignores the caller's ordering of the source set, so a
/// reordered list is not a new package.
#[test]
fn the_fingerprint_ignores_source_order() {
    let f = fixture();
    let mut other = fixtures::source(f.project, None);
    other.media_fingerprint = "sha256:second".into();
    f.store
        .insert_source_asset(&other)
        .expect("source inserted");

    let mut forwards = f.content();
    forwards.sources = vec![f.source, other.id];
    let mut backwards = forwards.clone();
    backwards.sources.reverse();
    assert_eq!(
        forwards.fingerprint(),
        backwards.fingerprint(),
        "the set is the identity, not the order it was built in",
    );
    assert_ne!(
        forwards.fingerprint(),
        f.content_titled("another title").fingerprint(),
        "a different title is a different package",
    );
    assert_eq!(
        forwards.fingerprint(),
        forwards.clone().fingerprint(),
        "re-deriving it is stable",
    );
}
