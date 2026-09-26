//! Shared fixtures. Builders rather than constructors so a test only states
//! the fields its assertion is about.

#![allow(dead_code)]

use std::path::PathBuf;

use strivo_editor::time::MediaTime;

use super::super::entities::*;
use super::super::ids::*;

pub fn at(ts: &str) -> String {
    ts.to_string()
}

/// A duration in whole milliseconds, which is how a caller states one; the
/// fixture's own inputs stay readable in the unit a person thinks in and the
/// conversion happens once, here.
pub fn dur(millis: i64) -> MediaTime {
    MediaTime::from_millis(millis).expect("a non-negative duration")
}

pub fn project(name: &str) -> Project {
    Project {
        id: ProjectId::generate(),
        name: name.into(),
        description: String::new(),
        channel: "twitch".into(),
        series: String::new(),
        settings_json: "{}".into(),
        created_at: at("2026-09-07T00:00:00Z"),
        updated_at: at("2026-09-07T00:00:00Z"),
    }
}

pub fn source(project_id: ProjectId, locator: Option<PathBuf>) -> SourceAsset {
    SourceAsset {
        id: SourceAssetId::generate(),
        project_id,
        kind: "recording".into(),
        title: "stream".into(),
        media_fingerprint: "sha256:abc".into(),
        locator,
        origin: "capture".into(),
        rights_json: "{}".into(),
        track_type: "video".into(),
        codec: "h264".into(),
        duration: Some(dur(7_200_000)),
        created_at: at("2026-09-07T00:00:00Z"),
    }
}

pub fn revision(
    project_id: ProjectId,
    n: u32,
    parent: Option<CompositionRevisionId>,
) -> CompositionRevision {
    CompositionRevision {
        id: CompositionRevisionId::generate(),
        project_id,
        parent_revision_id: parent,
        revision: n,
        author: "operator".into(),
        timeline_json: format!("{{\"rev\":{n}}}"),
        output_profile: "long-form".into(),
        // Distinct per revision: the list is ordered by (created_at, id), and
        // a tie would fall back to the random id and make this test flaky.
        created_at: format!("2026-09-07T00:00:0{n}Z"),
    }
}
