//! Provider cache models and hit-rate floors — the basis for Cull's cache-correct compression.

/// Caching provider + TTL regime (spec section 8). Determines the write/read multipliers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Anthropic5m,
    Anthropic1h,
    OpenAi,
}

/// Provider-parameterized cache economics. `write_mult` (W) and `read_mult` (R) are
/// relative to base input price; all thresholds derive from them.
#[derive(Debug, Clone, Copy)]
pub struct CacheModel {
    pub write_mult: f64,        // W
    pub read_mult: f64,         // R
    pub min_prefix_tokens: u32, // below this, no caching occurs
}

impl CacheModel {
    pub fn for_provider(p: Provider) -> CacheModel {
        match p {
            Provider::Anthropic5m => CacheModel {
                write_mult: 1.25,
                read_mult: 0.1,
                min_prefix_tokens: 1024,
            },
            Provider::Anthropic1h => CacheModel {
                write_mult: 2.0,
                read_mult: 0.1,
                min_prefix_tokens: 1024,
            },
            Provider::OpenAi => CacheModel {
                write_mult: 1.0,
                read_mult: 0.1,
                min_prefix_tokens: 1024,
            },
        }
    }

    /// Caching is net-positive when hit_rate h > (W-1)/(W-R).
    pub fn caching_net_positive(&self, hit_rate: f64) -> bool {
        hit_rate > (self.write_mult - 1.0) / (self.write_mult - self.read_mult)
    }

    /// Provider hit-rate floor = break-even h below which caching is net-negative: (W-1)/(W-R).
    pub fn hit_rate_floor(&self) -> f64 {
        (self.write_mult - 1.0) / (self.write_mult - self.read_mult)
    }

    /// Compress-once-at-boundary pays off when n_future > W / ((1-c) * R),
    /// where c = compressed_tokens / original_tokens (c < 1 means smaller).
    pub fn amortization_gate(&self, compression_ratio: f64, n_future_turns: u32) -> bool {
        if compression_ratio >= 1.0 {
            return false;
        }
        (n_future_turns as f64) > self.write_mult / ((1.0 - compression_ratio) * self.read_mult)
    }

    /// Deliberately busting the cache to shrink context pays off when
    /// (t_old - t_new) * R * n_future > t_new * W.
    pub fn cache_bust_worth_it(&self, t_old: u32, t_new: u32, n_future_turns: u32) -> bool {
        if t_new >= t_old {
            return false;
        }
        ((t_old - t_new) as f64) * self.read_mult * (n_future_turns as f64)
            > (t_new as f64) * self.write_mult
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caching_break_even_thresholds() {
        let a5 = CacheModel::for_provider(Provider::Anthropic5m);
        assert!(a5.caching_net_positive(0.22)); // > 0.2174
        assert!(!a5.caching_net_positive(0.21));

        let a1 = CacheModel::for_provider(Provider::Anthropic1h);
        assert!(a1.caching_net_positive(0.53)); // > 0.5263
        assert!(!a1.caching_net_positive(0.52));

        let oa = CacheModel::for_provider(Provider::OpenAi);
        assert!(oa.caching_net_positive(0.0001)); // threshold is 0 (no write premium)
    }

    #[test]
    fn amortization_gate_threshold() {
        let a5 = CacheModel::for_provider(Provider::Anthropic5m);
        // W/((1-c)*R) = 1.25/((0.4)*0.1) = 31.25
        assert!(a5.amortization_gate(0.6, 32));
        assert!(!a5.amortization_gate(0.6, 31));
        assert!(!a5.amortization_gate(1.0, 10_000)); // no compression never amortizes
    }

    #[test]
    fn hit_rate_floor_matches_break_even() {
        let a5 = CacheModel::for_provider(Provider::Anthropic5m);
        assert!((a5.hit_rate_floor() - 0.21739).abs() < 1e-4); // (1.25-1)/(1.25-0.1)
        let a1 = CacheModel::for_provider(Provider::Anthropic1h);
        assert!((a1.hit_rate_floor() - 0.52632).abs() < 1e-4); // (2-1)/(2-0.1)
        let oa = CacheModel::for_provider(Provider::OpenAi);
        assert!(oa.hit_rate_floor().abs() < 1e-9); // (1-1)/(1-0.1) = 0
    }

    #[test]
    fn cache_bust_threshold() {
        let a5 = CacheModel::for_provider(Provider::Anthropic5m);
        // (t_old - t_new)*R*N > t_new*W ; 100k->20k: (80000*0.1*N) > 20000*1.25 => N > 3.125
        assert!(a5.cache_bust_worth_it(100_000, 20_000, 4));
        assert!(!a5.cache_bust_worth_it(100_000, 20_000, 3));
        assert!(!a5.cache_bust_worth_it(20_000, 100_000, 1000)); // growing is never worth busting
    }
}
