//! Lock-free single-producer/single-consumer event queue.
//!
//! The capture threads (window event loop, MIDI callback) own the
//! [`EventProducer`]; the audio callback owns the [`EventConsumer`]. The queue
//! is allocated once at startup, never locks, and never allocates on the push
//! or pop path, which keeps it safe for the real-time thread.
//!
//! When the queue is full the producer drops the newest event and increments a
//! diagnostic counter instead of blocking. A dropped hit is better than a
//! stalled MIDI callback or window loop.

use std::cell::{Cell, UnsafeCell};
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use super::{InputEvent, ReleaseEvent, TriggerEvent};

/// Why a push failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PushError {
    /// The consumer has not caught up; the event was dropped.
    #[error("input event queue is full")]
    Full,
}

/// Create a queue with `capacity` slots.
///
/// Capacity must be a power of two no smaller than 2. 1024 events is roughly a
/// second of furious drumming and costs a few tens of kilobytes.
///
/// # Panics
///
/// Panics when `capacity` is not a power of two or is below 2.
#[must_use]
pub fn event_queue(capacity: usize) -> (EventProducer, EventConsumer) {
    assert!(
        capacity.is_power_of_two() && capacity >= 2,
        "queue capacity must be a power of two >= 2"
    );
    let buffer = (0..capacity)
        .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let inner = Arc::new(Ring {
        buffer,
        capacity,
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
        dropped: AtomicU64::new(0),
    });
    (
        EventProducer {
            inner: Arc::clone(&inner),
            _not_sync: PhantomData,
        },
        EventConsumer {
            inner,
            _not_sync: PhantomData,
        },
    )
}

struct Ring {
    buffer: Box<[UnsafeCell<MaybeUninit<InputEvent>>]>,
    capacity: usize,
    /// Consumer position (monotonic; masked for indexing).
    head: AtomicUsize,
    /// Producer position (monotonic; masked for indexing).
    tail: AtomicUsize,
    dropped: AtomicU64,
}

// SAFETY: access to each slot is split by the head/tail protocol: the producer
// only writes the slot at `tail` and publishes it with a release store; the
// consumer only reads the slot at `head` after observing that release store,
// then publishes with a release store. `InputEvent` is `Send`.
unsafe impl Send for Ring {}
unsafe impl Sync for Ring {}

impl Ring {
    fn mask(&self, position: usize) -> usize {
        position & (self.capacity - 1)
    }

    fn push(&self, event: InputEvent) -> Result<(), PushError> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail.wrapping_sub(head) >= self.capacity {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return Err(PushError::Full);
        }
        let slot = self.mask(tail);
        // SAFETY: the producer is the only writer of this slot, and the slot is
        // uninitialized or already consumed because the queue is not full.
        unsafe {
            (*self.buffer[slot].get()).write(event);
        }
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    fn pop(&self) -> Option<InputEvent> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head == tail {
            return None;
        }
        let slot = self.mask(head);
        // SAFETY: the producer published this slot before advancing `tail`, and
        // the consumer is the only reader.
        let event = unsafe { (*self.buffer[slot].get()).assume_init_read() };
        self.head.store(head.wrapping_add(1), Ordering::Release);
        Some(event)
    }

    fn len(&self) -> usize {
        let tail = self.tail.load(Ordering::Acquire);
        let head = self.head.load(Ordering::Acquire);
        tail.wrapping_sub(head).min(self.capacity)
    }
}

impl Drop for Ring {
    fn drop(&mut self) {
        let head = *self.head.get_mut();
        let tail = *self.tail.get_mut();
        for position in head..tail {
            let slot = self.mask(position);
            // SAFETY: positions in `head..tail` were initialized and have no
            // other readers; `Arc` guarantees no producer or consumer is alive.
            unsafe {
                (*self.buffer[slot].get()).assume_init_drop();
            }
        }
    }
}

/// Capture-side handle. One per producer thread.
pub struct EventProducer {
    inner: Arc<Ring>,
    // Producers and consumers are single-owner; prevent accidental sharing.
    _not_sync: PhantomData<Cell<()>>,
}

impl EventProducer {
    /// Push an event, dropping it when the queue is full.
    pub fn push(&self, event: impl Into<InputEvent>) -> Result<(), PushError> {
        self.inner.push(event.into())
    }

    /// Push a trigger event.
    pub fn push_trigger(&self, event: TriggerEvent) -> Result<(), PushError> {
        self.inner.push(InputEvent::Trigger(event))
    }

    /// Push a release event.
    pub fn push_release(&self, event: ReleaseEvent) -> Result<(), PushError> {
        self.inner.push(InputEvent::Release(event))
    }

    /// Number of events dropped since startup because the queue was full.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.inner.dropped.load(Ordering::Relaxed)
    }

    /// Configured slot count.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.inner.capacity
    }

    /// Approximate queued events; only the consumer can observe an exact value.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Approximate emptiness check.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Audio-side handle. One per consumer thread.
pub struct EventConsumer {
    inner: Arc<Ring>,
    _not_sync: PhantomData<Cell<()>>,
}

impl EventConsumer {
    /// Pop the oldest event, if any.
    #[must_use]
    pub fn pop(&self) -> Option<InputEvent> {
        self.inner.pop()
    }

    /// Number of events dropped by the producer because the queue was full.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.inner.dropped.load(Ordering::Relaxed)
    }

    /// Configured slot count.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.inner.capacity
    }

    /// Approximate queued events.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Approximate emptiness check.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::{PadId, PadIdExt, TriggerSource, Velocity};

    fn trigger(index: u8, timestamp_us: u64) -> InputEvent {
        InputEvent::Trigger(TriggerEvent::new(
            PadId::from_index(index),
            Velocity::new(100).unwrap(),
            TriggerSource::Keyboard,
            timestamp_us,
        ))
    }

    #[test]
    fn fifo_order_survives_wraparound() {
        let (producer, consumer) = event_queue(4);
        for round in 0..10_u64 {
            for offset in 0..4_u64 {
                let event = trigger(offset as u8, round * 4 + offset);
                assert!(producer.push(event).is_ok());
            }
            for offset in 0..4_u64 {
                assert_eq!(
                    consumer.pop(),
                    Some(trigger(offset as u8, round * 4 + offset))
                );
            }
            assert_eq!(consumer.pop(), None);
        }
        assert_eq!(producer.dropped(), 0);
    }

    #[test]
    fn push_to_full_queue_drops_and_reports() {
        let (producer, consumer) = event_queue(2);
        assert!(producer.push(trigger(0, 1)).is_ok());
        assert!(producer.push(trigger(1, 2)).is_ok());
        assert_eq!(producer.push(trigger(2, 3)), Err(PushError::Full));
        assert_eq!(producer.dropped(), 1);
        assert_eq!(consumer.dropped(), 1);
        assert_eq!(consumer.len(), 2);
        assert_eq!(consumer.pop(), Some(trigger(0, 1)));
        assert!(producer.push(trigger(2, 3)).is_ok());
        assert_eq!(consumer.pop(), Some(trigger(1, 2)));
        assert_eq!(consumer.pop(), Some(trigger(2, 3)));
        assert_eq!(consumer.pop(), None);
    }

    #[test]
    fn typed_push_helpers_and_diagnostics_preserve_events() {
        let (producer, consumer) = event_queue(4);
        assert_eq!(producer.capacity(), 4);
        assert_eq!(consumer.capacity(), 4);
        assert!(producer.is_empty());
        assert!(consumer.is_empty());
        assert_eq!(producer.len(), 0);

        let trigger = TriggerEvent::new(
            PadId::from_index(2),
            Velocity::new(80).unwrap(),
            TriggerSource::Midi,
            7,
        );
        producer.push_trigger(trigger).unwrap();
        let release = ReleaseEvent::new(PadId::from_index(2), TriggerSource::Midi, 8);
        producer.push_release(release).unwrap();

        assert_eq!(producer.len(), 2);
        assert_eq!(consumer.len(), 2);
        assert!(!producer.is_empty());
        assert!(!consumer.is_empty());
        assert_eq!(producer.dropped(), 0);
        assert_eq!(consumer.dropped(), 0);
        assert_eq!(consumer.pop(), Some(InputEvent::Trigger(trigger)));
        assert_eq!(consumer.pop(), Some(InputEvent::Release(release)));
        assert_eq!(consumer.pop(), None);
        assert!(consumer.is_empty());
    }

    #[test]
    fn queue_is_send_across_threads() {
        let (producer, consumer) = event_queue(64);
        let total = 20_000_u64;
        std::thread::scope(|scope| {
            scope.spawn(move || {
                let mut next = 0;
                while next < total {
                    if producer.push(trigger((next % 16) as u8, next)).is_ok() {
                        next += 1;
                    } else {
                        std::hint::spin_loop();
                    }
                }
            });
            let mut received = 0;
            while received < total {
                match consumer.pop() {
                    Some(event) => {
                        assert_eq!(event.timestamp_us(), received);
                        assert_eq!(event.pad().index(), (received % 16) as usize);
                        received += 1;
                    }
                    None => std::hint::spin_loop(),
                }
            }
            assert_eq!(consumer.pop(), None);
        });
    }

    #[test]
    fn dropping_the_queue_releases_pending_events() {
        let (producer, consumer) = event_queue(8);
        for index in 0..8 {
            producer.push(trigger(index, u64::from(index))).unwrap();
        }
        drop(consumer);
        drop(producer);
        // Reaching this point without a panic or leak (checked by Miri/ASan in
        // CI) proves the ring drops initialized slots exactly once.
    }

    #[test]
    #[should_panic(expected = "power of two")]
    fn capacity_must_be_a_power_of_two() {
        let _ = event_queue(3);
    }
}
