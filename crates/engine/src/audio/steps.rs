//! Which of a list of footstep clips plays next: in list order, truly at
//! random, or "smart" random (no recent repeats, slight variation per step).

use crate::player::settings::{Footsteps, RandomOrder};
use std::collections::VecDeque;

pub struct StepPicker {
    /// Next index in list order.
    next: usize,
    /// Most recently played indices, newest last.
    recent: VecDeque<usize>,
    /// xorshift state, never 0.
    seed: u32,
}

impl StepPicker {
    pub fn new(seed: u32) -> Self {
        Self {
            next: 0,
            recent: VecDeque::new(),
            seed: seed.max(1),
        }
    }

    /// The clip to play out of `count` (> 0).
    pub fn pick(&mut self, count: usize, steps: &Footsteps) -> usize {
        let index = match (steps.randomize, steps.random) {
            (false, _) => self.next % count,
            (true, RandomOrder::True) => self.below(count),
            (true, RandomOrder::Smart) => {
                // Skip the last half of the list's worth of clips: with 2
                // clips they alternate, with 4 each step picks from the 2
                // that played least recently.
                let avoid = count / 2;
                let skip = self.recent.len().saturating_sub(avoid);
                let fresh: Vec<usize> = (0..count)
                    .filter(|i| !self.recent.iter().skip(skip).any(|r| r == i))
                    .collect();
                fresh[self.below(fresh.len())]
            }
        };
        self.next = index + 1;
        self.recent.push_back(index);
        if self.recent.len() > count {
            self.recent.pop_front();
        }
        index
    }

    /// Smart random's per-step variation: (pitch factor, gain), about ±5%
    /// pitch and up to 1.5 dB quieter.
    pub fn variation(&mut self) -> (f32, f32) {
        (0.95 + 0.1 * self.unit(), 0.84 + 0.16 * self.unit())
    }

    /// 0 ≤ x < 1.
    fn unit(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (self.seed >> 8) as f32 / (1u32 << 24) as f32
    }

    /// 0 ≤ x < n.
    fn below(&mut self, n: usize) -> usize {
        ((self.unit() * n as f32) as usize).min(n - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(randomize: bool, random: RandomOrder) -> Footsteps {
        Footsteps {
            use_default: false,
            volume: 1.0,
            clips: Vec::new(),
            randomize,
            random,
        }
    }

    fn sequence(steps: &Footsteps, count: usize, len: usize) -> Vec<usize> {
        let mut picker = StepPicker::new(12345);
        (0..len).map(|_| picker.pick(count, steps)).collect()
    }

    #[test]
    fn in_order_follows_the_list() {
        let order = sequence(&steps(false, RandomOrder::Smart), 3, 7);
        assert_eq!(order, [0, 1, 2, 0, 1, 2, 0]);
    }

    #[test]
    fn true_random_repeats_sometimes_and_uses_every_clip() {
        let order = sequence(&steps(true, RandomOrder::True), 3, 300);
        assert!(order.windows(2).any(|w| w[0] == w[1]));
        assert!((0..3).all(|i| order.contains(&i)));
    }

    #[test]
    fn smart_random_never_repeats_recent_clips() {
        for count in 1..=8 {
            let order = sequence(&steps(true, RandomOrder::Smart), count, 400);
            let avoid = count / 2;
            for (i, &clip) in order.iter().enumerate() {
                let recent = &order[i.saturating_sub(avoid)..i];
                assert!(!recent.contains(&clip), "{count} clips: {order:?}");
            }
            assert!((0..count).all(|i| order.contains(&i)));
        }
    }

    #[test]
    fn smart_variation_stays_small() {
        let mut picker = StepPicker::new(7);
        for _ in 0..1000 {
            let (pitch, gain) = picker.variation();
            assert!((0.95..=1.05).contains(&pitch) && (0.84..=1.0).contains(&gain));
        }
    }
}
