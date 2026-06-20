# Cull — Plan 27: Embedding-Salience Pass (B3)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]` checkboxes.

**Goal:** Close §7 B3 "embedding / logprob salience" with a real, tested embedding-salience pass — dependency-free, no model download — plus the trait seam for a neural backend.

**Architecture:** An `Embedder` trait + a `HashEmbedder` (the hashing trick: tokens → hashed buckets → L2-normalized term-frequency vector, reusing the existing `xxhash_rust` dep). `EmbeddingSaliencePass<E: Embedder>` embeds the joined task symbols as the query and each droppable, non-recent segment, dropping those whose cosine similarity is below `min_similarity`. It mirrors `RelevancePass` gating (droppable kinds only, recency guard, no-task ⇒ no drops). It is **opt-in** — NOT added to the default proxy pipeline, which stays symbol-conservative (consistent with D1 being off-by-default). A neural embedder (e.g. fastembed) is a drop-in behind the same trait; not added here to keep `cull-core` ML-dependency-free.

**Tech:** Rust, `xxhash_rust` (already a dep). Builds on `passes/relevance.rs` idioms, `task.rs`. Reference: spec §7 B3.

---

### Task 1: `Embedder` trait + `HashEmbedder` + `cosine`

**Files:** create `crates/cull-core/src/embed.rs`; modify `crates/cull-core/src/lib.rs` (`pub mod embed;`).

- [ ] **Step 1 — failing tests.** Create `crates/cull-core/src/embed.rs` with the test module:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn related_text_scores_higher_than_unrelated() {
        let e = HashEmbedder::default();
        let q = e.embed("jwt authentication token expiry");
        let related = e.embed("token authentication jwt expired");
        let unrelated = e.embed("kubernetes helm chart registry docker");
        assert!(cosine(&q, &related) > cosine(&q, &unrelated));
        assert!(cosine(&q, &related) > 0.5, "related sim {}", cosine(&q, &related));
        assert!(cosine(&q, &unrelated) < 0.1, "unrelated sim {}", cosine(&q, &unrelated));
    }

    #[test]
    fn embedding_is_deterministic_and_normalized() {
        let e = HashEmbedder::new(128);
        assert_eq!(e.embed("hello world"), e.embed("hello world"));
        let norm: f32 = e.embed("hello world").iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "L2 norm {norm}");
    }

    #[test]
    fn cosine_handles_mismatched_lengths_and_empty() {
        assert_eq!(cosine(&[1.0, 0.0], &[1.0]), 0.0);
        assert_eq!(cosine(&HashEmbedder::default().embed(""), &HashEmbedder::default().embed("x")), 0.0);
    }
}
```

- [ ] **Step 2 — confirm FAIL** (`cargo test -p cull-core embed`).

- [ ] **Step 3 — implement** at the top of `embed.rs`:
```rust
use xxhash_rust::xxh3::xxh3_64;

/// Maps text to a fixed-dimension vector for salience scoring (spec §7 B3).
pub trait Embedder {
    fn embed(&self, text: &str) -> Vec<f32>;
    fn dim(&self) -> usize;
}

/// Dependency-free hashing embedder (the hashing trick): lowercased alphanumeric tokens are hashed
/// into `dim` buckets as an L2-normalized term-frequency vector. Cosine similarity over these
/// vectors is a lexical/semantic salience signal complementary to symbol extraction (B1). A neural
/// embedder can replace it behind the `Embedder` trait without changing the pass.
pub struct HashEmbedder { dim: usize }

impl HashEmbedder {
    pub fn new(dim: usize) -> Self { Self { dim: dim.max(1) } }
}
impl Default for HashEmbedder { fn default() -> Self { Self::new(256) } }

impl Embedder for HashEmbedder {
    fn dim(&self) -> usize { self.dim }
    fn embed(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0f32; self.dim];
        for tok in text.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()) {
            let h = (xxh3_64(tok.to_ascii_lowercase().as_bytes()) as usize) % self.dim;
            v[h] += 1.0;
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 { for x in &mut v { *x /= norm; } }
        v
    }
}

/// Cosine similarity. For the L2-normalized vectors `HashEmbedder` produces this is the dot
/// product. Returns 0.0 on length mismatch or a zero vector.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() { return 0.0; }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
```

- [ ] **Step 4 — wire** `pub mod embed;` into `crates/cull-core/src/lib.rs` (near the other `pub mod`s).

- [ ] **Step 5 — confirm PASS** (`cargo test -p cull-core embed`), then `cargo test --workspace`.

- [ ] **Step 6 — commit.** `git add crates/cull-core && git commit -m "feat(core): Embedder trait + dependency-free HashEmbedder (B3 substrate)"`

---

### Task 2: `EmbeddingSaliencePass`

**Files:** create `crates/cull-core/src/passes/salience.rs`; modify `crates/cull-core/src/passes/mod.rs` (`pub mod salience;` + `pub use salience::EmbeddingSaliencePass;`).

- [ ] **Step 1 — failing tests.** Create `crates/cull-core/src/passes/salience.rs` with the test module:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::HashEmbedder;
    use crate::segment::*;
    use crate::plan::{SegmentAction, DropReason};
    use crate::planner::Planner;
    use crate::session::SessionState;
    use crate::task::TaskSignal;

    fn seg(id: u64, pos: usize, kind: SegmentKind, text: &str) -> Segment {
        Segment {
            id: SegmentId(id), kind, role: Role::Tool, bytes: text.as_bytes().to_vec(),
            token_count: 10, position: pos, mutation_class: MutationClass::Fast,
            origin: Origin::default(), protected_spans: vec![], refs: RefLedger::default(),
        }
    }

    #[test]
    fn drops_low_salience_old_keeps_salient_and_recent() {
        let task = TaskSignal::from_text("jwt authentication token");
        let segs = vec![
            seg(0, 0,  SegmentKind::FileRead, "jwt token verify authentication middleware"), // salient, old -> keep
            seg(1, 1,  SegmentKind::ToolOutput { class: "grep".into() }, "kafka broker partition offset consumer"), // low, old -> drop
            seg(2, 50, SegmentKind::ToolOutput { class: "grep".into() }, "redis sentinel cluster failover"), // low BUT recent -> keep
        ];
        let plan = Planner::new(vec![Box::new(EmbeddingSaliencePass::new(HashEmbedder::default(), 0.1, 6))])
            .plan_with_task(&segs, &SessionState::default(), &task);
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
        assert_eq!(plan.entries[1].action, SegmentAction::Drop(DropReason::IrrelevantBySlice));
        assert_eq!(plan.entries[2].action, SegmentAction::Keep);
    }

    #[test]
    fn no_task_signal_drops_nothing() {
        let segs = vec![seg(0, 0, SegmentKind::FileRead, "anything at all here")];
        let plan = Planner::new(vec![Box::new(EmbeddingSaliencePass::new(HashEmbedder::default(), 0.1, 0))])
            .plan(&segs, &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
    }

    #[test]
    fn non_droppable_kinds_never_dropped() {
        let task = TaskSignal::from_text("jwt authentication");
        let segs = vec![seg(0, 0, SegmentKind::ConversationTurn, "totally unrelated kafka chatter")];
        let plan = Planner::new(vec![Box::new(EmbeddingSaliencePass::new(HashEmbedder::default(), 0.5, 0))])
            .plan_with_task(&segs, &SessionState::default(), &task);
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
    }
}
```

- [ ] **Step 2 — confirm FAIL** (`cargo test -p cull-core salience`).

- [ ] **Step 3 — implement** at the top of `salience.rs`:
```rust
use crate::plan::{DropReason, PlanEntry, SegmentAction};
use crate::planner::{Pass, PlanCtx};
use crate::segment::SegmentKind;
use crate::embed::{cosine, Embedder};

fn is_droppable_kind(kind: &SegmentKind) -> bool {
    matches!(
        kind,
        SegmentKind::FileRead | SegmentKind::DirListing | SegmentKind::ToolOutput { .. }
            | SegmentKind::StackTrace | SegmentKind::TestOutput | SegmentKind::Diff
    )
}

/// Embedding-salience pruning (spec §7 B3). Complements symbol-based relevance (B1): scores each
/// droppable, non-recent segment by cosine similarity between its embedding and the task embedding
/// (joined task symbols), dropping those below `min_similarity`. The default embedder is the
/// dependency-free `HashEmbedder`; a neural embedder plugs in behind the `Embedder` trait. Opt-in:
/// not part of the default pipeline (the symbol-based relevance pass stays the conservative
/// default). No task signal ⇒ no drops.
pub struct EmbeddingSaliencePass<E: Embedder> {
    pub embedder: E,
    pub min_similarity: f32,
    pub recency_keep: usize,
}

impl<E: Embedder> EmbeddingSaliencePass<E> {
    pub fn new(embedder: E, min_similarity: f32, recency_keep: usize) -> Self {
        Self { embedder, min_similarity, recency_keep }
    }
}

impl<E: Embedder> Pass for EmbeddingSaliencePass<E> {
    fn name(&self) -> &'static str { "embedding-salience" }

    fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
        if ctx.task.is_empty() { return Vec::new(); }
        let mut query: Vec<&str> = ctx.task.symbols.iter().map(|s| s.as_str()).collect();
        query.sort_unstable(); // deterministic query string
        let qvec = self.embedder.embed(&query.join(" "));
        let max_pos = ctx.segments.iter().map(|s| s.position).max().unwrap_or(0);

        ctx.segments.iter().filter_map(|s| {
            if !is_droppable_kind(&s.kind) { return None; }
            if max_pos.saturating_sub(s.position) < self.recency_keep { return None; }
            let svec = self.embedder.embed(&String::from_utf8_lossy(&s.bytes));
            if cosine(&qvec, &svec) < self.min_similarity {
                Some(PlanEntry { id: s.id, action: SegmentAction::Drop(DropReason::IrrelevantBySlice) })
            } else {
                None
            }
        }).collect()
    }
}
```

- [ ] **Step 4 — wire** in `crates/cull-core/src/passes/mod.rs`: add `pub mod salience;` next to the other `pub mod`s and `pub use salience::EmbeddingSaliencePass;` next to the other re-exports. Do NOT add it to `structural_passes()` or `query_passes()` (it is opt-in — the existing pass-count tests must stay green).

- [ ] **Step 5 — confirm PASS** (`cargo test -p cull-core salience`), then `cargo test --workspace` (the `structural_passes`/`query_passes` count tests must still pass).

- [ ] **Step 6 — commit.** `git add crates/cull-core && git commit -m "feat(core): EmbeddingSaliencePass (B3) — opt-in semantic pruning behind Embedder"`

---

## After this plan — ledger
- ✅ B3 embedding salience — `Embedder` trait + dependency-free `HashEmbedder` + opt-in `EmbeddingSaliencePass` (cosine salience, complementary to B1). Neural backend is a drop-in behind the trait (not added to keep `cull-core` ML-dependency-free); fastembed feasibility probed + reported separately.

## Self-Review
- Reuses `xxhash_rust` (already a dep) → no new dependency, no model download → no env-gating. ✓
- Mirrors `RelevancePass` gating (droppable kinds, recency guard, no-task ⇒ no-op) → safe, consistent. ✓
- Opt-in (not in default pipeline) → proxy's verified lossless behavior unchanged; pass-count tests stay green. ✓
- Deterministic (sorted query, fixed hashing) → tests are stable. ✓
