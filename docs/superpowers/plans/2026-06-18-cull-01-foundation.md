# Cull — Plan 1: Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the Cull Rust workspace and the engine's foundation — typed, token-counted, protected-span-annotated segments plus the session-state scaffold — as a working, tested library.

**Architecture:** A Cargo workspace of focused crates. This plan delivers `cull-core` (data model + segmenter + session-state types) and `cull-tokenize` (provider-aware token counting). No compression passes yet — those build on these types in Plans 2–6. Everything here is pure (no network/I/O) and unit-tested.

**Tech Stack:** Rust (stable), `tiktoken-rs` (token approximation), `regex` (protected-span detection), `xxhash-rust` (prefix commitment hash), `serde` (types). Reference: spec `docs/superpowers/specs/2026-06-18-cull-design.md` §6 (data model), §7 (segment kinds), §8 (mutation classes), §9 (exact-token classes).

---

## File Structure

```
cull/
  Cargo.toml                      # workspace manifest (all 6 member crates)
  crates/
    cull-core/
      Cargo.toml
      src/
        lib.rs                    # re-exports
        segment.rs                # Segment + enums (Role, SegmentKind, MutationClass, TaskPhase)
        protected.rs              # ProtectedSpan + detect_protected_spans()
        segmenter.rs              # RawBlock -> Vec<Segment>
        session.rs                # session-state skeleton structs
    cull-tokenize/
      Cargo.toml
      src/lib.rs                  # TokenCounter trait + ApproxCounter
    cull-cache/   (stub this plan; filled in Plan 2)
    cull-proxy/   (stub; Plan 7)
    cull-bench/   (stub; Plan 8)
    cull-cli/     (stub; Plan 9)
```

Split rationale: types/segmenter/session change together (one domain → `cull-core`); token counting is an independent capability with its own dependency (`tiktoken-rs`) and is consumed by core → its own crate. The four later crates are created as empty stubs now so the workspace structure is locked.

---

### Task 0: Workspace skeleton

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/cull-core/Cargo.toml`, `crates/cull-core/src/lib.rs`
- Create: `crates/cull-tokenize/Cargo.toml`, `crates/cull-tokenize/src/lib.rs`
- Create stubs: `crates/cull-cache/{Cargo.toml,src/lib.rs}`, `cull-proxy`, `cull-bench`, `cull-cli`

- [ ] **Step 1: Create the workspace manifest**

Create `Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = [
    "crates/cull-core",
    "crates/cull-tokenize",
    "crates/cull-cache",
    "crates/cull-proxy",
    "crates/cull-bench",
    "crates/cull-cli",
]

[workspace.package]
edition = "2021"
license = "MIT"
repository = "https://github.com/<owner>/cull"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
regex = "1"
xxhash-rust = { version = "0.8", features = ["xxh3"] }
tiktoken-rs = "0.6"
```

- [ ] **Step 2: Create each crate's Cargo.toml + empty lib**

`crates/cull-core/Cargo.toml`:

```toml
[package]
name = "cull-core"
version = "0.0.0"
edition.workspace = true

[dependencies]
serde = { workspace = true }
regex = { workspace = true }
xxhash-rust = { workspace = true }
```

`crates/cull-tokenize/Cargo.toml`:

```toml
[package]
name = "cull-tokenize"
version = "0.0.0"
edition.workspace = true

[dependencies]
tiktoken-rs = { workspace = true }
```

For each of `cull-cache`, `cull-proxy`, `cull-bench`, `cull-cli`, create a `Cargo.toml` with `name` set accordingly, `version = "0.0.0"`, `edition.workspace = true`, and **no** dependencies yet. Create each `src/lib.rs` containing only:

```rust
// stub — implemented in a later plan
```

Put the same stub line in `crates/cull-core/src/lib.rs` and `crates/cull-tokenize/src/lib.rs` for now.

- [ ] **Step 3: Verify the workspace builds**

Run: `cargo build --workspace`
Expected: PASS (compiles all six empty crates).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates
git commit -m "chore: scaffold cull cargo workspace (6 crates)"
```

---

### Task 1: Core segment types

**Files:**
- Create: `crates/cull-core/src/segment.rs`
- Modify: `crates/cull-core/src/lib.rs`
- Test: inline `#[cfg(test)]` module in `segment.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/cull-core/src/segment.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_class_follows_kind() {
        assert_eq!(MutationClass::for_kind(&SegmentKind::SystemPrompt), MutationClass::Frozen);
        assert_eq!(MutationClass::for_kind(&SegmentKind::ToolSchema), MutationClass::Slow);
        assert_eq!(
            MutationClass::for_kind(&SegmentKind::ToolOutput { class: "cargo-test".into() }),
            MutationClass::Fast
        );
        assert_eq!(MutationClass::for_kind(&SegmentKind::ConversationTurn), MutationClass::Fast);
    }

    #[test]
    fn segment_round_trips_via_serde() {
        let s = Segment {
            id: SegmentId(1),
            kind: SegmentKind::FileRead,
            role: Role::Tool,
            bytes: b"hello".to_vec(),
            token_count: 1,
            position: 0,
            mutation_class: MutationClass::Fast,
            origin: Origin::default(),
            protected_spans: vec![],
            refs: RefLedger::default(),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Segment = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, SegmentId(1));
        assert_eq!(back.bytes, b"hello");
    }
}
```

Add `serde_json = "1"` to `crates/cull-core/Cargo.toml` under `[dev-dependencies]`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cull-core segment::`
Expected: FAIL — `Segment`, `SegmentKind`, etc. not defined.

- [ ] **Step 3: Write the types**

Replace `crates/cull-core/src/segment.rs` (above the test module) with:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SegmentId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role { System, User, Assistant, Tool }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SegmentKind {
    SystemPrompt,
    ToolSchema,
    FileRead,
    DirListing,
    Diff,
    ToolOutput { class: String }, // e.g. "cargo-test", "git-status", "grep"
    StackTrace,
    TestOutput,
    ReasoningTrace,
    ConversationTurn,
    CompactSummary,
}

/// Cache-stability class. Drives stability-ordered segmentation (spec §8 Rule 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MutationClass { Frozen, Slow, Fast }

impl MutationClass {
    pub fn for_kind(kind: &SegmentKind) -> MutationClass {
        match kind {
            SegmentKind::SystemPrompt => MutationClass::Frozen,
            SegmentKind::ToolSchema => MutationClass::Slow,
            SegmentKind::CompactSummary => MutationClass::Slow,
            _ => MutationClass::Fast,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskPhase { Discovery, Planning, Execution, Verification }

impl Default for TaskPhase { fn default() -> Self { TaskPhase::Discovery } }

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefLedger {
    pub recency: usize,   // turns since last referenced
    pub frequency: u32,   // times referenced
    pub phase: TaskPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span { pub start: usize, pub end: usize } // byte offsets, half-open

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub turn: usize,
    pub tool: Option<String>,
    pub path: Option<String>,
    pub byte_range: Option<Span>,
    pub mtime: Option<u64>,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub id: SegmentId,
    pub kind: SegmentKind,
    pub role: Role,
    pub bytes: Vec<u8>,            // exact original content; never mutated if kept (spec I2)
    pub token_count: u32,
    pub position: usize,
    pub mutation_class: MutationClass,
    pub origin: Origin,
    pub protected_spans: Vec<crate::protected::ProtectedSpan>,
    pub refs: RefLedger,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cull-core segment::`
Expected: PASS (both tests).

- [ ] **Step 5: Wire up lib.rs and commit**

Set `crates/cull-core/src/lib.rs` to:

```rust
pub mod segment;
pub mod protected;
pub mod segmenter;
pub mod session;

pub use segment::*;
```

(`protected`, `segmenter`, `session` modules are added in the next tasks; create empty files `protected.rs`, `segmenter.rs`, `session.rs` now so the crate compiles — each containing `// implemented below`.)

Run: `cargo build -p cull-core` → Expected: PASS.

```bash
git add crates/cull-core
git commit -m "feat(core): segment data model with mutation classes"
```

---

### Task 2: Protected exact-token-class detection

**Files:**
- Create/replace: `crates/cull-core/src/protected.rs`
- Test: inline `#[cfg(test)]` in `protected.rs`

Spec §9 I4: file paths, line numbers, error codes, numeric literals, null-vs-empty must never be dropped or mutated. This task only *detects and records* them; enforcement lands with the passes (Plan 2+).

- [ ] **Step 1: Write the failing test**

Put in `crates/cull-core/src/protected.rs`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cull-core protected::`
Expected: FAIL — `detect_protected_spans` / `ProtectedKind` not defined.

- [ ] **Step 3: Implement detection**

Above the test module in `protected.rs`:

```rust
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
        // path-like: one or more segments with a dot+extension
        path: Regex::new(r"(?:[\w.-]+/)+[\w.-]+\.\w+|/[\w./-]+").unwrap(),
        line_no: Regex::new(r":(\d+)(?::\d+)?\b").unwrap(),
        error_code: Regex::new(r"\b[EC]\d{3,5}\b").unwrap(),
        number: Regex::new(r"\b\d+\b").unwrap(),
        null_empty: Regex::new(r#"\bnull\b|""|''"#).unwrap(),
    })
}

/// Detect exact-token classes that must never be dropped or mutated (spec §9 I4).
/// Spans may overlap (a path may contain numbers); downstream treats the union as protected.
pub fn detect_protected_spans(text: &str) -> Vec<ProtectedSpan> {
    let p = patterns();
    let mut out = Vec::new();
    let mut push = |m: regex::Match, kind: ProtectedKind| {
        out.push(ProtectedSpan { span: Span { start: m.start(), end: m.end() }, kind });
    };
    for m in p.path.find_iter(text) { push(m, ProtectedKind::Path); }
    for m in p.line_no.find_iter(text) { push(m, ProtectedKind::LineNumber); }
    for m in p.error_code.find_iter(text) { push(m, ProtectedKind::ErrorCode); }
    for m in p.number.find_iter(text) { push(m, ProtectedKind::NumericLiteral); }
    for m in p.null_empty.find_iter(text) { push(m, ProtectedKind::NullVsEmpty); }
    out
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cull-core protected::`
Expected: PASS (all three).

Note: the `error_code` regex (`[EC]\d{3,5}`) may also match inside paths; that is acceptable — overlap is fine because the union is protected. If a test for a specific code value fails due to overlap, prefer widening protection, never narrowing.

- [ ] **Step 5: Commit**

```bash
git add crates/cull-core
git commit -m "feat(core): protected exact-token-class detection"
```

---

### Task 3: Token counting (`cull-tokenize`)

**Files:**
- Replace: `crates/cull-tokenize/src/lib.rs`
- Test: inline `#[cfg(test)]`

- [ ] **Step 1: Write the failing test**

Put in `crates/cull-tokenize/src/lib.rs`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cull-tokenize`
Expected: FAIL — `ApproxCounter` not defined.

- [ ] **Step 3: Implement the counter**

Above the test module:

```rust
use tiktoken_rs::{o200k_base, CoreBPE};

/// Counts tokens for budgeting/segmentation. Exact provider counts (e.g. Anthropic
/// `count_tokens`) are added in Plan 7; this approximation is deterministic and offline.
pub trait TokenCounter: Send + Sync {
    fn count(&self, text: &str) -> usize;
}

pub struct ApproxCounter { bpe: CoreBPE }

impl ApproxCounter {
    /// o200k_base — the modern BPE; a stable approximation across providers.
    pub fn o200k() -> Self {
        Self { bpe: o200k_base().expect("o200k_base BPE must load") }
    }
}

impl TokenCounter for ApproxCounter {
    fn count(&self, text: &str) -> usize {
        if text.is_empty() { return 0; }
        self.bpe.encode_with_special_tokens(text).len()
    }
}
```

(Executor note: confirm the exact `tiktoken-rs` 0.6 API — function name `o200k_base` and method `encode_with_special_tokens`. If the crate version differs, adapt the call but keep the `TokenCounter` trait surface identical.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cull-tokenize`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/cull-tokenize
git commit -m "feat(tokenize): provider-aware token counter trait + tiktoken approximation"
```

---

### Task 4: Segmenter

**Files:**
- Replace: `crates/cull-core/src/segmenter.rs`
- Modify: `crates/cull-core/Cargo.toml` (add `cull-tokenize` path dep)
- Test: inline `#[cfg(test)]`

The segmenter turns a list of raw input blocks (role + kind + text — the proxy will map provider requests into these in Plan 7) into fully-populated `Segment`s: token counts, mutation classes, protected spans, sequential positions/ids.

- [ ] **Step 1: Add the dependency**

In `crates/cull-core/Cargo.toml` `[dependencies]`, add:

```toml
cull-tokenize = { path = "../cull-tokenize" }
```

- [ ] **Step 2: Write the failing test**

Put in `crates/cull-core/src/segmenter.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::*;
    use cull_tokenize::ApproxCounter;

    fn raw(role: Role, kind: SegmentKind, text: &str) -> RawBlock {
        RawBlock { role, kind, text: text.to_string() }
    }

    #[test]
    fn assigns_ids_positions_counts_and_classes() {
        let counter = ApproxCounter::o200k();
        let blocks = vec![
            raw(Role::System, SegmentKind::SystemPrompt, "You are an agent."),
            raw(Role::Tool, SegmentKind::FileRead, "fn main() {} // src/main.rs:1"),
        ];
        let segs = segment(&blocks, &counter);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].id, SegmentId(0));
        assert_eq!(segs[1].id, SegmentId(1));
        assert_eq!(segs[0].position, 0);
        assert_eq!(segs[1].position, 1);
        assert_eq!(segs[0].mutation_class, MutationClass::Frozen);
        assert_eq!(segs[1].mutation_class, MutationClass::Fast);
        assert!(segs[0].token_count > 0);
        // protected spans detected in the file read
        assert!(!segs[1].protected_spans.is_empty());
        // bytes are exact
        assert_eq!(segs[1].bytes, b"fn main() {} // src/main.rs:1");
    }

    #[test]
    fn empty_input_yields_no_segments() {
        let counter = ApproxCounter::o200k();
        assert!(segment(&[], &counter).is_empty());
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p cull-core segmenter::`
Expected: FAIL — `RawBlock` / `segment` not defined.

- [ ] **Step 4: Implement the segmenter**

Above the test module:

```rust
use crate::protected::detect_protected_spans;
use crate::segment::*;
use cull_tokenize::TokenCounter;

/// Raw input unit (a provider message/content block, pre-classified). The proxy
/// produces these from Anthropic/OpenAI requests in Plan 7.
#[derive(Debug, Clone)]
pub struct RawBlock {
    pub role: Role,
    pub kind: SegmentKind,
    pub text: String,
}

/// Turn raw blocks into fully-populated segments: sequential ids/positions,
/// token counts, mutation classes, and protected-span annotations.
pub fn segment(blocks: &[RawBlock], counter: &dyn TokenCounter) -> Vec<Segment> {
    blocks
        .iter()
        .enumerate()
        .map(|(i, b)| Segment {
            id: SegmentId(i as u64),
            kind: b.kind.clone(),
            role: b.role,
            token_count: counter.count(&b.text) as u32,
            position: i,
            mutation_class: MutationClass::for_kind(&b.kind),
            protected_spans: detect_protected_spans(&b.text),
            origin: Origin { turn: i, ..Origin::default() },
            bytes: b.text.clone().into_bytes(),
            refs: RefLedger::default(),
        })
        .collect()
}
```

- [ ] **Step 5: Run test to verify it passes, then commit**

Run: `cargo test -p cull-core segmenter::`
Expected: PASS.

```bash
git add crates/cull-core
git commit -m "feat(core): segmenter producing typed, counted, protected segments"
```

---

### Task 5: Session-state skeleton

**Files:**
- Replace: `crates/cull-core/src/session.rs`
- Test: inline `#[cfg(test)]`

Types only — the logic that mutates these lands with the passes (Plans 3–5). This locks the interfaces the passes will share (spec §6 "Session state").

- [ ] **Step 1: Write the failing test**

Put in `crates/cull-core/src/session.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_store_inserts_and_reads() {
        let mut store = CanonicalFileStore::default();
        store.put("src/a.rs", b"contents".to_vec(), 2);
        let f = store.get("src/a.rs").unwrap();
        assert_eq!(f.version, 0);
        assert_eq!(f.token_count, 2);
        // re-put bumps version, keeps identity
        store.put("src/a.rs", b"new".to_vec(), 1);
        assert_eq!(store.get("src/a.rs").unwrap().version, 1);
    }

    #[test]
    fn prefix_commitment_is_stable_for_same_bytes() {
        let mut c = CachePrefixCommitment::default();
        c.commit(b"frozen-prefix-bytes", 5);
        let h1 = c.frozen_hash;
        c.commit(b"frozen-prefix-bytes", 5);
        assert_eq!(h1, c.frozen_hash);
        c.commit(b"different", 2);
        assert_ne!(h1, c.frozen_hash);
    }

    #[test]
    fn tool_registry_tracks_latest_run() {
        let mut r = ToolClassRegistry::default();
        r.record("cargo-test", 12, Some(1));
        r.record("cargo-test", 31, Some(0));
        let run = r.latest_run("cargo-test").unwrap();
        assert_eq!(run.turn, 31);
        assert_eq!(run.exit_code, Some(0));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cull-core session::`
Expected: FAIL — types not defined.

- [ ] **Step 3: Implement the skeleton**

Above the test module:

```rust
use std::collections::HashMap;
use xxhash_rust::xxh3::xxh3_64;
use crate::segment::{RefLedger, SegmentId};

#[derive(Debug, Clone)]
pub struct CanonicalFile { pub bytes: Vec<u8>, pub token_count: u32, pub version: u32 }

/// File-read IVM baseline store (spec §7 A2): path -> canonical snapshot.
#[derive(Debug, Default)]
pub struct CanonicalFileStore { map: HashMap<String, CanonicalFile> }

impl CanonicalFileStore {
    pub fn get(&self, path: &str) -> Option<&CanonicalFile> { self.map.get(path) }
    pub fn put(&mut self, path: &str, bytes: Vec<u8>, token_count: u32) {
        let version = self.map.get(path).map(|f| f.version + 1).unwrap_or(0);
        self.map.insert(path.to_string(), CanonicalFile { bytes, token_count, version });
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ToolRun { pub turn: usize, pub exit_code: Option<i32> }

/// Supersession registry (spec §7 A1): tool class -> latest run.
#[derive(Debug, Default)]
pub struct ToolClassRegistry { latest: HashMap<String, ToolRun> }

impl ToolClassRegistry {
    pub fn record(&mut self, class: &str, turn: usize, exit_code: Option<i32>) {
        self.latest.insert(class.to_string(), ToolRun { turn, exit_code });
    }
    pub fn latest_run(&self, class: &str) -> Option<ToolRun> { self.latest.get(class).copied() }
}

/// Span reference/recency/phase ledger (spec §7 C1/C2 eviction inputs).
#[derive(Debug, Default)]
pub struct SpanLedger { pub entries: HashMap<SegmentId, RefLedger> }

/// Cache-prefix commitment (spec §8 Rule 1/7): the frozen zone is a content hash.
#[derive(Debug, Default)]
pub struct CachePrefixCommitment { pub frozen_hash: Option<u64>, pub frozen_len_tokens: usize }

impl CachePrefixCommitment {
    pub fn commit(&mut self, frozen_bytes: &[u8], frozen_len_tokens: usize) {
        self.frozen_hash = Some(xxh3_64(frozen_bytes));
        self.frozen_len_tokens = frozen_len_tokens;
    }
}

/// Aggregate per-session engine state (passes in Plans 3–5 read/write these).
#[derive(Debug, Default)]
pub struct SessionState {
    pub files: CanonicalFileStore,
    pub tools: ToolClassRegistry,
    pub spans: SpanLedger,
    pub prefix: CachePrefixCommitment,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cull-core session::`
Expected: PASS (all three).

- [ ] **Step 5: Full workspace test + commit**

Run: `cargo test --workspace`
Expected: PASS (all cull-core + cull-tokenize tests).

```bash
git add crates/cull-core
git commit -m "feat(core): session-state skeleton (canonical store, tool registry, prefix commitment)"
```

---

## Self-Review

**1. Spec coverage (this plan's slice):**
- §6 data model → Tasks 1, 5 (Segment, Origin, RefLedger, session state). ✓
- §7 segment kinds → Task 1 `SegmentKind`. ✓
- §8 Rule 3 mutation classes → Task 1 `MutationClass::for_kind`. ✓ (ordering logic itself is Plan 2's planner.)
- §9 I4 exact-token classes → Task 2 detection. ✓ (enforcement is Plan 2+.)
- §6 token counting → Task 3. ✓
- Passes, planner, economic model, proxy, bench, emitter → **out of scope by design**, covered by Plans 2–9.

**2. Placeholder scan:** No "TBD"/"add error handling"/"write tests for the above". Every step has runnable code/commands. The four later crates are intentional empty stubs (Task 0), not placeholders in this plan's deliverables. ✓

**3. Type consistency:** `Segment.protected_spans: Vec<ProtectedSpan>` (Task 1) matches `ProtectedSpan` (Task 2) and `detect_protected_spans` return (Tasks 2, 4). `RefLedger`/`SegmentId` defined in Task 1, reused in Task 5 `SpanLedger`. `TokenCounter` trait (Task 3) consumed by `segment()` (Task 4) as `&dyn TokenCounter`. `MutationClass::for_kind` signature consistent between Task 1 definition and Task 4 use. ✓

**Outcome:** Foundation is a self-contained, fully tested library. Plan 2 (cache model + planner + invariants) builds directly on `Segment`, `SessionState`, and `CachePrefixCommitment` defined here.
