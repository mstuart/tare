use std::sync::OnceLock;
use tiktoken_rs::{o200k_base, CoreBPE};

/// Counts tokens for budgeting/segmentation. Exact provider counts (e.g. Anthropic
/// `count_tokens`) are added in a later plan; this approximation is deterministic and offline.
pub trait TokenCounter: Send + Sync {
    fn count(&self, text: &str) -> usize;
}

/// The o200k BPE (≈200k merge rules) is immutable and expensive to construct (~100s of ms), so it
/// is built once per process and shared. Without this, every `ApproxCounter::o200k()` rebuilt the
/// whole table — and it is constructed per request in the proxy and repeatedly inside segmentation
/// and each pass, so the rebuild cost dominated real latency.
static O200K_BPE: OnceLock<CoreBPE> = OnceLock::new();

fn o200k_bpe() -> &'static CoreBPE {
    O200K_BPE.get_or_init(|| o200k_base().expect("o200k_base BPE must load"))
}

pub struct ApproxCounter {
    bpe: &'static CoreBPE,
}

impl ApproxCounter {
    /// o200k_base — the modern BPE; a stable approximation across providers. Shares one
    /// process-global BPE (built on first use), so construction is effectively free after the first.
    pub fn o200k() -> Self {
        Self { bpe: o200k_bpe() }
    }
}

impl TokenCounter for ApproxCounter {
    fn count(&self, text: &str) -> usize {
        if text.is_empty() {
            return 0;
        }
        self.bpe.encode_with_special_tokens(text).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_is_zero_tokens() {
        let c = ApproxCounter::o200k();
        assert_eq!(c.count(""), 0);
    }

    #[test]
    fn counts_are_positive_and_monotonic() {
        let c = ApproxCounter::o200k();
        let short = c.count("hello");
        let long = c.count("hello world this is a longer string of tokens");
        assert!(short > 0);
        assert!(long > short);
    }
}
