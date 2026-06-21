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

/// Per-session OUTPUT-side monitor — the compression-paradox sensor (the signal the field ignores).
///
/// Aggressive INPUT compression can make a model *compensate with verbose OUTPUT* (the "verbosity
/// compensation" effect): total session cost can rise even as input tokens fall. Every existing
/// compressor optimizes input-tokens-removed and never looks at the output. This tracks each turn's
/// output token count against a running EWMA baseline and flags a **verbosity spike** when output
/// jumps well above baseline — the cue for the controller to BACK OFF compression instead of pushing
/// it harder. It observes this turn and exposes state the next turn acts on (same cadence as
/// [`HitRateMonitor`]). The spike→action policy lives in the controller, not here; this is the sensor.
#[derive(Debug)]
pub struct OutputMonitor {
    baseline: Option<f64>, // EWMA of per-turn output tokens
    spiking: bool,         // did the most recent observed turn spike vs baseline?
    turns: u32,
}

/// EWMA smoothing for the output baseline (higher = more reactive to recent turns).
const EWMA_ALPHA: f64 = 0.3;
/// Output above `SPIKE_FACTOR × baseline` is treated as verbosity compensation.
const SPIKE_FACTOR: f64 = 1.75;
/// Observations needed before a baseline is trustworthy enough to flag a spike.
const WARMUP_TURNS: u32 = 2;

impl Default for OutputMonitor {
    fn default() -> Self { Self { baseline: None, spiking: false, turns: 0 } }
}

impl OutputMonitor {
    pub fn new() -> Self { Self::default() }

    /// Record one turn's output token count. Returns `true` if it is a verbosity spike vs the
    /// session baseline (after warmup). The baseline is then updated to include this turn.
    pub fn observe(&mut self, output_tokens: u64) -> bool {
        let o = output_tokens as f64;
        let spike = matches!(self.baseline, Some(b) if self.turns >= WARMUP_TURNS && o > b * SPIKE_FACTOR);
        self.baseline = Some(match self.baseline {
            Some(b) => EWMA_ALPHA * o + (1.0 - EWMA_ALPHA) * b,
            None => o,
        });
        self.turns += 1;
        self.spiking = spike;
        spike
    }

    /// Whether the most recently observed turn was a verbosity spike (read at the next turn).
    pub fn spiking(&self) -> bool { self.spiking }

    /// Current EWMA output baseline, if any turn has been observed.
    pub fn baseline(&self) -> Option<f64> { self.baseline }
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

    #[test]
    fn output_monitor_flags_verbosity_spike_after_warmup() {
        let mut m = OutputMonitor::new();
        assert!(!m.observe(100));  // warmup turn 1 — no baseline yet
        assert!(!m.observe(120));  // warmup turn 2
        assert!(!m.observe(110));  // normal turn, near baseline -> no spike
        assert!(!m.spiking());
        // a 4x jump in output is verbosity compensation -> spike
        assert!(m.observe(420), "large output jump should flag a spike");
        assert!(m.spiking());
        assert!(m.baseline().is_some());
    }

    #[test]
    fn output_monitor_no_spike_on_steady_output() {
        let mut m = OutputMonitor::new();
        for _ in 0..6 { assert!(!m.observe(200), "steady output never spikes"); }
        assert!(!m.spiking());
    }

    #[test]
    fn output_monitor_does_not_flag_during_warmup() {
        let mut m = OutputMonitor::new();
        // even a huge second turn can't spike before the baseline is trustworthy
        assert!(!m.observe(10));
        assert!(!m.observe(10_000));
    }
}
