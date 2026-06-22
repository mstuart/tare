use tare_cache::{CacheModel, Provider};

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
        Self {
            floor: CacheModel::for_provider(provider).hit_rate_floor(),
            consecutive_below: 0,
            halted: false,
        }
    }

    /// Record one turn's observed hit rate.
    pub fn observe(&mut self, hit_rate: f64) {
        if hit_rate < self.floor {
            self.consecutive_below += 1;
            if self.consecutive_below >= HALT_STREAK {
                self.halted = true;
            }
        } else {
            self.consecutive_below = 0;
        }
    }

    pub fn halted(&self) -> bool {
        self.halted
    }
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
#[derive(Debug, Default)]
pub struct OutputMonitor {
    baseline: Option<f64>, // EWMA of per-turn output tokens
    spiking: bool,         // is the session currently in verbosity back-off?
    turns: u32,
    consecutive_spikes: u32, // run length of above-threshold turns (bounds the back-off)
}

/// EWMA smoothing for the output baseline (higher = more reactive to recent turns).
const EWMA_ALPHA: f64 = 0.3;
/// Output above `SPIKE_FACTOR × baseline` is treated as verbosity compensation.
const SPIKE_FACTOR: f64 = 1.75;
/// Observations needed before a baseline is trustworthy enough to flag a spike.
const WARMUP_TURNS: u32 = 2;
/// After this many consecutive high turns, treat the level as a genuine SHIFT (a legitimately longer
/// task), re-baseline, and release back-off — so we never compress-conservatively forever. Transient
/// verbosity-compensation from over-compression clears within a few turns once we back off.
const SPIKE_ADAPT_LIMIT: u32 = 5;

impl OutputMonitor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one turn's output token count. Returns `true` while the session is in verbosity
    /// back-off. A spike turn is NOT folded into the baseline (else a model stuck in verbosity-
    /// compensation would inflate the baseline and silence the signal after one turn) — UNLESS the
    /// high level persists for `SPIKE_ADAPT_LIMIT` turns, at which point it's a genuine shift, so we
    /// re-baseline and release back-off rather than stay conservative forever.
    pub fn observe(&mut self, output_tokens: u64) -> bool {
        let o = output_tokens as f64;
        let over =
            matches!(self.baseline, Some(b) if self.turns >= WARMUP_TURNS && o > b * SPIKE_FACTOR);
        self.consecutive_spikes = if over { self.consecutive_spikes + 1 } else { 0 };
        let adapting_shift = over && self.consecutive_spikes >= SPIKE_ADAPT_LIMIT;
        if !over || adapting_shift {
            self.baseline = Some(match self.baseline {
                Some(b) => EWMA_ALPHA * o + (1.0 - EWMA_ALPHA) * b,
                None => o,
            });
        }
        self.turns += 1;
        // Back off while a spike is active, but not once we've adapted to a sustained new level.
        self.spiking = over && !adapting_shift;
        self.spiking
    }

    /// Whether the most recently observed turn was a verbosity spike (read at the next turn).
    pub fn spiking(&self) -> bool {
        self.spiking
    }

    /// Current EWMA output baseline, if any turn has been observed.
    pub fn baseline(&self) -> Option<f64> {
        self.baseline
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tare_cache::Provider;

    #[test]
    fn halts_after_three_consecutive_below_floor() {
        let mut m = HitRateMonitor::new(Provider::Anthropic5m); // floor ~0.217
        assert!(!m.halted());
        m.observe(0.0);
        assert!(!m.halted()); // 1 below
        m.observe(0.1);
        assert!(!m.halted()); // 2 below
        m.observe(0.05); // 3 below -> halt
        assert!(m.halted());
    }

    #[test]
    fn a_hit_resets_the_streak() {
        let mut m = HitRateMonitor::new(Provider::Anthropic5m);
        m.observe(0.0);
        m.observe(0.0); // 2 below
        m.observe(0.9); // a hit resets
        m.observe(0.0);
        m.observe(0.0); // 2 below again
        assert!(!m.halted()); // never hit 3 consecutive
    }

    #[test]
    fn openai_floor_zero_never_halts_on_positive_rate() {
        let mut m = HitRateMonitor::new(Provider::OpenAi); // floor 0
        for _ in 0..10 {
            m.observe(0.0001);
        }
        assert!(!m.halted()); // 0.0001 is NOT below a 0 floor
    }

    #[test]
    fn stays_halted_once_tripped() {
        let mut m = HitRateMonitor::new(Provider::Anthropic5m);
        m.observe(0.0);
        m.observe(0.0);
        m.observe(0.0);
        assert!(m.halted());
        m.observe(0.99); // recovery does not auto-resume (spec: halt + diagnose)
        assert!(m.halted());
    }

    #[test]
    fn output_monitor_flags_verbosity_spike_after_warmup() {
        let mut m = OutputMonitor::new();
        assert!(!m.observe(100)); // warmup turn 1 — no baseline yet
        assert!(!m.observe(120)); // warmup turn 2
        assert!(!m.observe(110)); // normal turn, near baseline -> no spike
        assert!(!m.spiking());
        // a 4x jump in output is verbosity compensation -> spike
        assert!(m.observe(420), "large output jump should flag a spike");
        assert!(m.spiking());
        assert!(m.baseline().is_some());
    }

    #[test]
    fn output_monitor_no_spike_on_steady_output() {
        let mut m = OutputMonitor::new();
        for _ in 0..6 {
            assert!(!m.observe(200), "steady output never spikes");
        }
        assert!(!m.spiking());
    }

    #[test]
    fn output_monitor_does_not_flag_during_warmup() {
        let mut m = OutputMonitor::new();
        // even a huge second turn can't spike before the baseline is trustworthy
        assert!(!m.observe(10));
        assert!(!m.observe(10_000));
    }

    #[test]
    fn output_monitor_sustained_verbosity_keeps_spiking() {
        // regression for the EWMA-masking bug: a model stuck verbose must keep flagging, not just
        // on the first spike turn (the spike must not inflate the baseline).
        let mut m = OutputMonitor::new();
        m.observe(100);
        m.observe(100); // baseline ~100
        assert!(m.observe(400), "first high turn spikes");
        assert!(
            m.observe(400),
            "sustained high output STILL spikes (baseline not inflated)"
        );
        assert!(m.observe(400), "and keeps spiking");
        // once output returns to normal, the baseline resumes and the spike clears
        assert!(!m.observe(100));
    }

    #[test]
    fn output_monitor_adapts_after_sustained_shift_releases_backoff() {
        // regression guard for the never-adapt-up bug: a genuine, sustained higher output level must
        // NOT keep the session in back-off forever — after SPIKE_ADAPT_LIMIT turns it re-baselines.
        let mut m = OutputMonitor::new();
        m.observe(100);
        m.observe(100); // baseline ~100
        let mut active = 0;
        for _ in 0..10 {
            if m.observe(400) {
                active += 1;
            }
        }
        assert!(active >= 1, "transient verbosity backs off at least once");
        assert!(
            active <= SPIKE_ADAPT_LIMIT,
            "back-off is bounded, not forever (got {active})"
        );
        assert!(
            !m.spiking(),
            "after a sustained shift the baseline adapts and back-off releases"
        );
    }
}
