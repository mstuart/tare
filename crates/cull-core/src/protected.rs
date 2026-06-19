use serde::{Deserialize, Serialize};
use regex::Regex;
use std::sync::OnceLock;
use crate::segment::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtectedKind { Path, LineNumber, ErrorCode, NumericLiteral, NullVsEmpty }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectedSpan { pub span: Span, pub kind: ProtectedKind }

struct Patterns { path: Regex, line_no: Regex, error_code: Regex, number: Regex, null_empty: Regex }

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        path: Regex::new(r"(?:[\w.-]+/)+[\w.-]+\.\w+|/[\w./-]+").unwrap(),
        // line numbers like ":128" or ":128:4"; the leading colon is intentionally
        // included in the span (superset protection — never narrows what is protected).
        line_no: Regex::new(r":\d+(?::\d+)?\b").unwrap(),
        error_code: Regex::new(r"\b[EC]\d{3,5}\b").unwrap(),
        number: Regex::new(r"\b\d+\b").unwrap(),
        null_empty: Regex::new(r#"\bnull\b|""|''"#).unwrap(),
    })
}

/// Detect exact-token classes that must never be dropped or mutated (spec I4).
/// Spans may overlap (a path may contain numbers); downstream treats the union as protected.
pub fn detect_protected_spans(text: &str) -> Vec<ProtectedSpan> {
    let p = patterns();
    let mut out = Vec::new();
    for m in p.path.find_iter(text) {
        out.push(ProtectedSpan { span: Span { start: m.start(), end: m.end() }, kind: ProtectedKind::Path });
    }
    for m in p.line_no.find_iter(text) {
        out.push(ProtectedSpan { span: Span { start: m.start(), end: m.end() }, kind: ProtectedKind::LineNumber });
    }
    for m in p.error_code.find_iter(text) {
        out.push(ProtectedSpan { span: Span { start: m.start(), end: m.end() }, kind: ProtectedKind::ErrorCode });
    }
    for m in p.number.find_iter(text) {
        out.push(ProtectedSpan { span: Span { start: m.start(), end: m.end() }, kind: ProtectedKind::NumericLiteral });
    }
    for m in p.null_empty.find_iter(text) {
        out.push(ProtectedSpan { span: Span { start: m.start(), end: m.end() }, kind: ProtectedKind::NullVsEmpty });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_paths_and_line_numbers() {
        let text = "error at src/auth/jwt.rs:128 in verify()";
        let spans = detect_protected_spans(text);
        assert!(spans.iter().any(|s| s.kind == ProtectedKind::Path
            && &text[s.span.start..s.span.end] == "src/auth/jwt.rs"));
        assert!(spans.iter().any(|s| s.kind == ProtectedKind::LineNumber));
    }

    #[test]
    fn detects_numeric_literals_and_error_codes() {
        let text = "listen on port 8080, retries=3, code E0277";
        let spans = detect_protected_spans(text);
        assert!(spans.iter().filter(|s| s.kind == ProtectedKind::NumericLiteral).count() >= 2);
        assert!(spans.iter().any(|s| s.kind == ProtectedKind::ErrorCode));
    }

    #[test]
    fn empty_text_has_no_spans() {
        assert!(detect_protected_spans("").is_empty());
    }
}
