//! The wall-clock to media-time relationship of a capture, one row per
//! capture part.
//!
//! The segment is a [`strivo_editor::time::ClockSegment`] — the same value
//! the time model converts with — so a row and a mapping answer are the same
//! type rather than two shapes that have to be kept in step. Inserts only,
//! like [`super::store_delivery`]: the reader is the route and the jobs todo
//! 23 and 24 build.

use rusqlite::params;
use strivo_editor::time::ClockSegment;

use super::ids::{CaptureSessionId, ProjectId};
use super::store::Store;
use crate::Result;

/// One capture part's clock segment, with the identity the row needs.
///
/// Grouped into a value rather than five parameters because the identity
/// fields and the segment travel together: a segment with no run and no
/// position in it is not addressable, and a flat signature is how one of them
/// gets left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockSegmentRow {
    pub capture_session_id: CaptureSessionId,
    pub project_id: ProjectId,
    /// The part's position in the run. The unique key on it is what makes a
    /// resumed recording's parts ordered rather than merely present.
    pub seq: u32,
    pub segment: ClockSegment,
}

impl Store {
    /// Record one capture part's clock segment.
    pub fn insert_clock_segment(&self, row: &ClockSegmentRow) -> Result<()> {
        let s = row.segment;
        self.conn.execute(
            "INSERT INTO creator_clock_segments
             (id,capture_session_id,project_id,seq,wall_start_ms,media_start_us,media_len_us,
              rate_num,rate_den)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                uuid::Uuid::new_v4().to_string(),
                row.capture_session_id.to_string(),
                row.project_id.to_string(),
                i64::from(row.seq),
                s.wall_start_ms,
                s.media_start.micros(),
                s.media_len.micros(),
                s.rate_num,
                s.rate_den,
            ],
        )?;
        Ok(())
    }
}
