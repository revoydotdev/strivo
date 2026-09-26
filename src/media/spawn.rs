//! Every media child process this module starts, recorded, plus the two-slot
//! bound the interaction paths acquire it under.
//!
//! The record is not a debug aid. Report R02's `Must NOT` — *no interaction
//! path spawns an original-resolution decode* — is a claim about what actually
//! ran, and an assertion about the code's shape cannot establish it: a second
//! spawn site added later reads identically to the first. So the spawn itself
//! is the observation point, and [`snapshot`] is the only way to read it.
//!
//! Two properties make the observation falsifiable rather than decorative:
//!
//! * **Always on.** There is no flag to forget to set, so a test cannot
//!   accidentally pass by recording nothing. [`snapshot`] returning an empty
//!   vector is the failure a caller must check, and the tests do check it.
//! * **Bounded.** [`SPAWN_LOG_CAPACITY`] entries, oldest dropped. A log that
//!   grows for the life of a daemon is the same unbounded-cache defect this
//!   todo exists to remove, and it would be silly to add one to police it.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Entries retained by the spawn record. 256 is far more than any test needs
/// and short enough that a long daemon run cannot accumulate history.
pub const SPAWN_LOG_CAPACITY: usize = 256;

/// Concurrent interactive media child processes. This is the *same* bound
/// `docs/PERFORMANCE.md` states for "Concurrent interactive media
/// probes/remuxes" and that `crates/strivo-web/src/server.rs:99` enforces for
/// `probe_slots`; it is restated here rather than re-derived, because a second
/// number would be a second claim.
pub const MEDIA_SLOTS: usize = 2;

/// How long an interaction path waits for a slot. `docs/PERFORMANCE.md` states
/// 30 s for probe and thumbnail attempts (`routes/settings.rs:1308`).
///
/// The web layer's own acquisitions are unbounded waits
/// (`routes/settings.rs:325`), safe only because the request context bounds
/// them. An interaction path has no such backstop, so this one is bounded at
/// the acquisition site — the failure todo 37 recorded about the existing
/// sites, fixed here rather than restated.
pub const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(30);

/// What one child process was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spawn {
    /// `ffmpeg` or `ffprobe`.
    pub tool: &'static str,
    /// What this module was doing: `proxy-build`, `proxy-index`,
    /// `proxy-frame`, `pyramid-build`, `filmstrip-build`.
    pub purpose: &'static str,
    /// The path handed to the child as its input.
    pub input: String,
    /// Whether that input is a generated proxy rather than the user's source.
    /// The `Must NOT` is exactly the assertion that this is `true` for every
    /// spawn an interaction path makes.
    pub input_is_proxy: bool,
    /// The picture size the child was told to decode, when it decodes picture
    /// at all. `0` means the stream is read or copied without decoding.
    pub width: u32,
    pub height: u32,
    /// `true` when the child decodes picture frames rather than only reading
    /// packet headers.
    pub decodes_picture: bool,
}

impl Spawn {
    /// Whether this spawn would decode at the size of the user's own source.
    /// A caller asserting "no original-resolution decode" asserts this is
    /// `false` for every recorded spawn.
    pub fn is_original_resolution(&self) -> bool {
        self.decodes_picture && !self.input_is_proxy
    }
}

fn log() -> &'static Mutex<VecDeque<Spawn>> {
    static LOG: OnceLock<Mutex<VecDeque<Spawn>>> = OnceLock::new();
    LOG.get_or_init(|| Mutex::new(VecDeque::with_capacity(SPAWN_LOG_CAPACITY)))
}

fn slots() -> &'static Arc<Semaphore> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(Semaphore::new(MEDIA_SLOTS)))
}

/// Record one child process. Called by the spawn helpers in this module, not
/// by callers: a record a caller can forget to make proves nothing.
pub(crate) fn record(spawn: Spawn) {
    if let Ok(mut log) = log().lock() {
        if log.len() == SPAWN_LOG_CAPACITY {
            log.pop_front();
        }
        log.push_back(spawn);
    }
}

/// Everything recorded so far, oldest first. A caller that wants "what did
/// this interaction do" clears first and reads the difference.
pub fn snapshot() -> Vec<Spawn> {
    log()
        .lock()
        .map(|l| l.iter().cloned().collect())
        .unwrap_or_default()
}

/// Forget the record. Tests call this so a shared process does not let one
/// test's spawns satisfy another's assertion.
pub fn clear() {
    if let Ok(mut log) = log().lock() {
        log.clear();
    }
}

/// Take one of the [`MEDIA_SLOTS`] permits, waiting at most [`ACQUIRE_TIMEOUT`].
///
/// The error names the bound that refused it, so a stalled interaction is
/// diagnosable from the message alone.
pub async fn acquire_slot(purpose: &'static str) -> Result<OwnedSemaphorePermit, String> {
    acquire_slot_within(purpose, ACQUIRE_TIMEOUT).await
}

/// [`acquire_slot`] with the wait spelled out, so a test can exercise the
/// refusal without spending the production deadline.
pub async fn acquire_slot_within(
    purpose: &'static str,
    wait: std::time::Duration,
) -> Result<OwnedSemaphorePermit, String> {
    let pool = Arc::clone(slots());
    match tokio::time::timeout(wait, pool.acquire_owned()).await {
        Ok(Ok(permit)) => Ok(permit),
        Ok(Err(_)) => Err(format!("the {MEDIA_SLOTS}-slot media pool is closed")),
        Err(_) => Err(format!(
            "{purpose}: no media slot within {wait:?} \
             ({MEDIA_SLOTS} slots, docs/PERFORMANCE.md)"
        )),
    }
}

/// The number of slots currently free. A test asserting the bound is real
/// reads this while every permit is held.
pub fn free_slots() -> usize {
    slots().available_permits()
}

/// Spawn `ffmpeg` with `args`, recording the spawn first so a failed child is
/// still visible. `kill_on_drop` so an abandoned future does not leave a child
/// decoding a six-hour source forever.
pub(crate) async fn ffmpeg(
    purpose: &'static str,
    input: &Path,
    input_is_proxy: bool,
    width: u32,
    height: u32,
    decodes_picture: bool,
    args: &[&std::ffi::OsStr],
) -> anyhow::Result<std::process::ExitStatus> {
    record(Spawn {
        tool: "ffmpeg",
        purpose,
        input: input.to_string_lossy().into_owned(),
        input_is_proxy,
        width,
        height,
        decodes_picture,
    });
    let mut cmd = tokio::process::Command::new(crate::tools::resolve_tool("ffmpeg"));
    cmd.kill_on_drop(true);
    cmd.arg("-nostdin").args(args);
    Ok(cmd.status().await?)
}

/// Spawn `ffprobe` to read container or packet headers. It decodes no picture,
/// so it is recorded with `decodes_picture: false` and does not narrow the
/// `Must NOT` — but it is recorded, because a probe pointed at a user's source
/// is still a child process an interaction path started.
pub(crate) async fn ffprobe(
    purpose: &'static str,
    input: &Path,
    input_is_proxy: bool,
    args: &[&std::ffi::OsStr],
) -> anyhow::Result<std::process::Output> {
    record(Spawn {
        tool: "ffprobe",
        purpose,
        input: input.to_string_lossy().into_owned(),
        input_is_proxy,
        width: 0,
        height: 0,
        decodes_picture: false,
    });
    let mut cmd = tokio::process::Command::new(crate::tools::resolve_tool("ffprobe"));
    cmd.kill_on_drop(true);
    cmd.args(args);
    Ok(cmd.output().await?)
}
