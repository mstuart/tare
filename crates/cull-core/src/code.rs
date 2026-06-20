use std::collections::HashSet;
use tree_sitter::{Language, Parser};
use crate::task::extract_symbols;

fn lang_for_path(path: &str) -> Option<Language> {
    let ext = path.rsplit('.').next()?;
    let lang: Language = match ext {
        "rs" => tree_sitter_rust::LANGUAGE.into(),
        "py" => tree_sitter_python::LANGUAGE.into(),
        "js" | "jsx" | "mjs" | "cjs" => tree_sitter_javascript::LANGUAGE.into(),
        "ts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        "go" => tree_sitter_go::LANGUAGE.into(),
        _ => return None,
    };
    Some(lang)
}

/// Collect the text of every named node whose kind contains "identifier" (cross-language).
fn ts_symbols(text: &str, lang: Language) -> HashSet<String> {
    let mut parser = Parser::new();
    if parser.set_language(&lang).is_err() { return HashSet::new(); }
    let Some(tree) = parser.parse(text, None) else { return HashSet::new(); };
    let mut out = HashSet::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.is_named() && node.kind().contains("identifier") {
            if let Ok(s) = node.utf8_text(text.as_bytes()) {
                if s.len() >= 3 { out.insert(s.to_ascii_lowercase()); }
            }
        }
        let mut c = node.walk();
        for child in node.children(&mut c) { stack.push(child); }
    }
    out
}

/// Path-aware symbol extraction: tree-sitter for known code languages, regex otherwise.
pub fn extract_symbols_for(text: &str, path: Option<&str>) -> HashSet<String> {
    if let Some(p) = path {
        if let Some(lang) = lang_for_path(p) {
            let syms = ts_symbols(text, lang);
            if !syms.is_empty() { return syms; }
        }
    }
    extract_symbols(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_rust_identifiers_via_treesitter() {
        let src = "fn verify_token(input: AuthToken) -> Result<Claims> { decode_jwt(input) }";
        let syms = extract_symbols_for(src, Some("auth/jwt.rs"));
        assert!(syms.contains("verify_token"));
        assert!(syms.contains("decode_jwt"));
        assert!(syms.contains("authtoken") || syms.contains("AuthToken".to_lowercase().as_str()));
    }

    #[test]
    fn extracts_python_identifiers() {
        let src = "def authenticate(user):\n    return verify_password(user.token)";
        let syms = extract_symbols_for(src, Some("auth.py"));
        assert!(syms.contains("authenticate"));
        assert!(syms.contains("verify_password"));
    }

    #[test]
    fn unknown_extension_falls_back_to_regex() {
        let syms = extract_symbols_for("port 8080 listen retries", Some("notes.txt"));
        assert!(syms.contains("listen") || syms.contains("retries"));
    }
}
