use cull_cache::{CacheModel, Provider};

/// Per-session cache hit-rate monitor (spec §8 Rule 5). Halts compression after `HALT_STREAK`
/// consecutive turns whose observed hit rate is strictly below the provider floor
/// (`CacheModel::hit_rate_floor`, R6). Once halted it stays halted — the operator diagnoses the
/// invalidation source (spec: "halt compression and diagnose"); it does not auto-resume.
const HALT_STREAK: u32 = 3;

#[derive(Debug)]
pub struct HitRateMonitor {
    floor: f64,
    consecutive_below: u32,
    halted: bool,
}

impl HitRateMonitor {
    pub fn new(provider: Provider) -> Self {
        Self { floor: CacheModel::for_provider(provider).hit_rate_floor(), consecutive_below: 0, halted: false }
    }

    /// Record one turn's observed hit rate.
    pub fn observe(&mut self, hit_rate: f64) {
        if hit_rate < self.floor {
            self.consecutive_below += 1;
            if self.consecutive_below >= HALT_STREAK { self.halted = true; }
        } else {
            self.consecutive_below = 0;
        }
    }

    pub fn halted(&self) -> bool { self.halted }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cull_cache::Provider;

    #[test]
    fn halts_after_three_consecutive_below_floor() {
        let mut m = HitRateMonitor::new(Provider::Anthropic5m); // floor ~0.217
        assert!(!m.halted());
        m.observe(0.0); assert!(!m.halted());   // 1 below
        m.observe(0.1); assert!(!m.halted());   // 2 below
        m.observe(0.05);                          // 3 below -> halt
        assert!(m.halted());
    }

    #[test]
    fn a_hit_resets_the_streak() {
        let mut m = HitRateMonitor::new(Provider::Anthropic5m);
        m.observe(0.0); m.observe(0.0);          // 2 below
        m.observe(0.9);                           // a hit resets
        m.observe(0.0); m.observe(0.0);          // 2 below again
        assert!(!m.halted());                     // never hit 3 consecutive
    }

    #[test]
    fn openai_floor_zero_never_halts_on_positive_rate() {
        let mut m = HitRateMonitor::new(Provider::OpenAi); // floor 0
        for _ in 0..10 { m.observe(0.0001); }
        assert!(!m.halted());                     // 0.0001 is NOT below a 0 floor
    }

    #[test]
    fn stays_halted_once_tripped() {
        let mut m = HitRateMonitor::new(Provider::Anthropic5m);
        m.observe(0.0); m.observe(0.0); m.observe(0.0);
        assert!(m.halted());
        m.observe(0.99);                          // recovery does not auto-resume (spec: halt + diagnose)
        assert!(m.halted());
    }
}
