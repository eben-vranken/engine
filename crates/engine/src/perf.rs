//! Measuring hitches: a ring buffer of frame times, and timers and counters
//! around work that may spike (acoustic rebuilds, scene uploads, shadow
//! passes). Everything is summarised per interval (`INTERVAL`), so
//! `--perf-log` and the test drawer's STATS tab show the same numbers.
//!
//! Timers keep the maximum, not the average: one 30 ms rebuild every few
//! seconds is a visible hitch that an average hides.

use std::sync::Mutex;
use std::time::Instant;

/// How long one summary covers, in seconds.
pub const INTERVAL: f32 = 2.0;

/// Frames kept: more than one interval at any refresh rate we expect.
const CAPACITY: usize = 1024;

/// A frame counts as long beyond this many refresh intervals.
pub const LONG_FRAME: f32 = 1.5;

/// The last `CAPACITY` frame times, in milliseconds.
pub struct FrameTimes {
    ring: Vec<f32>,
    next: usize,
    /// Frames pushed since the last summary.
    since: usize,
}

impl Default for FrameTimes {
    fn default() -> Self {
        Self {
            ring: Vec::with_capacity(CAPACITY),
            next: 0,
            since: 0,
        }
    }
}

/// Frame times over one interval.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameSummary {
    pub frames: usize,
    pub p50: f32,
    pub p99: f32,
    pub max: f32,
    /// Frames longer than `LONG_FRAME` × the refresh interval.
    pub long: usize,
    /// The refresh interval this was judged against, in milliseconds.
    pub refresh_ms: f32,
}

impl FrameTimes {
    pub fn push(&mut self, ms: f32) {
        if self.ring.len() < CAPACITY {
            self.ring.push(ms);
        } else {
            self.ring[self.next] = ms;
        }
        self.next = (self.next + 1) % CAPACITY;
        self.since += 1;
    }

    /// Summarises the frames since the last call (at most `CAPACITY`).
    pub fn summary(&mut self, refresh_ms: f32) -> FrameSummary {
        let n = self.since.min(self.ring.len());
        self.since = 0;
        if n == 0 {
            return FrameSummary {
                refresh_ms,
                ..Default::default()
            };
        }
        // The newest `n` frames end just before `next`.
        let mut frames: Vec<f32> = (0..n)
            .map(|i| self.ring[(self.next + CAPACITY - 1 - i) % CAPACITY])
            .collect();
        frames.sort_by(f32::total_cmp);
        let at = |q: f32| frames[((n - 1) as f32 * q).round() as usize];
        FrameSummary {
            frames: n,
            p50: at(0.5),
            p99: at(0.99),
            max: frames[n - 1],
            long: frames
                .iter()
                .filter(|&&ms| ms > refresh_ms * LONG_FRAME)
                .count(),
            refresh_ms,
        }
    }
}

/// One timer or counter over an interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stat {
    pub name: &'static str,
    pub kind: Kind,
    /// Timers: the longest call, in milliseconds. Counters: the largest
    /// single add.
    pub max: f32,
    /// Timers: milliseconds in all. Counters: the sum.
    pub total: f32,
    /// Calls or adds.
    pub count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Time,
    Count,
}

struct Stats {
    current: Vec<Stat>,
    last: Vec<Stat>,
}

static STATS: Mutex<Stats> = Mutex::new(Stats {
    current: Vec::new(),
    last: Vec::new(),
});

fn record(name: &'static str, kind: Kind, value: f32) {
    let Ok(mut stats) = STATS.lock() else { return };
    match stats.current.iter_mut().find(|s| s.name == name) {
        Some(stat) => {
            stat.max = stat.max.max(value);
            stat.total += value;
            stat.count += 1;
        }
        None => stats.current.push(Stat {
            name,
            kind,
            max: value,
            total: value,
            count: 1,
        }),
    }
}

/// Runs `f`, recording how long it took under `name`.
pub fn time<T>(name: &'static str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let result = f();
    record(name, Kind::Time, start.elapsed().as_secs_f32() * 1000.0);
    result
}

/// Adds `n` to the counter `name`.
pub fn count(name: &'static str, n: usize) {
    record(name, Kind::Count, n as f32);
}

/// Ends the interval: what was recorded becomes `last()`, in name order.
pub fn roll() {
    let Ok(mut stats) = STATS.lock() else { return };
    let mut done = std::mem::take(&mut stats.current);
    done.sort_by_key(|s| s.name);
    stats.last = done;
}

/// The timers and counters of the last finished interval (only those
/// that ran).
pub fn last() -> Vec<Stat> {
    STATS.lock().map(|s| s.last.clone()).unwrap_or_default()
}

impl Stat {
    /// `12.30 MS MAX (4×)` or `36 (2×)`.
    pub fn text(&self) -> String {
        match self.kind {
            Kind::Time => format!("{:.2} MS MAX ({}×)", self.max, self.count),
            Kind::Count => format!("{:.0} ({}×)", self.total, self.count),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_covers_frames_since_the_last_one() {
        let mut times = FrameTimes::default();
        for _ in 0..99 {
            times.push(16.0);
        }
        times.push(40.0);
        let s = times.summary(16.7);
        assert_eq!(s.frames, 100);
        assert_eq!(s.p50, 16.0);
        assert_eq!(s.max, 40.0);
        assert_eq!(s.long, 1);

        times.push(10.0);
        let s = times.summary(16.7);
        assert_eq!((s.frames, s.max, s.long), (1, 10.0, 0));
        assert_eq!(times.summary(16.7).frames, 0);
    }

    #[test]
    fn ring_keeps_the_newest_frames() {
        let mut times = FrameTimes::default();
        for i in 0..CAPACITY + 10 {
            times.push(i as f32);
        }
        let s = times.summary(1000.0);
        assert_eq!(s.frames, CAPACITY);
        assert_eq!(s.max, (CAPACITY + 9) as f32);
    }

    #[test]
    fn stats_keep_max_and_roll() {
        count("TEST COUNTER", 6);
        count("TEST COUNTER", 12);
        roll();
        let stat = last()
            .into_iter()
            .find(|s| s.name == "TEST COUNTER")
            .unwrap();
        assert_eq!((stat.max, stat.total, stat.count), (12.0, 18.0, 2));
    }
}
