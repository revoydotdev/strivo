//! One render attempt's identity, and the two paths that identity owns.
//!
//! Before this module the executor named its scratch directory after the
//! **subject** and the render route named its output after the **recording**.
//! Both are the wrong key: two attempts at the same subject are the same
//! directory and the same filename, so a second render overwrote the first's
//! per-cut sub-clips mid-flight and the first to finish deleted the second's
//! working directory out from under it. Measured, not inferred — see
//! `tests::isolation::two_concurrent_exports_of_one_revision_both_survive`,
//! which reproduced the collision on five runs out of five.
//!
//! The key is the **attempt**, and an attempt is minted per [`super::export`]
//! call. A subject appears in both paths for legibility; it decides nothing.
//!
//! The id is a process id, a nanosecond reading and a process-local counter.
//! No dependency: `std::process::id` separates processes, the clock separates a
//! recycled pid from its predecessor, and the counter separates two calls in
//! the same process inside one clock tick. All three components are digits and
//! dashes, so the id is a legal filename stem on every platform this daemon
//! runs on.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::RenderError;

static NEXT: AtomicU64 = AtomicU64::new(0);

/// One render attempt. Minted by [`AttemptId::mint`], never constructed from
/// caller input, so a caller cannot name another attempt's files.
///
/// `Serialize` but deliberately not `Deserialize`: an attempt id travels out to
/// the caller as the address of an artifact, and it travels back in as a string
/// the caller displays. Deserialising one would let a caller assert an identity
/// it never minted, which is the hole [`CacheKey::from_raw`](super::CacheKey)
/// keeps crate-internal in todo 24 for the same reason.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
pub struct AttemptId(String);

impl AttemptId {
    #[must_use]
    pub fn mint() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        Self(format!(
            "{nanos:x}-{:x}-{:x}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AttemptId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// This attempt's scratch directory: where the per-cut sub-renders land.
#[must_use]
pub fn scratch_dir(parent: &Path, subject: &str, attempt: &AttemptId) -> PathBuf {
    parent.join(format!(".plan-{}-{attempt}", scrub(subject)))
}

/// This attempt's output: a fresh path every time, so a render can never land
/// on a previous attempt's bytes.
#[must_use]
pub fn output_path(parent: &Path, subject: &str, attempt: &AttemptId, extension: &str) -> PathBuf {
    parent.join(format!("{}-{attempt}.{extension}", scrub(subject)))
}

/// Removes the scratch directory on drop, including on an early `?` return or
/// a panic. One directory, one attempt, one owner — which is what makes it safe
/// for two attempts to exist at once.
pub struct Scratch(pub PathBuf);

impl std::ops::Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The artifact gate: a tool that exited zero has said nothing about whether it
/// wrote a file.
///
/// A missing output means the tool reported success over nothing, which is the
/// shape a substituted or stubbed renderer leaves. A zero-byte output means the
/// muxer created a container and put no stream in it — not a render, and its
/// size is indistinguishable from a real artifact's until someone plays it.
/// Both are failures, and both are refused here rather than counted as bytes.
pub fn validate_output(path: &Path) -> Result<u64, RenderError> {
    let meta = std::fs::metadata(path).map_err(|e| RenderError::MissingOutput {
        path: path.to_path_buf(),
        detail: e.to_string(),
    })?;
    if meta.len() == 0 {
        return Err(RenderError::EmptyOutput {
            path: path.to_path_buf(),
        });
    }
    Ok(meta.len())
}

/// A filename-safe stem. A recording id is already filename-safe, but a
/// subject may be a project id, and a path separator in a scratch or output
/// path would write outside the directory it is supposed to stay in.
fn scrub(subject: &str) -> String {
    subject
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
