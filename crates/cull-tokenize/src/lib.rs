use tiktoken_rs::{o200k_base, CoreBPE};

/// Counts tokens for budgeting/segmentation. Exact provider counts (e.g. Anthropic
/// `count_tokens`) are added in a later plan; this approximation is deterministic and offline.
pub trait TokenCounter: Send + Sync {
    fn count(&self, text: &str) -> usize;
}

pub struct ApproxCounter {
    bpe: CoreBPE,
}

impl ApproxCounter {
    /// o200k_base — the modern BPE; a stable approximation across providers.
    pub fn o200k() -> Self {
        Self {
            bpe: o200k_base().expect("o200k_base BPE must load"),
        }
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
