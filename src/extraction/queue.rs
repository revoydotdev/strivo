//! Bounded, back-pressured intake queue for extraction work.
//!
//! Nothing upstream of [`run_extractor`](super::run_extractor) currently
//! stops a fast producer (capture, ingest, whatever schedules extraction
//! work) from piling up unbounded work for a slower consumer. This module is
//! the fix: [`bounded`] hands back a fixed-capacity queue where
//! [`ExtractionQueue::push`] blocks once the queue is full, instead of
//! growing without bound or dropping work silently. That blocking *is* the
//! back-pressure signal — a producer that keeps pushing is made to wait for
//! the consumer, not the other way around.
//!
//! Draining is not a separate, parallel contract: [`ExtractionConsumer::drain_one`]
//! runs the dequeued [`WorkItem`] through the real [`run_extractor`], into a
//! real [`SignalStore`]. The queue is intake plumbing in front of the
//! existing extractor contract, not a new one.

use std::sync::mpsc::{self, Receiver, SyncSender};

use super::{ExtractionContext, ExtractionError, Extractor, run_extractor};
use crate::signal_store::SignalStore;

/// A unit of extraction work: an extractor paired with the context to run
/// it against. This is what actually moves through the queue — draining an
/// item means running it through [`run_extractor`], not handing back inert
/// data.
pub struct WorkItem {
    extractor: Box<dyn Extractor + Send>,
    ctx: ExtractionContext,
}

impl WorkItem {
    /// Package `extractor` and `ctx` into a work item ready to enqueue.
    pub fn new<E: Extractor + Send + 'static>(extractor: E, ctx: ExtractionContext) -> Self {
        Self {
            extractor: Box::new(extractor),
            ctx,
        }
    }
}

/// The queue is permanently closed: every [`ExtractionConsumer`] handle for
/// it has been dropped, so a push can never be drained.
#[derive(Debug)]
pub struct QueueClosed;

impl std::fmt::Display for QueueClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "extraction queue consumer has been dropped")
    }
}

impl std::error::Error for QueueClosed {}

/// The producer/intake side of a bounded extraction queue.
///
/// [`push`](Self::push) blocks once the queue holds `capacity` undrained
/// items — the entire point of this type. Capacity does not grow past what
/// [`bounded`] was constructed with.
pub struct ExtractionQueue {
    sender: SyncSender<WorkItem>,
}

impl ExtractionQueue {
    /// Enqueue `item`, blocking the calling thread if the queue is
    /// currently at capacity until the consumer drains a slot.
    ///
    /// Returns [`QueueClosed`] if every [`ExtractionConsumer`] for this
    /// queue has already been dropped.
    pub fn push(&self, item: WorkItem) -> Result<(), QueueClosed> {
        self.sender.send(item).map_err(|_| QueueClosed)
    }
}

/// The consumer/drain side of a bounded extraction queue.
pub struct ExtractionConsumer {
    receiver: Receiver<WorkItem>,
}

impl ExtractionConsumer {
    /// Block until a [`WorkItem`] is available, then run it through
    /// [`run_extractor`] against `store`.
    ///
    /// Returns `None` once every [`ExtractionQueue`] handle has been
    /// dropped and no item is left to drain. A `Some` result carries
    /// whatever [`run_extractor`] itself returned — draining does not
    /// swallow extraction errors.
    pub fn drain_one(&self, store: &SignalStore) -> Option<Result<Vec<i64>, ExtractionError>> {
        let item = self.receiver.recv().ok()?;
        Some(run_extractor(item.extractor.as_ref(), &item.ctx, store))
    }
}

/// Construct a bounded extraction queue with room for `capacity` undrained
/// items, returning the intake ([`ExtractionQueue`]) and drain
/// ([`ExtractionConsumer`]) halves.
///
/// `capacity == 0` is a valid rendezvous queue: every push blocks until a
/// matching drain is ready for it.
pub fn bounded(capacity: usize) -> (ExtractionQueue, ExtractionConsumer) {
    let (sender, receiver) = mpsc::sync_channel(capacity);
    (ExtractionQueue { sender }, ExtractionConsumer { receiver })
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;

    use serde_json::json;

    use super::{WorkItem, bounded};
    use crate::extraction::ExtractionContext;
    use crate::extraction::events::{EventExtractor, TimecodedEvent};
    use crate::signal_store::{SignalQuery, SignalStore};

    fn detection(kind: &str, label: &str) -> TimecodedEvent {
        TimecodedEvent {
            t_start: 1.0,
            t_end: 2.0,
            kind: kind.to_string(),
            label: label.to_string(),
            payload: json!({"kind": kind}),
            confidence: 0.9,
        }
    }

    fn work_item(recording_id: &str) -> WorkItem {
        let extractor = EventExtractor::new("backpressure-extractor", vec![detection("score", "goal")]);
        let ctx = ExtractionContext {
            recording_id: recording_id.to_string(),
        };
        WorkItem::new(extractor, ctx)
    }

    /// Proves the bound is real: a producer pushing past capacity must
    /// block until the consumer drains, not return early or let the queue
    /// grow past capacity.
    ///
    /// Deterministic by construction, not by timing: the producer only
    /// acks a push over `pushed_tx` *after* `ExtractionQueue::push` itself
    /// returns. `sync_channel`'s bounded semantics mean the 3rd push
    /// physically cannot return before a slot is freed, so observing no ack
    /// for it is not a race against scheduling — it is guaranteed by the
    /// channel contract as long as the consumer has not drained yet, which
    /// this test controls explicitly.
    #[test]
    fn extractor_backpressure_blocks_producer_until_consumer_drains() {
        let (queue, consumer) = bounded(2);
        let store = SignalStore::open_in_memory().expect("in-memory store should open");

        let (pushed_tx, pushed_rx) = mpsc::channel::<usize>();

        let producer = thread::spawn(move || {
            for i in 0..3 {
                queue
                    .push(work_item(&format!("rec-{i}")))
                    .expect("push should succeed while consumer is alive");
                pushed_tx.send(i).expect("ack channel should be open");
            }
        });

        // The first two pushes fit inside capacity 2 and must complete
        // without any drain happening.
        assert_eq!(pushed_rx.recv().expect("ack for push 0"), 0);
        assert_eq!(pushed_rx.recv().expect("ack for push 1"), 1);

        // The queue now holds 2 undrained items -- exactly its capacity.
        // The 3rd push cannot have acked: it cannot return until this test
        // drains a slot, which it has deliberately not done yet.
        assert!(
            pushed_rx.try_recv().is_err(),
            "producer must be blocked on the 3rd push while the queue is full and undrained"
        );

        // Draining one item frees a slot and unblocks the producer's
        // pending push.
        consumer
            .drain_one(&store)
            .expect("an item should be available to drain")
            .expect("drained item should run through run_extractor successfully");

        assert_eq!(
            pushed_rx.recv().expect("ack for push 2"),
            2,
            "the blocked push must complete only once the consumer has drained a slot"
        );

        producer.join().expect("producer thread must not panic");
    }

    /// Proves the wiring is real, not plumbing: an item pushed through the
    /// queue and drained via `ExtractionConsumer::drain_one` actually runs
    /// through `run_extractor` and lands in a real `SignalStore`, with
    /// fields intact.
    #[test]
    fn extractor_backpressure_drained_items_persist_through_run_extractor() {
        let (queue, consumer) = bounded(4);
        let store = SignalStore::open_in_memory().expect("in-memory store should open");

        queue
            .push(work_item("rec-backpressure-e2e"))
            .expect("push should succeed");

        let ids = consumer
            .drain_one(&store)
            .expect("an item should be available to drain")
            .expect("drained item should run through run_extractor successfully");
        assert_eq!(ids.len(), 1, "one row id for the one queued detection");

        let rows = store
            .query_signals(&SignalQuery::new().recording_id("rec-backpressure-e2e"))
            .expect("query should succeed");
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.source_plugin, "backpressure-extractor");
        assert_eq!(row.kind, "event:score");
        assert_eq!(row.label, "goal");
        assert_eq!(row.t_start, 1.0);
        assert_eq!(row.t_end, 2.0);
        assert_eq!(row.confidence, 0.9);
        assert_eq!(row.payload, json!({"kind": "score"}));
    }
}
