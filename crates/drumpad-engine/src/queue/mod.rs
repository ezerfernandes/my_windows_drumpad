//! Lock-free queues between input/UI threads and the audio thread.
//!
//! Pad triggers and releases use the canonical SPSC queue from
//! `drumpad_input::trigger::queue` (it owns `InputEvent` and its drop policy),
//! which the engine drains in the callback. Parameter commands use the generic
//! `rtrb` queue in this module. Neither path allocates, locks, or calls into
//! the OS on the push/pop fast path.
//!
//! Drop policy for commands: **drop the newest** and increment a shared
//! counter, so a stalled audio thread cannot block the UI.

use crate::voice::{Interpolation, Playback, SampleBank};
use drumpad_model::ids::PadId;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Default command queue capacity.
pub const DEFAULT_COMMAND_CAPACITY: usize = 1024;

/// How the voice pool resolves exhaustion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StealPolicy {
    /// Steal the voice that started earliest.
    #[default]
    Oldest,
    /// Steal the voice with the lowest current envelope.
    Quietest,
}

/// Non-real-time commands applied at the start of an audio block.
///
/// `SwapSampleBank` moves an `Arc` into the audio thread; the engine never
/// clones or drops it in the callback. It retires the previous bank through a
/// return queue for the UI thread to drop.
#[derive(Debug, Clone)]
pub enum Command {
    /// Set a pad's linear gain (`0.0..=2.0`).
    SetPadGain {
        /// Pad index.
        pad: PadId,
        /// Linear gain.
        gain: f32,
    },
    /// Set a pad's pan (`-1.0` left .. `1.0` right).
    SetPadPan {
        /// Pad index.
        pad: PadId,
        /// Pan position.
        pan: f32,
    },
    /// Set a pad's tuning in cents, applied on the next trigger.
    SetPadTune {
        /// Pad index.
        pad: PadId,
        /// Tuning in cents.
        cents: i32,
    },
    /// Set a pad's choke group (`0` disables choking).
    SetPadChokeGroup {
        /// Pad index.
        pad: PadId,
        /// Choke group in `0..=16`.
        group: u8,
    },
    /// Set a pad's playback mode and loop points. Applies to the next trigger.
    SetPadPlayback {
        /// Pad index.
        pad: PadId,
        /// Playback configuration.
        playback: Playback,
    },
    /// Point a pad at a sample bank slot.
    SetPadSample {
        /// Pad index.
        pad: PadId,
        /// Sample bank slot.
        slot: u16,
    },
    /// Clear a pad's sample assignment (silent pad).
    ClearPadSample {
        /// Pad index.
        pad: PadId,
    },
    /// Change the active voice limit (16..=256) without reallocating.
    SetVoiceLimit {
        /// Number of voices the allocator may use.
        voices: u16,
    },
    /// Change the stealing policy.
    SetStealPolicy(StealPolicy),
    /// Change the default interpolation for new voices.
    SetInterpolation(Interpolation),
    /// Enable or bypass the master limiter.
    SetLimiterEnabled(bool),
    /// Publish a new sample bank. Voices fade out before the swap.
    SwapSampleBank(Arc<SampleBank>),
    /// Release every held gate voice.
    AllNotesOff,
    /// Stop every voice immediately with a short fade.
    Panic,
    /// Clear the peak/RMS meters on the next block.
    ResetMeters,
}

/// Producer half of a lock-free SPSC queue with an observable drop counter.
#[derive(Debug)]
pub struct QueueProducer<T> {
    inner: rtrb::Producer<T>,
    dropped: Arc<AtomicU64>,
}

impl<T> QueueProducer<T> {
    /// Push an item. On a full queue the item is returned and the dropped
    /// counter is incremented (drop-newest policy).
    pub fn push(&mut self, item: T) -> Result<(), T> {
        match self.inner.push(item) {
            Ok(()) => Ok(()),
            Err(rtrb::PushError::Full(item)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                Err(item)
            }
        }
    }

    /// Number of items dropped because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Shared dropped counter, for diagnostics outside the producer.
    pub fn dropped_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.dropped)
    }

    /// Queue capacity in items.
    pub fn capacity(&self) -> usize {
        self.inner.buffer().capacity()
    }

    /// Number of items currently queued.
    pub fn len(&self) -> usize {
        self.capacity().saturating_sub(self.inner.slots())
    }

    /// True when no item is queued.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// True when the queue is full.
    pub fn is_full(&self) -> bool {
        self.inner.is_full()
    }
}

/// Consumer half of a lock-free SPSC queue. Designed for the audio thread.
#[derive(Debug)]
pub struct QueueConsumer<T> {
    inner: rtrb::Consumer<T>,
}

impl<T> QueueConsumer<T> {
    /// Pop the oldest item. Never blocks or allocates.
    pub fn pop(&mut self) -> Option<T> {
        self.inner.pop().ok()
    }

    /// Borrow the next item without removing it. Used to avoid taking
    /// ownership of a sample bank we cannot retire yet.
    pub fn peek(&self) -> Option<&T> {
        self.inner.peek().ok()
    }

    /// Queue capacity in items.
    pub fn capacity(&self) -> usize {
        self.inner.buffer().capacity()
    }

    /// Number of items currently queued.
    pub fn len(&self) -> usize {
        self.inner.slots()
    }

    /// True when no item is queued.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// True when the queue is full.
    pub fn is_full(&self) -> bool {
        self.inner.slots() == self.inner.buffer().capacity()
    }
}

/// Create a queue pair and a shared dropped-event counter.
pub fn queue<T>(capacity: usize) -> (QueueProducer<T>, QueueConsumer<T>, Arc<AtomicU64>) {
    let (producer, consumer) = rtrb::RingBuffer::new(capacity.max(1));
    let dropped = Arc::new(AtomicU64::new(0));
    (
        QueueProducer {
            inner: producer,
            dropped: Arc::clone(&dropped),
        },
        QueueConsumer { inner: consumer },
        dropped,
    )
}

/// Convenience constructor for the command queue.
pub fn command_queue() -> (
    QueueProducer<Command>,
    QueueConsumer<Command>,
    Arc<AtomicU64>,
) {
    queue(DEFAULT_COMMAND_CAPACITY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use drumpad_input::trigger::{
        InputEvent, PadId, TriggerEvent, TriggerSource, Velocity, event_queue,
    };

    fn trigger(index: u8, timestamp_us: u64) -> InputEvent {
        InputEvent::Trigger(TriggerEvent::new(
            PadId::from_raw_unchecked(index),
            Velocity::new(100).unwrap(),
            TriggerSource::Keyboard,
            timestamp_us,
        ))
    }

    #[test]
    fn sustained_ten_thousand_events_lose_nothing() {
        let (producer, consumer) = event_queue(1024);
        let mut expected = 0u64;
        for index in 0..10_000u64 {
            let event = trigger((index % 16) as u8, index * 48);
            producer.push(event).unwrap();
            if index % 8 == 7 {
                while let Some(event) = consumer.pop() {
                    assert_eq!(event.timestamp_us(), expected * 48);
                    assert_eq!(event.pad().index(), (expected % 16) as usize);
                    expected += 1;
                }
            }
        }
        while let Some(event) = consumer.pop() {
            assert_eq!(event.timestamp_us(), expected * 48);
            expected += 1;
        }
        assert_eq!(expected, 10_000);
        assert_eq!(producer.dropped(), 0);
    }

    #[test]
    fn full_event_queue_drops_newest_and_counts_it() {
        let (producer, consumer) = event_queue(2);
        assert!(producer.push(trigger(0, 1)).is_ok());
        assert!(producer.push(trigger(1, 2)).is_ok());
        assert!(producer.push(trigger(2, 3)).is_err());
        assert_eq!(producer.dropped(), 1);
        assert_eq!(consumer.pop().unwrap().timestamp_us(), 1);
        assert_eq!(consumer.pop().unwrap().timestamp_us(), 2);
        assert!(consumer.pop().is_none());
    }

    #[test]
    fn command_queue_wraps_many_times() {
        let (mut producer, mut consumer, dropped) = queue::<u32>(8);
        for round in 0..1000u32 {
            for index in 0..6 {
                producer.push(round * 6 + index).unwrap();
            }
            for index in 0..6 {
                assert_eq!(consumer.pop().unwrap(), round * 6 + index);
            }
        }
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn full_command_queue_drops_newest_and_counts_it() {
        let (mut producer, mut consumer, dropped) = queue::<u32>(4);
        for index in 0..4 {
            producer.push(index).unwrap();
        }
        assert!(producer.push(4).is_err());
        assert_eq!(producer.dropped(), 1);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        for index in 0..4 {
            assert_eq!(consumer.pop().unwrap(), index);
        }
        assert!(consumer.pop().is_none());
    }

    #[test]
    fn producer_len_reports_queued_items() {
        let (mut producer, mut consumer, _dropped) = queue::<u32>(4);
        assert_eq!(producer.capacity(), 4);
        assert_eq!(producer.len(), 0);
        assert!(producer.is_empty());
        assert!(!producer.is_full());

        producer.push(1).unwrap();
        assert_eq!(producer.len(), 1);
        assert!(!producer.is_empty());
        for value in 2..=4 {
            producer.push(value).unwrap();
        }
        assert_eq!(producer.len(), 4);
        assert!(producer.is_full());

        assert_eq!(consumer.pop(), Some(1));
        producer.push(5).unwrap();
        assert_eq!(producer.len(), 4);
        assert!(producer.is_full());
        assert_eq!(consumer.pop(), Some(2));
        assert_eq!(consumer.pop(), Some(3));
        assert_eq!(consumer.pop(), Some(4));
        assert_eq!(consumer.pop(), Some(5));
    }

    #[test]
    fn commands_can_carry_a_sample_bank_across_threads() {
        let mut bank = SampleBank::new();
        let sample = crate::voice::SampleBuffer::from_mono(vec![0.0, 0.1, 0.2], 48_000).unwrap();
        bank.push(Arc::new(sample));
        let bank = Arc::new(bank);

        let (mut producer, mut consumer, _) = queue::<Command>(8);
        producer
            .push(Command::SwapSampleBank(Arc::clone(&bank)))
            .unwrap();
        match consumer.pop().unwrap() {
            Command::SwapSampleBank(received) => {
                assert_eq!(received.len(), 1);
                assert_eq!(received.get(0).unwrap().frame_count(), 3);
            }
            other => panic!("expected bank swap, got {other:?}"),
        }
    }

    #[test]
    fn peek_does_not_consume() {
        let (mut producer, consumer, _dropped) = queue::<u32>(4);
        producer.push(7).unwrap();
        assert_eq!(consumer.peek(), Some(&7));
        assert_eq!(consumer.len(), 1);
        assert_eq!(consumer.peek(), Some(&7));
    }

    #[test]
    fn producers_and_consumers_are_send() {
        fn assert_send<T: Send>() {}
        assert_send::<QueueProducer<Command>>();
        assert_send::<QueueConsumer<Command>>();
        assert_send::<drumpad_input::trigger::EventProducer>();
        assert_send::<drumpad_input::trigger::EventConsumer>();
    }
}
