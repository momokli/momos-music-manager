//! In-memory ring buffer for telemetry events.
//!
//! Bounded FIFO queue: when the capacity is reached, the **oldest** event is
//! dropped (with a warning) to make room for the newest one. This protects the
//! app from unbounded memory growth when the flusher cannot keep up (e.g.
//! prolonged offline periods) while keeping the most recent events.

use std::collections::VecDeque;

use tracing::warn;

use super::events::{MAX_BATCH_EVENTS, TelemetryEvent};

/// Default capacity (10k events, per the concept doc).
pub const DEFAULT_CAPACITY: usize = 10_000;

/// Warn at most once per this many dropped events to avoid log spam.
const DROP_WARN_EVERY: u64 = 1000;

/// A bounded FIFO ring buffer of [`TelemetryEvent`]s.
#[derive(Debug)]
pub struct EventRingBuffer {
    events: VecDeque<TelemetryEvent>,
    capacity: usize,
    /// Total number of events dropped (oldest evicted) so far.
    dropped_total: u64,
    /// Events dropped since the last warning.
    dropped_since_warn: u64,
}

impl EventRingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            events: VecDeque::with_capacity(capacity.min(MAX_BATCH_EVENTS).max(16)),
            capacity,
            dropped_total: 0,
            dropped_since_warn: 0,
        }
    }

    /// Push an event, evicting the oldest one when full.
    pub fn push(&mut self, event: TelemetryEvent) {
        if self.events.len() == self.capacity {
            self.events.pop_front();
            self.dropped_total += 1;
            self.dropped_since_warn += 1;
            if self.dropped_since_warn >= DROP_WARN_EVERY
                || self.dropped_since_warn == 1
            {
                warn!(
                    "telemetry ring buffer full ({} events) — dropping oldest event \
                     (total dropped: {})",
                    self.capacity, self.dropped_total
                );
                self.dropped_since_warn = 0;
            }
        }
        self.events.push_back(event);
    }

    /// Drain up to `max_events` events (respecting a soft byte budget) from
    /// the front — the next flush batch. Returns them in FIFO order.
    pub fn drain_batch(&mut self, max_events: usize, max_bytes: usize) -> Vec<TelemetryEvent> {
        let mut batch = Vec::new();
        let mut bytes = 0usize;
        while batch.len() < max_events && bytes < max_bytes {
            let Some(event) = self.events.pop_front() else {
                break;
            };
            bytes += serde_json::to_vec(&event).map(|b| b.len()).unwrap_or(64);
            batch.push(event);
        }
        batch
    }

    /// Prepend events back to the front (failed flush → retry later, FIFO
    /// order preserved).
    pub fn prepend(&mut self, events: Vec<TelemetryEvent>) {
        for event in events.into_iter().rev() {
            self.events.push_front(event);
        }
    }

    /// Remove the given event ids (used after a batch was dropped as 4xx).
    pub fn remove_ids(&mut self, ids: &[String]) {
        self.events.retain(|e| !ids.contains(&e.event_id));
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Total events dropped (oldest evictions) over the buffer's lifetime.
    pub fn dropped_total(&self) -> u64 {
        self.dropped_total
    }

    pub fn iter(&self) -> impl Iterator<Item = &TelemetryEvent> {
        self.events.iter()
    }

    /// Replace contents (used to load the spool at startup).
    pub fn extend_from_front(&mut self, events: Vec<TelemetryEvent>) {
        for event in events {
            self.push(event);
        }
    }
}

impl Default for EventRingBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::events::EventType;

    fn ev(id: &str) -> TelemetryEvent {
        let mut e = TelemetryEvent::new(EventType::TaskCompleted, serde_json::json!({}));
        e.event_id = id.to_string();
        e
    }

    #[test]
    fn push_and_drain_fifo() {
        let mut buf = EventRingBuffer::new(4);
        buf.push(ev("a"));
        buf.push(ev("b"));
        buf.push(ev("c"));
        let batch = buf.drain_batch(10, usize::MAX);
        let ids: Vec<_> = batch.iter().map(|e| e.event_id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert!(buf.is_empty());
    }

    #[test]
    fn drop_oldest_when_full() {
        let mut buf = EventRingBuffer::new(2);
        buf.push(ev("a"));
        buf.push(ev("b"));
        buf.push(ev("c")); // evicts "a"
        let ids: Vec<_> = buf.iter().map(|e| e.event_id.as_str()).collect();
        assert_eq!(ids, ["b", "c"]);
        assert_eq!(buf.dropped_total(), 1);
    }

    #[test]
    fn drain_batch_respects_event_limit() {
        let mut buf = EventRingBuffer::new(MAX_BATCH_EVENTS + 10);
        for i in 0..MAX_BATCH_EVENTS {
            buf.push(ev(&format!("e{i}")));
        }
        let batch = buf.drain_batch(MAX_BATCH_EVENTS - 1, usize::MAX);
        assert_eq!(batch.len(), MAX_BATCH_EVENTS - 1);
        assert_eq!(buf.len(), 1);
    }

    #[test]
    fn drain_batch_takes_all_when_below_limit() {
        let mut buf = EventRingBuffer::new(10);
        for i in 0..5 {
            buf.push(ev(&format!("e{i}")));
        }
        let batch = buf.drain_batch(10, usize::MAX);
        assert_eq!(batch.len(), 5);
        assert!(buf.is_empty());
    }

    #[test]
    fn drain_batch_respects_byte_budget() {
        let mut buf = EventRingBuffer::new(10);
        for i in 0..10 {
            buf.push(ev(&format!("e{i}")));
        }
        let batch = buf.drain_batch(10, 1); // 1 byte budget → at most one event
        assert!(batch.len() <= 1, "batch too large: {}", batch.len());
        assert!(!buf.is_empty());
    }

    #[test]
    fn prepend_restores_fifo_order() {
        let mut buf = EventRingBuffer::new(10);
        buf.push(ev("a"));
        let batch = buf.drain_batch(10, usize::MAX);
        assert!(buf.is_empty());
        buf.prepend(batch);
        buf.push(ev("b"));
        let ids: Vec<_> = buf.iter().map(|e| e.event_id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn remove_ids_drops_matching() {
        let mut buf = EventRingBuffer::new(10);
        buf.push(ev("a"));
        buf.push(ev("b"));
        buf.push(ev("c"));
        buf.remove_ids(&["b".to_string()]);
        let ids: Vec<_> = buf.iter().map(|e| e.event_id.as_str()).collect();
        assert_eq!(ids, ["a", "c"]);
    }

    #[test]
    fn max_bytes_constant_is_sane() {
        // Wire sanity: batch building uses these caps, keep them in sync with
        // the ring buffer defaults.
        assert_eq!(super::super::events::MAX_BATCH_EVENTS, 200);
        assert_eq!(super::super::events::MAX_BATCH_BYTES, 1_048_576);
    }
}
