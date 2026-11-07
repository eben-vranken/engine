//! How the audio output callback keeps up, for `--perf-log`: the callback
//! has one buffer's worth of time to fill the next buffer, or the device runs
//! dry (an underrun). Written from the callback with atomics, so measuring
//! never blocks it.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

#[derive(Default)]
pub struct Timing {
    /// Microseconds, the worst since the last `take`.
    slowest_fill: AtomicU32,
    longest_gap: AtomicU32,
    /// The time one buffer plays for (the budget), as last seen.
    buffer: AtomicU32,
    underruns: AtomicU32,
}

/// One `--perf-log` interval, in milliseconds.
#[derive(Debug, Clone, Copy)]
pub struct Report {
    /// Longest the callback took to fill a buffer.
    pub slowest_fill: f32,
    /// Longest time between two callbacks starting (about `buffer` when the
    /// thread is woken on time).
    pub longest_gap: f32,
    pub buffer: f32,
    pub underruns: u32,
}

impl Timing {
    /// Records one callback that started at `start` (the previous one at
    /// `previous`) and filled `frames` frames at `rate`.
    pub fn record(&self, previous: Option<Instant>, start: Instant, frames: usize, rate: u32) {
        let us = |seconds: f32| (seconds * 1e6) as u32;
        self.slowest_fill
            .fetch_max(us(start.elapsed().as_secs_f32()), Ordering::Relaxed);
        if let Some(previous) = previous {
            self.longest_gap
                .fetch_max(us((start - previous).as_secs_f32()), Ordering::Relaxed);
        }
        self.buffer
            .store(us(frames as f32 / rate as f32), Ordering::Relaxed);
    }

    pub fn underrun(&self) {
        self.underruns.fetch_add(1, Ordering::Relaxed);
    }

    /// The worst values since the last call, and starts over.
    pub fn take(&self) -> Report {
        let ms = |a: &AtomicU32| a.swap(0, Ordering::Relaxed) as f32 / 1000.0;
        Report {
            slowest_fill: ms(&self.slowest_fill),
            longest_gap: ms(&self.longest_gap),
            buffer: self.buffer.load(Ordering::Relaxed) as f32 / 1000.0,
            underruns: self.underruns.swap(0, Ordering::Relaxed),
        }
    }
}
