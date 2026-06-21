//! Opt-in LOSSY code skeletonization — Cull's answer to the dominant token sink in coding-agent
//! context. Read-type operations (mostly source-file reads) are ~67–76% of an agent's total tokens
//! (SWE-Pruner, ACL 2026), yet a structure-aware skeleton — signatures, types, fields, imports, and
//! doc comments KEPT, function/method BODIES dropped — preserves the navigational information an
//! agent needs while removing the bulk. SWE-Pruner reports 23–54% token reduction at <1% accuracy
//! loss with this shape. Reversible: the agent re-reads the file to recover any elided body.
//!
//! Tree-sitter based, reusing the same grammars [`crate::code`] already vendors (rust, python, js,
//! ts, go). Language-generic: we elide the `block`/`statement_block` body of any FUNCTION-like node,
//! which leaves class/impl/trait/module bodies (and therefore every method signature) intact.

use tree_sitter::Parser;

/// Body node kinds we elide (the implementation block of a function/method).
const BODY_KINDS: &[&str] = &["block", "statement_block"];

/// Parent kinds that mark a body as a function/method implementation — NOT a class/impl/module block
/// (those we keep, so the signatures inside them survive). Covers rust/python/js/ts/go shapes.
const FUNCTION_KINDS: &[&str] = &[
    "function_item",                  // rust
    "function_definition",            // python, c/cpp
    "function_declaration",           // js, go, ts
    "method_definition",              // js/ts class methods
    "method_declaration",             // go, ts interfaces
    "arrow_function",                 // js/ts
    "function_expression",            // js
    "generator_function",             // js
    "generator_function_declaration", // js
    "closure_expression",             // rust closures `|..| { .. }`
];

/// Skip eliding bodies shorter than this (keep trivial fns whole; the marker isn't worth it).
const MIN_BODY_LINES: usize = 3;

/// Skeletonize source `text` for a file at `path`: keep declarations/signatures/types/imports/docs,
/// replace each function or method body with a compact `… N lines elided` marker. Returns `None`
/// when the language is unknown, nothing is elidable, or the result isn't smaller.
pub fn skeletonize(text: &str, path: &str) -> Option<String> {
    let lang = crate::code::lang_for_path(path)?;
    let mut parser = Parser::new();
    parser.set_language(&lang).ok()?;
    let tree = parser.parse(text, None)?;
    let had_errors = tree.root_node().has_error();

    // Collect OUTERMOST function-body byte ranges (don't descend into an elided body, so a nested
    // closure's body inside an elided function isn't double-counted).
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        let is_fn_body = BODY_KINDS.contains(&node.kind())
            && node
                .parent()
                .is_some_and(|p| FUNCTION_KINDS.contains(&p.kind()));
        if is_fn_body {
            let (s, e) = (node.start_byte(), node.end_byte());
            if text[s..e].bytes().filter(|&b| b == b'\n').count() + 1 >= MIN_BODY_LINES {
                ranges.push((s, e));
                continue; // do not descend into the body we're about to elide
            }
        }
        let mut c = node.walk();
        for child in node.children(&mut c) {
            stack.push(child);
        }
    }
    if ranges.is_empty() {
        return None;
    }
    ranges.sort_by_key(|r| r.0);

    // Reconstruct: copy the gaps (signatures, types, imports, comments) verbatim; replace each body
    // with a marker — but only when the marker is actually shorter than the body it replaces.
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for (s, e) in ranges {
        if s < cursor {
            continue; // overlap guard (shouldn't happen given the outermost-only walk)
        }
        out.push_str(&text[cursor..s]);
        let body = &text[s..e];
        let nl = body.bytes().filter(|&b| b == b'\n').count();
        // `hidden` = lines of SOURCE the body occupied. Brace bodies keep `{`/`}` in the marker, so
        // the hidden interior is nl-1; indentation-delimited (python) bodies are the nl+1 statements.
        let brace = body.starts_with('{');
        let hidden = if brace { nl.saturating_sub(1) } else { nl + 1 };
        // `cull:` sentinel — greppable and unambiguous (not a bare `...` that reads as real code).
        let marker = if brace {
            format!("{{ /* cull: {hidden} lines hidden */ }}")
        } else {
            format!("# cull: {hidden} lines hidden")
        };
        // The MIN_BODY_LINES gate skips trivial bodies; this length guard is the final safety net —
        // never emit a marker larger than the body it replaces.
        if marker.len() < body.len() {
            out.push_str(&marker);
        } else {
            out.push_str(body);
        }
        cursor = e;
    }
    out.push_str(&text[cursor..]);

    // Decide worth-it on the SKELETON itself (the note below is metadata, not content — it must not
    // be able to flip a real saving into "no saving" on small files).
    if out.len() >= text.len() {
        return None;
    }
    // If the file didn't fully parse, flag it: some bodies were kept for a structural reason
    // (incomplete parse), not because they were trivial — the agent shouldn't trust them as stubs.
    if had_errors {
        let cmt = if path.rsplit('.').next() == Some("py") {
            "#"
        } else {
            "//"
        };
        return Some(format!(
            "{cmt} [cull: parse errors; skeleton may be incomplete]\n{out}"
        ));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_keeps_signatures_and_types_drops_bodies() {
        let src = "\
use std::collections::HashMap;

/// Verifies a token.
pub struct Auth { pub key: String, pub ttl: u64 }

pub fn verify_token(input: AuthToken) -> Result<Claims> {
    let decoded = decode_jwt(input);
    let claims = validate(decoded);
    Ok(claims)
}
";
        let out = skeletonize(src, "auth/jwt.rs").expect("should skeletonize");
        assert!(out.len() < src.len());
        // kept: import, doc comment, struct + fields, fn signature
        assert!(out.contains("use std::collections::HashMap;"));
        assert!(out.contains("/// Verifies a token."));
        assert!(out.contains("pub struct Auth { pub key: String, pub ttl: u64 }"));
        assert!(out.contains("pub fn verify_token(input: AuthToken) -> Result<Claims>"));
        // dropped: body internals + elision marker present
        assert!(!out.contains("decode_jwt"), "body should be elided: {out}");
        // accurate hidden-line count (3 interior statements) + greppable sentinel
        assert!(
            out.contains("cull: 3 lines hidden"),
            "accurate sentinel marker: {out}"
        );
    }

    #[test]
    fn python_keeps_class_and_def_signatures_drops_bodies() {
        let src = "\
import os

class Service:
    def authenticate(self, user):
        token = user.token
        verified = verify_password(token)
        return verified
";
        let out = skeletonize(src, "svc.py").expect("should skeletonize");
        assert!(out.contains("import os"));
        assert!(out.contains("class Service:"));
        assert!(out.contains("def authenticate(self, user):"));
        assert!(!out.contains("verify_password"), "body elided: {out}");
        assert!(
            out.contains("# cull:") && out.contains("lines hidden"),
            "python sentinel marker: {out}"
        );
    }

    #[test]
    fn typescript_drops_method_and_function_bodies() {
        let src = "\
export interface User { id: number; name: string; }

export function loadUser(id: number): User {
    const row = db.query(id);
    const user = mapRow(row);
    return user;
}
";
        let out = skeletonize(src, "user.ts").expect("should skeletonize");
        assert!(out.contains("export interface User { id: number; name: string; }"));
        assert!(out.contains("export function loadUser(id: number): User"));
        assert!(!out.contains("db.query"), "body elided: {out}");
        assert!(
            out.contains("cull: 3 lines hidden"),
            "accurate count for TS: {out}"
        );
    }

    #[test]
    fn go_drops_func_bodies_keeps_signature() {
        let src = "\
package main

func Add(a int, b int) int {
\tsum := a + b
\tlogResult(sum)
\treturn sum
}
";
        let out = skeletonize(src, "math.go").expect("should skeletonize");
        assert!(out.contains("package main"));
        assert!(out.contains("func Add(a int, b int) int"));
        assert!(!out.contains("logResult"), "body elided: {out}");
        assert!(
            out.contains("cull: 3 lines hidden"),
            "accurate count for Go: {out}"
        );
    }

    #[test]
    fn unknown_language_returns_none() {
        assert!(skeletonize("some plain text\nwith lines\n", "notes.txt").is_none());
    }

    #[test]
    fn tiny_bodies_are_left_whole() {
        // every fn body is < MIN_BODY_LINES -> nothing elided -> None
        let src = "fn a() -> i32 { 1 }\nfn b() -> i32 { 2 }\n";
        assert!(skeletonize(src, "x.rs").is_none());
    }

    #[test]
    fn rust_elides_standalone_closure_body() {
        // a module-level closure (not inside an elided fn) — its block body must be dropped too
        let src = "static H: fn(i32) -> i32 = |x: i32| {\n    let a = x + 1;\n    let b = a * 2;\n    a + b\n};\n";
        let out = skeletonize(src, "h.rs").expect("should skeletonize");
        assert!(out.contains("static H"), "binding kept: {out}");
        assert!(!out.contains("let a = x + 1"), "closure body elided: {out}");
    }

    #[test]
    fn parse_errors_get_a_warning_note() {
        // one valid fn (elidable) + one truncated fn (missing close brace) -> file has parse errors
        let src = "fn ok() -> i32 {\n    let a = 1;\n    let b = 2;\n    a + b\n}\nfn broken() -> i32 {\n    let z = 1\n";
        let out = skeletonize(src, "b.rs").expect("elides the valid fn");
        assert!(
            out.contains("[cull: parse errors"),
            "warns about incomplete parse: {out}"
        );
    }
}
