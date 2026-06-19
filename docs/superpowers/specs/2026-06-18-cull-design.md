# Cull — Query-Aware, Cache-Correct, Lossless Context Compression for Coding Agents

- **Status:** Design — approved to spec 2026-06-18
- **Name:** `cull`
- **Language:** Rust
- **Scope:** Complete system, end to end. Not a v1/MVP/POC. Every technique surfaced in research is in scope; the "Build Order" section sequences the work but nothing is deferred out of the product.

---

## 1. Summary

Cull is a context-compression **engine + proxy** for LLM coding agents (Claude Code, Cursor, Cline, Codex, and any Anthropic-/OpenAI-compatible client). It reduces the token cost of long agent sessions by compressing the accumulating conversation/tool-result **tail** — at the compaction boundary, conditioned on the agent's **current task**, while **preserving the prompt cache** and **never mutating a kept token**.

The thesis, which is also the entire difference from every incumbent:

> **Whole-unit, lossless, query-aware, cache-correct.**
> Cull *drops* whole irrelevant / superseded / duplicate units and *losslessly delta-encodes* re-reads. Anything it keeps is byte-exact. It scores what to keep against the current task (incumbents are query-blind). It only ever touches the uncached tail and accounts for prompt-cache economics (incumbents silently bust the cache and can go net-negative). It guarantees it never increases net token cost.

This is a deliberate **anti-LLMLingua** position. Token-level lossy pruning is precisely what breaks coding agents — it destroys file paths, line numbers, identifiers, exact literals, and null-vs-empty distinctions. Cull performs **no token-level lossy compression**. It operates on whole semantic units and lossless deltas only.

---

## 2. Motivation & Evidence

The token/context-compression space for coding agents is **saturated** (60+ shipping tools: RTK, Headroom, Tamp, Edgee, Snip, Entroly, tokdiet, Caveman, LiteLLM, Cloudflare Code Mode, ~15 MCP servers, plus the research crowd). Yet a coherent set of blind spots is shared across **all** of them, confirmed by five independent research sweeps (technique frontier, competitor census, cross-domain novelty, coding-agent structure, cache economics):

1. **Query-blindness.** No shipping proxy conditions what it keeps on the current task. This is the root cause of headline collapse: Headroom's "60–95%" measured **0.34% net** on 3,000 real agent tool-outputs in an independent NousResearch eval (PR #47866), and the vendor's own telemetry median is 4.8%.
2. **Cache-interaction blindness.** No tool measures or optimizes compression against prompt-cache invalidation. Reformatting the cached prefix silently drops a 96% cache-hit rate; on Anthropic's write-premium model this can cost **more** than the tokens saved.
3. **Wrong-boundary firing.** Cost comes from cumulative tool-output growth across turns; tools fire reactively at a fixed % threshold or per-request, never at the inflection point.
4. **Lossy on exact values.** LLMLingua-style perplexity pruning destroys code identifiers and JSON keys; SmartCrusher collapses null and empty to the same cell. Almost nothing guarantees byte-exact code.
5. **Dishonest measurement.** Every 60–95% claim is self-reported on a workload chosen to favor the technique. The only independent measurement in the field found 0.34%.

Corroborating facts (each to be re-confirmed against primary sources before any number is published — see §15):

- Tool **output** dominates agent context (~84% of SWE-agent turn tokens). Simple observation masking halves cost and matches LLM summarization on solve rate. *(arXiv 2508.21433 "The Complexity Trap".)*
- On commercial APIs, heavy compression is often **net-slower** (<0.5× speedup); the latency "win" is an unoptimized-serving artifact. *(arXiv 2604.02985 "Prompt Compression in the Wild".)* → Cull claims **token/context** savings, never latency.
- "Can Compressed LLMs Truly Act?" (arXiv 2505.19433, ACBench) concerns **weight** compression (quantization/pruning), not prompt compression — it shows 1–3% tool-use degradation but 10–15% on end-to-end tasks. Prompt-level compression avoids weight-level degradation entirely; this paper is **not** evidence that prompt compression breaks tool-calling.
- Compression can **increase** tokens: a documented Hermes case went 64,186 → 71,173 (+7k). No incumbent guards against it.

**The opening:** a single tool that is query-aware **and** cache-correct **and** lossless-on-exact-values **and** fires at the compaction boundary **and** measures itself honestly does not exist. Cull is the coherent union of those blind spots plus the lossless/cache discipline. That union — not any single pass — is the novelty.

---

## 3. Goals / Non-Goals

### Goals
- Reduce **net** token cost (cache-adjusted) of long coding-agent sessions, conditioned on the current task.
- **Never** increase net cost; **never** mutate a kept token; **never** invalidate the cached prefix.
- Work against **black-box hosted models** (Anthropic, OpenAI) via a proxy — the only mechanism that can actually compress a prompt (Claude Code hooks *prepend*, they cannot replace).
- Self-report fidelity on the user's **real** workload (sidesteps benchmark-workload-mismatch).
- Prove "better" with an **honest, reproducible benchmark** against incumbents.

### Non-Goals
- No token-level lossy pruning of kept content (the anti-LLMLingua stance).
- No latency claims.
- No model fine-tuning or weight access; no KV-cache/serving internals; no soft-prompt/gist methods (all API-incompatible).
- Not a general LLM gateway, router, or observability product. Compression only.
- Not (in this product) a semantic cache or RAG store.

---

## 4. Success Criteria

1. **Correctness invariants hold, provably** (property-tested): net-non-negative, kept-token byte-exactness, cached-prefix immutability, exact-token-class preservation.
2. **Honest benchmark** shows Cull beats LLMLingua-2, Headroom, Tamp, naive truncation, and native `/compact` on **net cache-adjusted tokens at equal-or-better downstream task fidelity**, broken out by task type, on real agent traces.
3. **Tool-call fidelity** (parameter extraction, schema adherence) is preserved within measurement noise — measured explicitly, because nobody else measures it.
4. The **self-report** emitted on real workloads is accurate (ratio, net tokens, cache impact, what was dropped) and falsifiable.
5. The proxy is **transparent**: identical agent behavior with compression off; streaming and tool-calls preserved bit-for-bit.

---

## 5. System Architecture

```
                          ┌──────────────────────── cull ────────────────────────┐
client (Claude Code,      │                                                          │
Cursor, Cline, Codex) ───▶│  PROXY  (cull-proxy)                                   │
   ANTHROPIC_BASE_URL     │   • intercept request, detect provider + cache state     │
   / OPENAI_BASE_URL      │   • boundary detection (when to compress)                │
                          │   • streaming + tool-call + auth passthrough             │
                          │                    │                                     │
                          │                    ▼                                     │
                          │  ENGINE  (cull-core)         task signal ──┐           │
                          │   ┌──────────┐ ┌────────┐ ┌─────────┐ ┌──────────┐       │
                          │   │ segmenter │▶│ scorer │▶│ planner │▶│ emitter   │      │
                          │   └──────────┘ └────────┘ └─────────┘ └──────────┘       │
                          │    typed,       query-      cache-aware  compressed       │
                          │    token-       conditioned budget +     request +        │
                          │    counted      relevance   pass orch.   fidelity report  │
                          │    segments     + structural + invariants                 │
                          │                                                          │
                          │  SESSION STATE (cull-core::session)                    │
                          │   • canonical file store (IVM)  • tool-class registry    │
                          │   • span reference/recency/phase ledger                  │
                          │   • cache-prefix commitment (frozen-zone hash)           │
                          │   • cross-session chunk store (CDC/Merkle, on disk)      │
                          └──────────────────────────────────────────────────────────┘
                                       │                         ▲
                                       ▼                         │
                              upstream provider           BENCH (cull-bench)
                              (Anthropic / OpenAI)         engine + baselines over
                                                           real traces → leaderboard
```

The engine is **pure** (no I/O) and reusable; the proxy and bench wrap it. The CLI exposes the engine offline.

### Crate layout (Rust workspace)

| Crate | Responsibility |
|---|---|
| `cull-core` | Engine: segmenter, scorer, planner, emitter, all passes, session state. No I/O. |
| `cull-tokenize` | Token counting: `tiktoken-rs` approximation + Anthropic `count_tokens` API client; provider-aware. |
| `cull-cache` | Provider cache models + economic model (Anthropic 5m/1h, OpenAI), parameterized by `W` (write mult), `R` (read mult), min-prefix, TTL. |
| `cull-proxy` | Streaming reverse proxy (Anthropic + OpenAI compatible); boundary detection; passthrough fidelity. |
| `cull-bench` | Benchmark harness; runs engine + baselines (shells out to Python/Node tools) over corpora; emits leaderboard. |
| `cull-cli` | Offline compression / inspection / single-trace runs / fidelity report rendering. |

Key external crates: `tree-sitter` + language grammars (segmenter, slicer), `fastembed`/`ort` (embedding scorer, in-process ONNX), a Myers-diff crate (IVM/delta), `fastcdc` (content-defined chunking), `xxhash-rust` (content addressing), a BM25 implementation (PRF). No Python in the engine; Python/Node appear only as benchmark **baselines** the harness shells out to.

---

## 6. Core Data Model

### Segment
The unit of all operations. The engine never operates below the segment except for lossless dedup/delta inside a kept segment.

```
Segment {
  id: SegmentId,
  kind: SegmentKind,          // see below
  role: Role,                 // system | user | assistant | tool
  bytes: Bytes,               // exact original content (never mutated if kept)
  token_count: u32,           // provider-aware
  position: usize,            // order in the assembled request
  mutation_class: MutationClass,   // FROZEN | SLOW | FAST  (cache ordering)
  origin: Origin,             // turn index, tool name, file path+range, mtime, exit code
  protected_spans: Vec<Span>, // exact-token classes inside this segment (never dropped)
  refs: RefLedger,            // recency, frequency, task-phase (for eviction policies)
}
```

`SegmentKind ∈ { SystemPrompt, ToolSchema, FileRead, DirListing, Diff, ToolOutput(class), StackTrace, TestOutput, ReasoningTrace, ConversationTurn, CompactSummary }`.

`MutationClass` drives cache-stable ordering (§8 Rule 3): `FROZEN` (system, never changes), `SLOW` (tool schemas, project rules), `FAST` (tool results, current turn).

### Protected exact-token classes (never dropped or mutated)
File paths, line numbers, error codes/messages, numeric literals (ports, versions, retry counts, hashes), and the null-vs-empty distinction. Detected by the segmenter (regex + tree-sitter), recorded as `protected_spans`, and treated as a hard floor by every pass.

### Session state
- **Canonical file store** (IVM): `path → (canonical_bytes, snapshot_token_count, version)`; lives in the frozen prefix.
- **Tool-class registry** (supersession): `class → latest_turn`, with exit-code/resolution tracking.
- **Span ledger**: per-segment recency, frequency, task-phase — for eviction policies.
- **Cache-prefix commitment**: hash of the frozen zone up to the last cache breakpoint; the engine treats this as a cryptographic commitment and transmits it byte-identical.
- **Cross-session chunk store** (CDC/Merkle): on-disk content-addressed chunks keyed by repo, enabling re-read dedup across sessions.

### Task signal
Derived from: the latest user message, the active TODO/plan (TodoWrite list, `PLAN.md`, GSD phase file if present), and the files under active edit (most recent Edit tool calls). Parsed to a set of **query symbols** (function/type/file names, error codes) used by the slicer and scorers. No model call required to extract it.

---

## 7. The Engine: Passes

Passes are grouped by character. The **planner** (§8) decides which fire, in what order, gated by the economic model and invariants. All passes operate on the **tail** (post-cache-boundary) and are either whole-unit drops or lossless transforms.

### A. Structural lossless passes (no relevance judgment; always safe)

**A1. Supersession decay.** When a new tool result of a known class arrives (build/test/lint/`ls`/`grep`/file-read), prior results of the same class+target are replaced by a stub preserving turn index and exit code; fully resolved errors (non-zero → zero transition) are dropped entirely. *Highest leverage* (tool output ≈ 84% of tokens). Whole-unit; exact-value-safe. Requires the tool-class registry + a resolution signal. Guard: only supersede same target; preserve unresolved errors verbatim.

**A2. File-read IVM / delta-against-canonical.** First read of a path snapshots a canonical copy into the frozen prefix. Subsequent reads are emitted as **lossless unified diffs** against the canonical. At the boundary, accumulated deltas fold into a new canonical and the cycle repeats. Lossless (exact reconstruction). Includes the simpler **delta-against-immutable-baseline** variant for files with few edits.

**A3. Lossless repetitive-envelope dedup (RePair/n-gram).** Grammar-based compression over the token stream of repetitive tool output (repeated JSON envelopes, stack-frame prefixes, path prefixes). The dictionary/canonical forms live in the frozen prefix; duplicate occurrences in the tail are suppressed. **Internal-dedup mode**: the engine pre-expands before the request hits the wire, so the model never sees grammar notation — fully transparent and cache-safe. Lossless.

**A4. Content-defined chunking + Merkle dedup (CDC).** `fastcdc` rolling-hash chunking over tool results detects **near**-duplicates (file re-read after a small edit; directory listing with one new entry) that exact-match dedup misses. Within-session: canonical chunks in the prefix, back-references in the tail. **Cross-session**: a persistent repo-scoped chunk store emits common scaffolding (package.json, tsconfig, Cargo.toml) once ever per repo. Lossless.

### B. Query-conditioned relevance passes (the wedge)

**B1. Query-conditioned program-slice / taint pruning (primary mechanism).** Treat the tool-call history as a program and the task query symbols as the slice criterion. Build a dependency DAG over tool-result spans via tree-sitter symbol resolution; mark each span tainted-relevant or tainted-irrelevant; **drop whole irrelevant spans**. Deterministic, zero model calls, exact-value-safe (drops whole spans, never mutates kept ones), cache-safe. This is the highest-precision relevance signal because it uses actual symbol dependency, not heuristic proximity.

**B2. PRF query expansion → two-stage re-scoring (recall booster).** Stage 1: BM25 over the tail retrieves the top-k spans most similar to the query. Stage 2: extract their salient technical terms (symbols, paths, error codes) as an expanded query; re-score all spans; apply budget-aware MMR (relevance − redundancy). Recovers spans that are relevant but use vocabulary the short query omitted ("fix the auth bug" → jwt, TokenExpiredError, middleware.ts). No model call.

**B3. Embedding/logprob salience (fallback for ambiguous spans).** For spans where structure (B1) is insufficient, score by cosine similarity between span and task embeddings via in-process `fastembed` (ONNX, ~50ms/batch). Logprob-probe variant is gated to providers that expose logprobs (OpenAI yes; Anthropic limited) — so embedding similarity is the primary fallback.

**B4. Reasoning-trace pruning (conservative).** Prune assistant reasoning blocks for hypotheses explicitly negated by a later conclusion, and only when older than N turns. Keeps decision anchors, drops abandoned scratch-work. *Lower leverage than tool output* and higher risk (backtracking) — therefore conservative and last in the relevance group.

### C. Eviction / budget policies (only when the budget forces choices)

**C1. Belady-oracle eviction via plan lookahead.** Use the agent's structured future (plan steps, queued tool calls, GSD phase) as an offline oracle: spans whose symbols appear in upcoming steps score "near-future use" (keep); completed-subtask spans with no forward references score "far-future" (evict). Forward-looking, unlike all KV-cache eviction heuristics.

**C2. ARC-style frequency × recency with task-phase decay.** Self-tuning recency/frequency retention (ghost lists) over spans, with a phase-decay factor: spans created in earlier phases (discovery/planning) decay as the agent advances (execution/verification). Pure bookkeeping, zero model calls.

**C3. Tail-only eviction.** When the window must shrink, evict from the **tail**, never the head. The cached prefix is sacred (sliding-window-from-the-head resets cache-hit to 0 — the catastrophic incumbent pattern).

### D. Upstream prevention (before tool execution)

**D1. Predicate pushdown.** Rewrite over-broad tool-call arguments *before* execution using task context: `grep -r 'TODO' .` → scope to `src/auth/ --include='*.ts' -l`; `read_file('server.ts')` → predicted line range from the symbol index. Rule-based (no model call). Prevents bloat at the source instead of curing it. *Note:* this modifies tool **calls**, not just context, so it is opt-in and clearly isolated; it requires the proxy/hook to sit at the tool-call boundary.

---

## 8. Cache-Aware Planner & Economic Model

The planner is what makes Cull *correct* where the field is silently broken. It is parameterized by the provider cache model and enforces the invariants.

### Economic model (provider-parameterized)
Variables: base price `B`; write multiplier `W`; read multiplier `R`; prefix tokens `T`; hit rate `h`; turns of reuse `N`; compression ratio `c` (compressed/original).

- **Caching is net-positive** iff `h > (W − 1) / (W − R)`.
  - Anthropic 5-min (`W=1.25, R=0.1`): `h > 0.217`.
  - Anthropic 1-hour (`W=2.0, R=0.1`): `h > 0.526`.
  - OpenAI (`W=1.0, R=0.1`): always (no write premium).
- **Compress-once-at-the-boundary is worth it** iff remaining turns `N_future > W / ((1 − c) · R)`.
  - Anthropic 5-min at `c=0.6`: `N_future > ~31` turns.
  - Anthropic 1-hour at `c=0.6`: `N_future > ~50`.
  - OpenAI at `c=0.6`: `N_future > ~25`.
- **Deliberate cache bust** (e.g., `/compact`) is worth it iff `(T_old − T_new) · R · N_future > T_new · W`.

These thresholds are recomputed at session init from the detected provider + TTL.

### Cache-aware design rules (engine invariants)
1. **Immutable prefix.** Compression may only touch tokens after the last cache breakpoint. Any op that would mutate a frozen-zone byte is rejected.
2. **Write-amortization gate.** No compress-and-recache event unless `N_future > W/((1−c)·R)`. Short sessions compress **nothing**.
3. **Stability-ordered segmentation.** Segments ordered by ascending mutation frequency (FROZEN → SLOW → FAST). High-mutation content is never placed before low-mutation content.
4. **Tail-only eviction.** Eviction starts at the tail; evicting a frozen-zone segment is rejected (compress the tail or trigger the boundary gate instead).
5. **Hit-rate floor.** Monitor `cache_read_input_tokens` / `cache_creation_input_tokens`. If `h` falls below the provider floor for 3+ consecutive turns, halt compression and diagnose the invalidation source.
6. **Provider-aware costs.** All thresholds parameterized by detected `W`, `R`, TTL.
7. **No reformatting in the frozen zone.** Even lossless reformatting (whitespace, JSON key reorder) breaks the exact-prefix hash. Frozen zone is transmitted byte-identical.
8. **Tool-definition freeze.** Tool/MCP schema changes are batched to session boundaries; mid-session tool addition is flagged with its re-cache cost.
9. **Delta before full re-send.** New content similar to cached content (re-reads, revised docs) is delta-encoded first; full re-send only if delta > 70% of original.
10. **Compress-once, not incrementally.** One boundary compression replaces N incremental ones (each incremental pass costs a `W·B` write).

### Boundary detection (when to compress)
Compression fires at the **compaction boundary**, not per-request and not at a fixed % threshold. The boundary is detected when the ratio of *new useful content* to *already-seen content* in the tail drops below a threshold **and** the write-amortization gate (Rule 2) passes. This is the inflection point incumbents miss in both directions (reactive tools fire too late; proactive ones too early).

---

## 9. Correctness Invariants

Enforced in code and verified by property tests (§13):

- **I1 — Net-non-negative.** `net_tokens(out) ≤ net_tokens(in)`, cache-adjusted. If a compression plan does not strictly reduce net tokens, the engine emits the original unchanged. (Kills the Hermes +7k regression by construction.)
- **I2 — Kept-token byte-exactness.** Any segment retained is transmitted byte-identical; no token-level mutation of kept content, ever.
- **I3 — Cached-prefix immutability.** The frozen zone is byte-identical every turn.
- **I4 — Exact-token-class preservation.** Protected spans (paths, line numbers, error codes, numeric literals, null-vs-empty) are never dropped or altered.
- **I5 — Lossless reconstruction.** Every delta/dedup transform is exactly reversible by the engine before the request is sent (internal mode) or by documented expansion tooling.
- **I6 — Quality floor (PoC framing).** The engine targets a configurable downstream-quality floor, not a fixed ratio; it compresses as aggressively as the floor allows and no further.

---

## 10. The Proxy

The only mechanism that can actually compress a prompt against a hosted model (hooks prepend; they cannot replace). Delivered as a streaming reverse proxy.

- **Interception:** clients point `ANTHROPIC_BASE_URL` / `OPENAI_BASE_URL` at Cull. Requests are parsed, compressed at the boundary, forwarded; responses streamed back untouched.
- **Providers:** Anthropic (Messages API, `cache_control` breakpoints) and OpenAI-compatible (automatic prefix caching). Provider detection sets the economic parameters.
- **Fidelity (non-negotiable):** streaming SSE, tool-call blocks, `cache_control` markers, and auth headers pass through **bit-exact**. With compression disabled the proxy is a transparent passthrough (a measured invariant, not an aspiration).
- **State:** per-session (keyed by a stable session id derived from the stable prefix) — canonical file store, registries, ledgers, cache-prefix commitment.
- **Config:** provider, TTL mode, quality floor, which passes are enabled (D1 predicate pushdown off by default), hit-rate-floor thresholds.

---

## 11. Self-Reporting Emitter & Fidelity Report

The emitter assembles the compressed request and emits a fidelity report **on the user's real workload** — which structurally beats the field's cherry-picked benchmarks ("here's what it did on *your* traffic" can't be cherry-picked):

- Exact compression ratio and **net** tokens (cache-adjusted, incl. any per-request overhead).
- Per-segment keep/drop/delta decisions with the reason (superseded / irrelevant-by-slice / duplicate / evicted).
- Cache impact: predicted vs. observed `cache_read` / `cache_creation` tokens; hit-rate trend.
- A fidelity estimate of what task-relevant content was dropped (and an explicit "nothing in a protected class was touched" assertion).

---

## 12. The Benchmark (Honest Measurement)

"Better" is proven, not claimed. `cull-bench` is a first-class deliverable.

- **Corpus:** real multi-turn coding-agent traces (recorded Claude Code / agent sessions), SWE-bench-style tasks, and the public datasets used by the cited papers. Each item: full context + task + ground-truth correct continuation/tool-call.
- **Contestants:** Cull, LLMLingua-2, Headroom, Tamp, naive truncation, native `/compact`. Each invoked uniformly via a CLI seam (the harness shells out to the Python/Node baselines — we do **not** reimplement them).
- **Metrics:** compression ratio; **net cache-adjusted tokens**; **downstream task fidelity** (does the agent produce the correct next action with the compressed context?); **tool-call fidelity** (parameter extraction, schema adherence — measured explicitly, since nobody else does); **false-negative / divergence rate** (cases compression makes *wrong*, not just preserved cases); **cache-hit-rate impact**; broken out by **task type** with code-gen and exact-value lookup called out (the fragile tasks).
- **Output:** a reproducible leaderboard — the citable recognition artifact.

---

## 13. Testing Strategy

- **Property tests** (the invariants): I1 net-non-negative, I2/I4 byte-exactness of kept + protected content, I3 frozen-zone immutability, I5 lossless round-trip (`expand(compress(x)) == x` for all dedup/delta passes).
- **Golden tests:** recorded segments → expected keep/drop/delta plans.
- **Unit tests:** per pass (supersession resolution logic, Myers diff correctness, RePair grammar round-trip, slicer symbol resolution, BM25/MMR selection, economic-gate arithmetic).
- **Integration:** proxy round-trip preserves streaming + tool-calls bit-exact; compression-off transparency.
- **Benchmark as regression gate:** net-token and fidelity deltas vs. baselines tracked across commits; a regression fails CI.

---

## 14. Build Order (complete system — sequencing only, nothing descoped)

1. `cull-core` types, segmenter, `cull-tokenize`, session-state skeleton.
2. `cull-cache` economic model + planner skeleton + invariants I1–I4.
3. Structural lossless passes A1 (supersession), A2 (IVM/delta), A3 (RePair) — highest leverage, deterministic.
4. Query-conditioned passes B1 (slice) → B2 (PRF) → B3 (embedding fallback).
5. Eviction policies C1 (Belady-oracle), C2 (ARC phase-decay), C3 (tail-only).
6. A4 (CDC within-session, then cross-session store); B4 (reasoning-trace pruning).
7. Emitter + fidelity report (§11).
8. `cull-proxy` — Anthropic first, then OpenAI; transparency + streaming + tool-call fidelity.
9. D1 predicate pushdown (opt-in, at the tool-call boundary).
10. `cull-bench` — corpus, baselines, metrics, leaderboard.
11. `cull-cli`.

---

## 15. Risks & Open Questions

### Risks
- **Crowded field.** 60+ tools; even a strictly better tool faces an adoption/recognition fight. Differentiation rests on the lossless + cache-correct + query-aware + honest-measurement *combination*, not any single pass.
- **Not first to each idea.** Pieces exist scattered (tokdiet shadow-eval + cache preservation; Squeez/SWE-Pruner query-awareness via fine-tuned models; mcp-compressor schema compression). Cull's claim is the coherent union + correctness discipline, which must be stated honestly.
- **Slicer precision** (B1) on dynamically-typed / cross-file symbol resolution is the hardest engineering risk; B3 embedding fallback mitigates false drops.
- **Proxy fidelity is existential.** Any corruption of streaming or tool-call semantics is fatal to adoption; hence the transparency invariant + integration tests.

### Load-bearing claims to independently re-confirm before any number is published
1. Anthropic + OpenAI cache pricing/mechanics (`W`, `R`, min-prefix, TTL) — from official docs.
2. The ~84% tool-output / observation-masking leverage (arXiv 2508.21433).
3. The Headroom 0.34%-on-real-traces independent result (NousResearch PR #47866).

### Open questions (resolve during planning; non-blocking)
- Exact segment-granularity boundaries (per-tool-result vs. sub-result).
- Cross-session chunk-store on-disk format and cache-position accounting.
- Default embedding model (candidate: `bge-small` via `fastembed`).
- Session-id derivation that is stable across turns yet distinct across worktrees (Claude Code embeds cwd/branch/commits in the prefix).

---

## 16. References (primary sources; tags from research sweeps)

- Prompt Compression in the Wild — arXiv 2604.02985 (latency/quality reality) — VERIFIED
- The Complexity Trap (observation masking, ~84%) — arXiv 2508.21433 — VERIFIED
- Can Compressed LLMs Truly Act? / ACBench (weight, not prompt) — arXiv 2505.19433 — VERIFIED
- LLMLingua / LongLLMLingua / LLMLingua-2 — arXiv 2310.05736 / 2310.06839 / 2403.12968 — VERIFIED
- PoC performance-floor compression — arXiv 2603.19733 — VERIFIED
- TokenPilot (cache-aware dual-granularity) — arXiv 2606.17016 — VERIFIED
- CompactPrompt (lossless n-gram + reversal) — arXiv 2510.18043 — VERIFIED
- Dictionary-encoding lossless compression — arXiv 2604.13066 — VERIFIED
- SWE-Pruner — arXiv 2601.16746; Squeez — arXiv 2604.04979 — VERIFIED
- DCE-LLM (program slicing) — NAACL 2025; NeuroTaint — arXiv 2604.23374 — VERIFIED
- Incremental View Maintenance (Enzyme) — arXiv 2603.27775 — VERIFIED
- RePair / MR-RePair — arXiv 1811.04596 — VERIFIED
- FastCDC — USENIX ATC 2016 — VERIFIED
- ARC — Megiddo & Modha 2003; Belady 1966 — VERIFIED
- Don't Break the Cache — arXiv 2601.06007 — VERIFIED
- Anthropic prompt caching + pricing docs — platform.claude.com — VERIFIED
- OpenAI prompt caching + pricing docs — developers.openai.com — VERIFIED
- Headroom independent eval — NousResearch PR #47866 — VERIFIED
- "The Token-Saving Cake is a Lie" (hooks can't compress) — wynandpieters.dev — VERIFIED

*(Full 60-tool competitor census and 30+-paper technique frontier retained in research notes; condensed here to load-bearing items.)*
