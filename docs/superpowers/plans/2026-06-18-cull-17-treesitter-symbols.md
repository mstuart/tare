# Cull — Plan 17: Tree-sitter Symbol Resolution (B1, part 2 → ✅)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]` checkboxes.

**Goal:** Replace regex symbol extraction with **tree-sitter AST** symbol resolution for code segments (the mechanism spec §7 B1 names). Combined with Plan 16's transitive slice, this completes B1 → ✅.

**Architecture:** A `code` module in `cull-core`: `lang_for_path(path)` maps a file extension to a tree-sitter `Language` (Rust/Python/JS/TS/Go to start; more are one-line additions). `ts_symbols(text, lang)` parses and collects the text of every named node whose kind contains `identifier` (function/type/field/property identifiers — cross-language). `extract_symbols_for(text, path)` uses tree-sitter when the path has a known language (and parsing yields symbols), else falls back to the existing regex `extract_symbols`. `RelevancePass` calls the path-aware extractor.

**Tech:** Rust, `tree-sitter` 0.24 + `tree-sitter-rust`/`-python`/`-javascript`/`-typescript`/`-go`. Builds on Plans 4/16. Reference: spec §7 B1.

> **Version note:** the tree-sitter grammar crates' API (`LANGUAGE` const vs `language()` fn, `Language`/`LanguageFn` types, `Parser::set_language`) drifts by version. Pin a mutually compatible set; if a particular grammar won't build, DROP that language and report it — the mechanism is done with whatever subset compiles (Rust + Python + JS at minimum).

---

### Task 1: Tree-sitter symbol extractor

**Files:** `crates/cull-core/Cargo.toml`; create `crates/cull-core/src/code.rs`; modify `lib.rs` (`pub mod code;`); tests inline.

- [ ] **Step 1 — deps.** Add to `crates/cull-core/Cargo.toml` `[dependencies]` (pin compatible versions; adjust if needed):
```toml
tree-sitter = "0.24"
tree-sitter-rust = "0.23"
tree-sitter-python = "0.23"
tree-sitter-javascript = "0.23"
tree-sitter-typescript = "0.23"
tree-sitter-go = "0.23"
```

- [ ] **Step 2 — failing test.** In `crates/cull-core/src/code.rs`:
```rust
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
```

- [ ] **Step 3 — implement.** Above the tests:
```rust
use std::collections::HashSet;
use tree_sitter::{Parser, Language};
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
```
(If a grammar crate's API differs — e.g. `tree_sitter_rust::language()` returning `Language` directly instead of a `LANGUAGE` const — use that form. If a grammar won't compile at all, remove it from `Cargo.toml` and the match arm, and note it.)

- [ ] **Step 4 — confirm PASS** (`cargo test -p cull-core code`).

- [ ] **Step 5 — commit.** `git add crates/cull-core && git commit -m "feat(core): tree-sitter AST symbol extraction (rust/python/js/ts/go) with regex fallback"`

---

### Task 2: Use tree-sitter symbols in the relevance slice

**Files:** `crates/cull-core/src/passes/relevance.rs`; test inline.

- [ ] **Step 1 — failing test.** Add:
```rust
    #[test]
    fn relevance_uses_treesitter_symbols_for_code_paths() {
        // task references a function name; a code file defining it (path .rs) must be kept,
        // an unrelated code file dropped — symbols come from the AST, not loose words.
        let task = TaskSignal::from_text("fix validate_session");
        let mut a = seg(0, 0, SegmentKind::FileRead, "fn validate_session(t: Token) { check(t) }");
        a.origin.path = Some("session.rs".into());
        let mut b = seg(1, 1, SegmentKind::FileRead, "fn render_button() { draw() }");
        b.origin.path = Some("ui.rs".into());
        let plan = Planner::new(vec![Box::new(RelevancePass { recency_keep: 0 })])
            .plan_with_task(&[a, b], &SessionState::default(), &task);
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
        assert_eq!(plan.entries[1].action, SegmentAction::Drop(DropReason::IrrelevantBySlice));
    }
```

- [ ] **Step 2 — confirm FAIL/PASS.** (May already pass via regex if `validate_session` is also a regex token; the point is it must use the path. To force tree-sitter use, the implementation in Step 3 switches `extract_symbols` → `extract_symbols_for` with the segment path.)

- [ ] **Step 3 — implement.** In `RelevancePass::propose`, change the per-segment symbol extraction to be path-aware:
```rust
        let seg_syms: Vec<std::collections::HashSet<String>> = ctx.segments.iter()
            .map(|s| crate::code::extract_symbols_for(
                &String::from_utf8_lossy(&s.bytes), s.origin.path.as_deref()))
            .collect();
```
(import or fully-qualify `crate::code::extract_symbols_for`.)

- [ ] **Step 4 — confirm PASS** (`cargo test -p cull-core relevance` + new test; all prior relevance tests still pass).

- [ ] **Step 5 — workspace + commit.** `cargo test --workspace` green; `git add crates/cull-core && git commit -m "feat(core): relevance slice uses tree-sitter symbols for code segments (B1 complete)"`

---

## After this plan — ledger
- ✅ B1 — tree-sitter symbol resolution + transitive slice. (Note any languages dropped due to build issues — the mechanism is done; language coverage is extensible.)

## Self-Review
- Falls back to regex for unknown extensions / unparseable code / no path → no regression. ✓
- Tree-sitter only refines symbol extraction; the slice algorithm (Plan 16) is unchanged → existing relevance tests hold. ✓
- Identifier-kind heuristic is cross-language (works for any grammar with `*identifier*` node kinds). ✓
