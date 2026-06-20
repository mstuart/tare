# Cull — Plan 23: Predicate Pushdown (D1)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]` checkboxes.

**Goal:** Close §7 D1 — rule-based predicate pushdown: narrow an over-broad tool call's arguments using the task, BEFORE execution, so large outputs never enter context.

**Architecture:** A pure function `narrow_tool_call(name, input, task) -> input'` in a new `cull-core::predicate` module. Conservative + rule-based (no model call): for repo-wide searches (`grep`/`rg`/`find`/`search`) whose scope is `.`/missing, narrow the scope to a task-relevant path symbol. Returns the input unchanged otherwise. **Integration point is a tool-execution hook** (e.g. a Claude Code PreToolUse hook) — the model-boundary proxy can't rewrite tool calls, so this ships as a reusable function, not wired into the proxy. (Documented in the ledger.)

**Tech:** Rust, serde_json. Builds on Plan 4 (`TaskSignal`). Reference: spec §7 D1.

---

### Task 1: narrow_tool_call

**Files:** create `crates/cull-core/src/predicate.rs`; modify `lib.rs` (`pub mod predicate;`); tests inline.

- [ ] **Step 1 — failing test.**
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::TaskSignal;
    use serde_json::json;

    #[test]
    fn narrows_repo_wide_grep_to_task_path() {
        let task = TaskSignal::from_text("fix the auth bug in src/auth");
        let input = json!({"query": "TODO", "path": "."});
        let out = narrow_tool_call("grep", &input, &task);
        assert_eq!(out["path"], "src/auth");        // narrowed
        assert_eq!(out["query"], "TODO");           // other args preserved
    }

    #[test]
    fn leaves_already_scoped_or_non_search_calls_unchanged() {
        let task = TaskSignal::from_text("work in src/auth");
        let scoped = json!({"query":"x","path":"src/db"});
        assert_eq!(narrow_tool_call("grep", &scoped, &task), scoped); // already scoped
        let read = json!({"path":"."});
        assert_eq!(narrow_tool_call("read_file", &read, &task), read); // not a search
        let no_task = json!({"query":"x","path":"."});
        assert_eq!(narrow_tool_call("grep", &no_task, &TaskSignal::empty()), no_task); // no task path
    }
}
```

- [ ] **Step 2 — confirm FAIL.**

- [ ] **Step 3 — implement.**
```rust
use serde_json::Value;
use crate::task::TaskSignal;

/// Rule-based predicate pushdown (spec §7 D1): narrow an over-broad search tool call using the
/// task, BEFORE execution. Conservative — only narrows repo-wide (`.`/missing scope) searches to a
/// task-relevant path symbol; everything else is returned unchanged. Intended for a tool-execution
/// hook (the model-boundary proxy cannot rewrite tool calls).
pub fn narrow_tool_call(name: &str, input: &Value, task: &TaskSignal) -> Value {
    let mut out = input.clone();
    let l = name.to_ascii_lowercase();
    let is_search = ["grep", "rg", "ripgrep", "find", "search"].iter().any(|s| l.contains(s));
    if !is_search { return out; }

    let scope = out.get("path").or_else(|| out.get("dir")).or_else(|| out.get("scope"))
        .and_then(Value::as_str);
    let repo_wide = matches!(scope, None | Some(".") | Some("./"));
    if !repo_wide { return out; }

    // pick the most specific path-like task symbol (has a '/')
    if let Some(p) = task.symbols.iter().filter(|s| s.contains('/')).max_by_key(|s| s.len()) {
        if let Some(obj) = out.as_object_mut() {
            obj.insert("path".to_string(), Value::String(p.clone()));
        }
    }
    out
}
```

- [ ] **Step 4 — confirm PASS** (`cargo test -p cull-core predicate`).

- [ ] **Step 5 — wire + commit.** Add `pub mod predicate;` and `pub use predicate::narrow_tool_call;` to `lib.rs`. `cargo test --workspace` green; `git add crates/cull-core && git commit -m "feat(core): predicate-pushdown narrow_tool_call (D1) for tool-execution hooks"`

---

## After this plan — ledger
- ✅ §7 D1 — `narrow_tool_call` built; integration point is a tool-execution hook (documented; not wired into the model-boundary proxy, which is the wrong boundary for rewriting tool calls).

## Self-Review
- Pure, conservative, rule-based; non-search or already-scoped calls returned unchanged → safe. ✓
- Picks the longest path-like task symbol (most specific scope). ✓
- No proxy wiring (correct: proxy is at the model boundary; D1 belongs at the tool boundary). ✓
