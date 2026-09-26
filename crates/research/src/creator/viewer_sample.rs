//! The viewer sample: one provider poll, placed on the source clock.
//!
//! **A viewer sample is a wall-clock observation, and the two clocks are not
//! the same thing.** The value comes from a poll that happened at some
//! wall-clock instant; where that instant sits on the media timeline is a
//! question only the [`ClockMap`](strivo_editor::time::ClockMap) can answer,
//! and the answer carries an uncertainty bound. So a sample is built by
//! [`ViewerSample::observe`], which takes the map and cannot be handed a
//! position by a caller. There is no constructor that accepts a
//! [`MediaTime`](strivo_editor::time::MediaTime) directly, because that is
//! how a fabricated precision gets in.

use strivo_editor::time::{ClockMap, MediaTime};

use super::ids::{CaptureSessionId, ProjectId};
use super::observation_error::ObservationError;
use super::observations::{place, CaptureWindow, SamplePosition};

/// The store's own result type, so a refusal stays typed to the caller.
type Result<T> = std::result::Result<T, ObservationError>;

/// One row as SQLite hands it back, in column order. Named so the read and
/// the rebuild agree by construction rather than by two matching signatures.
pub(super) type SampleRow = (i64, Option<i64>, i64, Option<i64>, i64, Option<i64>, i64);

/// One viewer reading, as it arrived from the provider: a count, the instant
/// the poll completed, and the window the poll covered.
///
/// The count is [`i64`], not `u32`, on purpose. A provider reporting `-1`
/// is a real malformed input, and the honest response is a typed refusal
/// ([`ObservationError::NegativeValue`]) rather than an `as u32` cast that
/// turns it into 4 294 967 295 viewers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawViewerSample {
    pub wall_ms: i64,
    pub value: i64,
    /// How long the poll's window was. A value read by a poll that ran every
    /// 5 s describes the preceding 5 s; without this the position would be
    /// claimed to microsecond precision it was never measured to.
    pub poll_interval_ms: i64,
    /// Whether the provider itself reported this poll as dropped. A dropped
    /// poll is still recorded — it is evidence about the provider — but it
    /// contributes no coverage.
    pub dropped: bool,
}

/// One persisted viewer sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewerSample {
    pub capture_session_id: CaptureSessionId,
    pub project_id: ProjectId,
    /// Assigned by the store, never by the caller: a resumed capture takes
    /// the next one, so a resume cannot collide with a batch it never saw
    /// committed.
    pub seq: u32,
    pub wall_ms: i64,
    pub position: SamplePosition,
    pub value: u32,
    pub poll_interval_ms: i64,
    pub dropped: bool,
}

impl ViewerSample {
    /// Read one provider poll and place it on the source clock.
    ///
    /// Takes the window and the map rather than a position, so a caller
    /// cannot assert a coordinate it never derived.
    pub fn observe(window: &CaptureWindow, clock: &ClockMap, raw: RawViewerSample) -> Result<Self> {
        if raw.poll_interval_ms <= 0 {
            return Err(ObservationError::BadPollInterval {
                millis: raw.poll_interval_ms,
            });
        }
        let position = place(window, clock, raw.wall_ms, raw.poll_interval_ms)?;
        Ok(Self {
            capture_session_id: CaptureSessionId::generate(),
            project_id: ProjectId::generate(),
            seq: 0,
            wall_ms: raw.wall_ms,
            position,
            value: u32::try_from(raw.value)
                .map_err(|_| ObservationError::NegativeValue { viewers: raw.value })?,
            poll_interval_ms: raw.poll_interval_ms,
            dropped: raw.dropped,
        })
    }

    /// The instant this reading describes, as a media coordinate, refusing
    /// the ones the map does not cover. A convenience over [`Self::observe`]
    /// for a caller that has a bare wall clock and a map.
    pub fn media_position(
        window: &CaptureWindow,
        clock: &ClockMap,
        raw: &RawViewerSample,
    ) -> Result<SamplePosition> {
        if raw.poll_interval_ms <= 0 {
            return Err(ObservationError::BadPollInterval {
                millis: raw.poll_interval_ms,
            });
        }
        place(window, clock, raw.wall_ms, raw.poll_interval_ms)
    }

    /// Rebuild a sample from its stored columns.
    ///
    /// **This converts and nothing more.** A NULL wall clock, a NULL bound, a
    /// non-positive poll interval and a negative count are all impossible
    /// here, because `schema_v5.sql` declares those columns `NOT NULL` with
    /// `CHECK` constraints and those are the enforcing mechanism — proven
    /// rather than assumed by three tests that force the bad rows in past
    /// every writer. A mutation probe replacing one of the checks below with a
    /// default turned *no* test red, which is how an unreachable safeguard
    /// announces itself; they are gone rather than kept as a comfort.
    ///
    /// What remains is the two conversions the schema genuinely cannot
    /// express: `seq` and `viewer_count` are `CHECK(>= 0)`, which admits a
    /// value past `u32::MAX`. Those are refused, never truncated, because a
    /// count that does not fit is not a smaller count.
    ///
    /// `project_id` is not part of a sample's identity — `(capture_session_id,
    /// seq)` is — so it is not read back; a caller that needs it queries by
    /// session.
    pub(super) fn from_row(session: CaptureSessionId, row: SampleRow) -> Result<Self> {
        const TABLE: &str = "creator_viewer_samples";
        // Read the NOT NULL columns as plain `i64`. They arrive as `Option`
        // only because `get` is generic, and unwrapping that here would be
        // three unreachable branches standing in for a guarantee the column
        // already makes.
        let (
            seq,
            Some(wall_ms),
            media_us,
            Some(uncertainty_us),
            count,
            Some(poll_interval_ms),
            dropped,
        ) = row
        else {
            return Err(ObservationError::UnreadableRow {
                table: TABLE,
                detail: "a NOT NULL column read as NULL: the schema and the row disagree".into(),
            });
        };
        Ok(Self {
            capture_session_id: session,
            project_id: ProjectId::generate(),
            seq: u32::try_from(seq).map_err(|_| ObservationError::UnreadableRow {
                table: TABLE,
                detail: format!("seq {seq} is negative"),
            })?,
            wall_ms,
            position: SamplePosition {
                media: MediaTime::from_micros(media_us).map_err(|e| {
                    ObservationError::UnreadableRow {
                        table: TABLE,
                        detail: format!("media_us: {e}"),
                    }
                })?,
                uncertainty_micros: uncertainty_us,
            },
            value: u32::try_from(count).map_err(|_| ObservationError::UnreadableRow {
                table: TABLE,
                detail: format!("viewer_count {count} is negative"),
            })?,
            poll_interval_ms,
            dropped: dropped != 0,
        })
    }
}
