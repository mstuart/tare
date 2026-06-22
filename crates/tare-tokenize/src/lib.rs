//! Fast approximate token counting for Tare's compression decisions (chars/4 heuristic).

/// Counts tokens for budgeting/segmentation. Exact provider counts (e.g. Anthropic
/// `count_tokens`) are obtained via the proxy's network client; this is the fast offline
/// approximation used for compression decisions.
pub trait TokenCounter: Send + Sync {
    fn count(&self, text: &str) -> usize;
}

/// Fast, construction-free approximation of o200k token counts: ~one token per 4 characters (the
/// standard rule of thumb; tiktoken averages ≈3.5–4 chars/token across mixed code, JSON, and prose).
/// This preserves the ORDERING and RATIOS that compression decisions depend on — a smaller rendering
/// always counts as fewer tokens — without the ~50ms cost of building the 200k-entry tiktoken BPE
/// table, which (rebuilt per process) dominated CLI latency. Exact counts, when needed, come from
/// the provider `count_tokens` endpoint.
pub struct ApproxCounter;

impl ApproxCounter {
    /// Named `o200k` for source compatibility; returns the fast approximate counter.
    pub fn o200k() -> Self {
        Self
    }
}

impl Default for ApproxCounter {
    fn default() -> Self {
        Self
    }
}

impl TokenCounter for ApproxCounter {
    fn count(&self, text: &str) -> usize {
        // ceil(chars / 4); empty -> 0.
        text.chars().count().div_ceil(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_is_zero_tokens() {
        assert_eq!(ApproxCounter::o200k().count(""), 0);
    }

    #[test]
    fn counts_are_positive_and_monotonic() {
        let c = ApproxCounter::o200k();
        let short = c.count("hello");
        let long = c.count("hello world this is a longer string of tokens");
        assert!(short > 0);
        assert!(long > short);
    }

    #[test]
    fn smaller_text_never_counts_more() {
        // the ordering invariant compression decisions rely on
        let c = ApproxCounter::o200k();
        let full = "{\"id\":0,\"name\":\"item_0\",\"status\":\"active\"}";
        let crushed = "[0,\"item_0\"]";
        assert!(c.count(crushed) < c.count(full));
    }
}
