//! Scoped playback-session supervision.
//!
//! A playback consumer (browser tile, desktop player, etc.) must own only the
//! session it created.  In particular, a stale cleanup callback from an older
//! URL resolution must not cancel a newer session for the same source, and
//! removing one consumer must not affect other sources.  `SessionRegistry`
//! keeps this ownership boundary explicit and is deliberately independent of
//! recording jobs (which are backend-owned).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone)]
pub struct SessionRegistry {
    inner: Arc<Mutex<HashMap<Uuid, Entry>>>,
}

struct Entry {
    generation: u64,
    cancel: CancellationToken,
}

/// A scoped handle to one playback session generation.
#[derive(Clone)]
pub struct SessionLease {
    id: Uuid,
    generation: u64,
    cancel: CancellationToken,
    registry: SessionRegistry,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Start (or replace) a session for a tile/consumer. Replacing a session
    /// cancels only the previous generation for this id.
    pub fn start(&self, id: Uuid) -> SessionLease {
        let mut sessions = self.inner.lock().expect("session registry poisoned");
        let generation = sessions.get(&id).map_or(1, |e| {
            e.cancel.cancel();
            e.generation.saturating_add(1)
        });
        let cancel = CancellationToken::new();
        sessions.insert(
            id,
            Entry {
                generation,
                cancel: cancel.clone(),
            },
        );
        SessionLease {
            id,
            generation,
            cancel,
            registry: self.clone(),
        }
    }

    /// Remove a session only when the caller still owns the current
    /// generation. Late cleanup from an obsolete lease is a no-op.
    pub fn remove(&self, id: Uuid, generation: u64) -> bool {
        let mut sessions = self.inner.lock().expect("session registry poisoned");
        if sessions
            .get(&id)
            .is_some_and(|e| e.generation == generation)
        {
            if let Some(entry) = sessions.remove(&id) {
                entry.cancel.cancel();
            }
            true
        } else {
            false
        }
    }

    pub fn contains(&self, id: Uuid) -> bool {
        self.inner
            .lock()
            .expect("session registry poisoned")
            .contains_key(&id)
    }

    pub fn len(&self) -> usize {
        self.inner.lock().expect("session registry poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner
            .lock()
            .expect("session registry poisoned")
            .is_empty()
    }
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionLease {
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn cancellation(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Explicitly release this lease. Safe to call more than once.
    pub fn release(&self) -> bool {
        self.registry.remove(self.id, self.generation)
    }
}

impl Drop for SessionLease {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{timeout, Duration};

    #[tokio::test]
    async fn removing_one_session_does_not_cancel_another() {
        let registry = SessionRegistry::new();
        let first = registry.start(Uuid::new_v4());
        let second = registry.start(Uuid::new_v4());
        let first_cancel = first.cancellation();
        let second_cancel = second.cancellation();
        first.release();
        assert!(
            timeout(Duration::from_millis(100), first_cancel.cancelled())
                .await
                .is_ok()
        );
        assert!(
            timeout(Duration::from_millis(20), second_cancel.cancelled())
                .await
                .is_err()
        );
        assert_eq!(registry.len(), 1);
    }

    #[tokio::test]
    async fn stale_cleanup_cannot_cancel_replacement_generation() {
        let registry = SessionRegistry::new();
        let id = Uuid::new_v4();
        let old = registry.start(id);
        let old_cancel = old.cancellation();
        let current = registry.start(id);
        let current_cancel = current.cancellation();
        assert!(timeout(Duration::from_millis(100), old_cancel.cancelled())
            .await
            .is_ok());
        assert!(!registry.remove(id, old.generation()));
        assert!(registry.contains(id));
        assert!(
            timeout(Duration::from_millis(20), current_cancel.cancelled())
                .await
                .is_err()
        );
        current.release();
    }
}
