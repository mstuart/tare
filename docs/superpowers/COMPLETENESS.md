# Cull — Spec Completeness Ledger

**This is the single definition of "done."** Cull is DONE only when every item below is ✅ and
verified against the actual code — never against "a plan ran." Maintained continuously; the word
"complete" is not used until every line is ✅.

Legend: ✅ implemented + verified · ⚠️ partial · ❌ not built
Source of truth: `docs/superpowers/specs/2026-06-18-cull-design.md`
Last audited: 2026-06-20 (verified against code with grep, not memory)

## §7 Passes
- [x] ✅ A1 Supersession decay — `passes/supersession.rs`
- [x] ✅ A2 File-read IVM/delta — wired into the proxy over full-history requests (cross-turn) — `passes/ivm.rs` + `cull-proxy`
- [ ] ❌ A3 RePair / n-gram envelope dedup — only exact-content dedup exists
- [ ] ❌ A4 Content-defined chunking (CDC/Merkle) + cross-session dedup
- [ ] ⚠️ B1 Query taint/program-slice — symbol-overlap only, NOT the tree-sitter dependency DAG — `passes/relevance.rs`
- [ ] ❌ B2 PRF query expansion
- [ ] ❌ B3 Embedding / logprob salience
- [ ] ❌ B4 Reasoning-trace pruning
- [ ] ⚠️ C1 Belady-oracle eviction — recency+relevance priority, NO plan lookahead
- [ ] ❌ C2 ARC freq×recency + task-phase decay
- [x] ✅ C3 Tail-only eviction (frozen never evicted)
- [ ] ❌ D1 Predicate pushdown (built OFF-by-default is acceptable; not-built is not)

## §8 Cache-aware planner
- [x] ✅ Economic model formulas — `cull-cache`
- [ ] ❌ Model WIRED into live compress/skip decisions (verified: used NOWHERE outside cull-cache)
- [ ] ⚠️ Rule 1 immutable prefix — I3 enforces frozen=Keep, but no `CachePrefixCommitment` tracking in live flow
- [ ] ❌ Rule 2 write-amortization gate (formula exists, never applied)
- [x] ✅ Rule 3 stability-ordered segmentation
- [x] ✅ Rule 4 tail-only eviction
- [ ] ❌ Rule 5 hit-rate-floor monitor
- [ ] ⚠️ Rule 6 provider-aware costs (model only, not applied)
- [x] ✅ Rule 7 no reformatting in frozen zone (lossless, structural)
- [ ] ❌ Rule 8 tool-definition freeze
- [ ] ⚠️ Rule 9 delta-before-full-resend (IVM deltas, but not gated by this rule)
- [ ] ❌ Rule 10 compress-once-not-incrementally (no boundary logic)
- [ ] ❌ Boundary detection (when to compress)

## §9 Invariants
- [x] ✅ I1 net-non-negative
- [x] ✅ I2 kept-token byte-exact
- [x] ✅ I3 cached-prefix immutable (frozen)
- [x] ✅ I4 exact-token-class preservation
- [x] ✅ I5 lossless reconstruction (verifiable delta)
- [ ] ❌ I6 quality floor (compress-to-a-quality-floor loop) — not implemented

## §6 Session state & tokenization
- [ ] ⚠️ Session-state TYPES exist but are NEVER threaded (verified: only `::default()` placeholders outside `session.rs`)
- [x] ✅ ApproxCounter token counting
- [ ] ❌ Anthropic `count_tokens` API (exact counts) — approximation only

## §10 Proxy
- [ ] ⚠️ Anthropic request compression — `tool_result` STRING content only
- [ ] ❌ `tool_result` array content
- [ ] ❌ `system` / `tools` compression
- [ ] ❌ OpenAI support
- [x] ✅ Streaming response passthrough
- [x] ✅ Transparency mode (`CULL_ENABLED=0`)
- [x] ✅ Cross-turn compression via full-history requests (Anthropic resends history each call; an explicit cross-request store is unnecessary for stateless Anthropic)
- [x] ✅ Supersession + IVM wired into the proxy (via `tool_use` metadata: name→class, input.path→path)
- [x] ✅ FidelityReport surfaced from the proxy (`x-cull-*` response headers)

## §11 Emitter / fidelity report
- [x] ✅ FidelityReport — `emit.rs`
- [x] ✅ Surfaced in CLI
- [x] ✅ Surfaced from proxy (`x-cull-*` headers)

## §12 Benchmark
- [ ] ⚠️ Corpus — 3 small in-repo items (spec wants real agent traces / SWE-bench-style)
- [ ] ❌ Real-incumbent baselines (LLMLingua-2, Headroom, Tamp, native /compact)
- [x] ✅ Metric: compression ratio
- [x] ✅ Metric: net tokens
- [ ] ❌ Metric: downstream-task fidelity
- [ ] ❌ Metric: tool-call fidelity
- [ ] ❌ Metric: false-negative / divergence rate
- [ ] ❌ Metric: cache-hit-rate impact
- [x] ✅ Leaderboard (basic)

---

## Genuine environment blockers (surface explicitly, never silently skip)
- **Real-incumbent benchmark** needs external pip/npm installs (LLMLingua-2 = Python, Headroom = Python, Tamp = Node). Plan: attempt the installs; if the sandbox blocks network/install, the shell-out ADAPTERS are still built and the specific blocker is reported here — the adapters are "done," the live run is gated on the tool being present.
- **Predicate-pushdown (D1)** rewrites the agent's real tool calls. Plan: BUILD it, ship OFF by default (opt-in flag). "Off by default" counts as done; "not built" does not.

## Tally (update every change)
Updated after Plan 13: roughly 21 ✅ / 7 ⚠️ / 19 ❌. **NOT DONE.** (Plan 13 wired the full pass set + report into the proxy.)
