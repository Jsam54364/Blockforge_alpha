//! Fixed-timestep accumulator. The simulation always advances in equal steps, which
//! keeps it deterministic and independent of render frame rate.

use std::time::{Duration, Instant};

pub struct FixedTimestep {
    step: Duration,
    acc: Duration,
    last: Option<Instant>,
    max_catchup: u32,
}

impl FixedTimestep {
    pub fn new(hz: u32) -> Self {
        FixedTimestep {
            step: Duration::from_secs_f64(1.0 / (hz.max(1) as f64)),
            acc: Duration::ZERO,
            last: None,
            max_catchup: 5,
        }
    }

    pub fn step(&self) -> Duration {
        self.step
    }

    /// How many ticks to run now. Drops the backlog after `max_catchup` ticks so a
    /// long stall (phone sleeping, debugger) can't cause a spiral of death.
    pub fn advance(&mut self, now: Instant) -> u32 {
        let dt = match self.last {
            Some(l) => now.saturating_duration_since(l),
            None => Duration::ZERO,
        };
        self.last = Some(now);
        self.advance_by(dt)
    }

    pub fn advance_by(&mut self, dt: Duration) -> u32 {
        self.acc += dt;
        let mut n = 0;
        while self.acc >= self.step && n < self.max_catchup {
            self.acc -= self.step;
            n += 1;
        }
        if self.acc >= self.step {
            self.acc = Duration::ZERO;
        }
        n
    }

    /// 0..1 progress to the next tick, for render interpolation.
    pub fn alpha(&self) -> f32 {
        self.acc.as_secs_f32() / self.step.as_secs_f32()
    }

    pub fn time_until_next(&self) -> Duration {
        self.step.saturating_sub(self.acc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_whole_ticks_and_keeps_remainder() {
        let mut t = FixedTimestep::new(20); // 50 ms
        assert_eq!(t.advance_by(Duration::from_millis(120)), 2);
        assert_eq!(t.advance_by(Duration::from_millis(30)), 1); // 20 left + 30
        assert_eq!(t.advance_by(Duration::from_millis(10)), 0);
    }

    #[test]
    fn long_stall_is_capped() {
        let mut t = FixedTimestep::new(20);
        assert_eq!(t.advance_by(Duration::from_secs(10)), 5);
        assert_eq!(t.advance_by(Duration::ZERO), 0);
    }

    #[test]
    fn alpha_is_fractional() {
        let mut t = FixedTimestep::new(10);
        t.advance_by(Duration::from_millis(50));
        assert!((t.alpha() - 0.5).abs() < 0.01);
    }
}
