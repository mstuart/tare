use regex::Regex;
use std::collections::HashSet;
use std::sync::OnceLock;

/// The current task's technical symbols (function/type/file names, error codes, paths),
/// lowercased. Drives query-conditioned passes. An empty signal means "no task" → query
/// passes do nothing (safe default).
#[derive(Debug, Clone, Default)]
pub struct TaskSignal {
    pub symbols: HashSet<String>,
}

impl TaskSignal {
    pub fn empty() -> Self {
        Self::default()
    }
    pub fn from_text(text: &str) -> Self {
        Self {
            symbols: extract_symbols(text),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }
}

fn ident_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // identifiers, dotted names, and path-like tokens (stops at ':' so "jwt.rs:42" -> "jwt.rs")
    R.get_or_init(|| Regex::new(r"[A-Za-z_][A-Za-z0-9_./-]{2,}").unwrap())
}

fn is_stopword(s: &str) -> bool {
    const STOP: &[&str] = &[
        "the", "and", "for", "this", "that", "with", "from", "into", "not", "but", "all", "can",
        "you", "are", "let", "pub", "use", "mut", "self", "return", "const", "fix", "add", "make",
        "get", "set", "new", "out", "the.", "src", "run", "why", "how", "fn.", "ref",
    ];
    STOP.contains(&s)
}

/// Extract candidate technical symbols, lowercased, length >= 3, minus common stopwords.
/// Heuristic relevance signal; tree-sitter precision and embedding salience are later upgrades.
pub fn extract_symbols(text: &str) -> HashSet<String> {
    ident_re()
        .find_iter(text)
        .map(|m| m.as_str().to_ascii_lowercase())
        .filter(|s| s.len() >= 3 && !is_stopword(s))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_technical_symbols_and_drops_stopwords() {
        let sig = TaskSignal::from_text("fix the auth bug in src/jwt.rs TokenExpiredError");
        assert!(sig.symbols.contains("auth"));
        assert!(sig.symbols.contains("src/jwt.rs"));
        assert!(sig.symbols.contains("tokenexpirederror")); // lowercased
        assert!(!sig.symbols.contains("the")); // stopword
        assert!(!sig.symbols.contains("fix")); // stopword
    }

    #[test]
    fn empty_text_is_empty_signal() {
        assert!(TaskSignal::from_text("   ").is_empty());
        assert!(TaskSignal::empty().is_empty());
    }
}
