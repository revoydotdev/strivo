//! A clock map written through the typed store, on the unit the time model
//! reads.

use strivo_editor::time::{ClockSegment, MediaTime};

use super::super::entities::{Artifact, CaptureSession, Project, SourceAsset};
use super::super::ids::*;
use super::super::store::Store;
use super::fixtures::at;
use crate::ResearchError;

#[test]
fn a_clock_map_round_trips_through_the_store() {
    let store = Store::open_in_memory().expect("store opens");
    let project = Project {
        id: ProjectId::generate(),
        name: "streamer".into(),
        description: String::new(),
        channel: "twitch".into(),
        series: String::new(),
        settings_json: "{}".into(),
        created_at: at("2026-09-07T00:00:00Z"),
        updated_at: at("2026-09-07T00:00:00Z"),
    };
    store.insert_project(&project).expect("project");
    let source = SourceAsset {
        id: SourceAssetId::generate(),
        project_id: project.id,
        kind: "recording".into(),
        title: "stream".into(),
        media_fingerprint: "sha256:abc".into(),
        locator: None,
        origin: "capture".into(),
        rights_json: "{}".into(),
        track_type: "video".into(),
        codec: "h264".into(),
        duration: Some(MediaTime::from_millis(7_200_000).expect("two hours is a valid duration")),
        created_at: at("2026-09-07T00:00:00Z"),
    };
    store.insert_source_asset(&source).expect("source");
    let session = CaptureSessionId::generate();
    store
        .insert_capture_session(&CaptureSession {
            id: session,
            project_id: project.id,
            source_asset_id: Some(source.id),
            started_at: at("2026-09-07T00:00:00Z"),
            ended_at: Some(at("2026-09-07T02:00:00Z")),
            finalized: true,
            attributes_json: "{}".into(),
            created_at: at("2026-09-07T02:00:00Z"),
        })
        .expect("capture session");

    let hour = MediaTime::from_millis(3_600_000).expect("an hour is valid");
    let row = |seq: u32, wall: i64, media: MediaTime| super::super::ClockSegmentRow {
        capture_session_id: session,
        project_id: project.id,
        seq,
        segment: ClockSegment::exact(wall, media, hour),
    };
    store
        .insert_clock_segment(&row(0, 1_700_000_000_000, MediaTime::ZERO))
        .expect("part one");
    store
        .insert_clock_segment(&row(1, 1_700_003_840_000, hour))
        .expect("part two");

    let read: (i64, i64, i64, i64, i64) = store
        .conn
        .query_row(
            "SELECT wall_start_ms,media_start_us,media_len_us,rate_num,rate_den
             FROM creator_clock_segments WHERE capture_session_id=?1 AND seq=1",
            [session.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .expect("the second segment is readable");
    assert_eq!(
        read,
        (1_700_003_840_000, 3_600_000_000, 3_600_000_000, 1, 1),
        "media time is stored in the canonical microsecond unit"
    );

    let duplicate = store
        .insert_clock_segment(&row(1, 1_700_009_000_000, hour))
        .expect_err("a part cannot occupy a position twice");
    assert!(
        matches!(duplicate, ResearchError::Database(_)),
        "the uniqueness of a part's position is the schema's, got {duplicate:?}"
    );

    // An artifact's duration is the same unit as a source's.
    store
        .insert_artifact(&Artifact {
            id: ArtifactId::generate(),
            project_id: project.id,
            variant_id: None,
            render_plan_id: None,
            kind: "render".into(),
            content_hash: "sha256:art".into(),
            locator: None,
            bytes: Some(10),
            width: Some(1080),
            height: Some(1920),
            duration: Some(hour),
            created_at: at("2026-09-07T02:00:00Z"),
        })
        .expect("artifact");
}
