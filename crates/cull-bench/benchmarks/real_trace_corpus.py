#!/usr/bin/env python3
"""Real Claude Code agent-trace compression corpus.

Scans actual Claude Code session transcripts (~/.claude/projects/**/*.jsonl),
extracts tool outputs, and measures how Cull performs on real traffic vs the
synthetic benchmark numbers (68-88% compression).

Three measurement passes:
  1. Corpus overview — transcript count, tool-output count, token-size distribution.
  2. Per-output compression — sample up to 200 outputs; ratio distribution.
  3. Cross-turn compression — for the 10 largest transcripts, feed ALL tool
     outputs as one multi-block cull call to exercise dedup/supersession.

Usage:
  /tmp/cull-headroom-venv313/bin/python real_trace_corpus.py
  CULL_BIN=/path/to/cull python real_trace_corpus.py
"""

import json
import os
import random
import subprocess
import sys
from pathlib import Path

CULL = os.environ.get(
    "CULL_BIN",
    os.path.join(os.path.dirname(__file__), "..", "..", "..", "target", "release", "cull"),
)
PROJECTS_DIR = Path.home() / ".claude" / "projects"
MAX_PER_OUTPUT_SAMPLE = 200
CROSS_TURN_TOP_N = 10

try:
    import tiktoken
    _enc = tiktoken.get_encoding("o200k_base")
    def tok(s: str) -> int:
        return len(_enc.encode(s, disallowed_special=()))
except Exception:
    def tok(s: str) -> int:  # fallback
        return len(s) // 4


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def redact(s: str, max_chars: int = 40) -> str:
    """Return a short, redacted prefix: mask emails, paths, tokens."""
    import re
    snippet = s[:max_chars].replace("\n", " ")
    snippet = re.sub(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Z|a-z]{2,}", "<email>", snippet)
    snippet = re.sub(r"/Users/[^/\s]+", "/Users/<user>", snippet)
    snippet = re.sub(r"Bearer [A-Za-z0-9\-._~+/]+=*", "Bearer <tok>", snippet)
    return snippet


def extract_tool_results(path: Path) -> tuple[list[str], str | None]:
    """Parse a JSONL transcript; return (tool_output_texts, last_user_prompt)."""
    outputs: list[str] = []
    last_prompt: str | None = None
    try:
        with open(path) as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                try:
                    obj = json.loads(line)
                except json.JSONDecodeError:
                    continue
                etype = obj.get("type")

                # Capture the last top-level user prompt (non-tool turn)
                if etype == "user":
                    msg = obj.get("message", {})
                    content = msg.get("content", [])
                    if isinstance(content, str) and content.strip():
                        last_prompt = content.strip()[:200]
                    elif isinstance(content, list):
                        for block in content:
                            if isinstance(block, dict):
                                if block.get("type") == "tool_result":
                                    # Extract tool output text
                                    tr = block.get("content", "")
                                    if isinstance(tr, str):
                                        text = tr
                                    elif isinstance(tr, list):
                                        text = "\n".join(
                                            item.get("text", "") for item in tr
                                            if isinstance(item, dict)
                                        )
                                    else:
                                        text = str(tr)
                                    if text.strip():
                                        outputs.append(text)
                                elif block.get("type") == "text":
                                    t = block.get("text", "").strip()
                                    if t and not outputs:  # first user text = prompt
                                        last_prompt = t[:200]
    except (OSError, PermissionError):
        pass
    return outputs, last_prompt


def cull_single(text: str, task: str = "") -> str | None:
    blocks = [{"role": "tool", "kind": "tool_output", "class": "tool", "text": text}]
    try:
        p = subprocess.run(
            [CULL, "compress", "--task", task],
            input=json.dumps(blocks),
            capture_output=True, text=True, timeout=10,
        )
        return p.stdout.rstrip("\n") if p.returncode == 0 else None
    except (subprocess.TimeoutExpired, OSError):
        return None


def cull_multi(blocks: list[dict], task: str = "") -> str | None:
    try:
        p = subprocess.run(
            [CULL, "compress", "--task", task],
            input=json.dumps(blocks),
            capture_output=True, text=True, timeout=60,
        )
        return p.stdout if p.returncode == 0 else None
    except (subprocess.TimeoutExpired, OSError):
        return None


def percentile(sorted_vals: list[float], pct: float) -> float:
    if not sorted_vals:
        return 0.0
    k = (len(sorted_vals) - 1) * pct / 100
    lo, hi = int(k), min(int(k) + 1, len(sorted_vals) - 1)
    return sorted_vals[lo] + (k - lo) * (sorted_vals[hi] - sorted_vals[lo])


# ---------------------------------------------------------------------------
# Phase 1: Corpus scan
# ---------------------------------------------------------------------------

print("=" * 70)
print("PHASE 1 — Corpus scan")
print("=" * 70)

jsonl_files = sorted(PROJECTS_DIR.rglob("*.jsonl"))
print(f"Transcript files found: {len(jsonl_files)}")

all_transcripts: list[tuple[Path, list[str], str | None]] = []
skipped = 0
for path in jsonl_files:
    outputs, prompt = extract_tool_results(path)
    if outputs:
        all_transcripts.append((path, outputs, prompt))
    else:
        skipped += 1

print(f"Transcripts with tool outputs: {len(all_transcripts)}  (skipped {skipped} with none)")

all_outputs: list[tuple[str, Path]] = []  # (text, source_path)
for path, outputs, _ in all_transcripts:
    for o in outputs:
        all_outputs.append((o, path))

token_counts = sorted([tok(t) for t, _ in all_outputs])
print(f"Total tool outputs: {len(all_outputs)}")
if token_counts:
    print(f"Token-size distribution (tiktoken o200k):")
    print(f"  p50  = {percentile(token_counts, 50):.0f} tokens")
    print(f"  p90  = {percentile(token_counts, 90):.0f} tokens")
    print(f"  max  = {max(token_counts)} tokens")
    print(f"  mean = {sum(token_counts)/len(token_counts):.0f} tokens")
    zero_or_tiny = sum(1 for t in token_counts if t < 5)
    print(f"  <5-token (empty/trivial): {zero_or_tiny} ({100*zero_or_tiny/len(token_counts):.1f}%)")


# ---------------------------------------------------------------------------
# Phase 2: Per-output compression (sample up to 200)
# ---------------------------------------------------------------------------

print()
print("=" * 70)
print("PHASE 2 — Per-output compression (sample up to 200)")
print("=" * 70)

# Filter out trivially tiny outputs (< 10 tokens) for meaningful ratios
substantial = [(t, p) for t, p in all_outputs if tok(t) >= 10]
print(f"Substantial outputs (>=10 tok): {len(substantial)}")

random.seed(42)
sample = random.sample(substantial, min(MAX_PER_OUTPUT_SAMPLE, len(substantial)))
print(f"Sampling {len(sample)} outputs...")

ratios: list[float] = []
by_category: dict[str, list[float]] = {"json": [], "prose": [], "error_short": [], "other": []}
sample_good: list[tuple[float, str]] = []   # (ratio, redacted snippet) compressed well
sample_bad: list[tuple[float, str]] = []    # compressed poorly

for i, (text, src_path) in enumerate(sample, 1):
    before = tok(text)
    # Recover task from transcript prompt (best-effort)
    task = ""
    for path, outputs, prompt in all_transcripts:
        if src_path == path and prompt:
            task = prompt[:100]
            break

    out = cull_single(text, task)
    if out is None:
        continue
    after = tok(out)
    ratio = 1.0 - (after / before) if before > 0 else 0.0
    ratios.append(ratio)

    # Categorize
    stripped = text.strip()
    if stripped.startswith("{") or stripped.startswith("["):
        cat = "json"
    elif "Error" in text[:50] or "error" in text[:50] or (before < 30 and "\n" not in text[:40]):
        cat = "error_short"
    elif any(text.strip().startswith(p) for p in ["#", "The ", "This ", "I ", "We ", "To ", "In "]):
        cat = "prose"
    else:
        cat = "other"
    by_category[cat].append(ratio)

    snippet = redact(text, 40)
    if ratio > 0.10:
        if len(sample_good) < 5:
            sample_good.append((ratio, snippet))
    else:
        if len(sample_bad) < 5:
            sample_bad.append((ratio, snippet))

ratios_sorted = sorted(ratios)
print(f"\nCompression ratio distribution (1 = perfect, 0 = no gain):")
print(f"  mean = {sum(ratios)/len(ratios):.3f}" if ratios else "  (no data)")
print(f"  p50  = {percentile(ratios_sorted, 50):.3f}")
print(f"  p90  = {percentile(ratios_sorted, 90):.3f}")
pct_above_10 = 100 * sum(1 for r in ratios if r > 0.10) / len(ratios) if ratios else 0
pct_above_30 = 100 * sum(1 for r in ratios if r > 0.30) / len(ratios) if ratios else 0
print(f"  % compressed >10%: {pct_above_10:.1f}%")
print(f"  % compressed >30%: {pct_above_30:.1f}%")

print("\nBy category:")
for cat, cat_ratios in by_category.items():
    if cat_ratios:
        mean_r = sum(cat_ratios) / len(cat_ratios)
        print(f"  {cat:<14} n={len(cat_ratios):>3}  mean={mean_r:.3f}")

print("\nSample: compressed well (>10%):")
for r, snip in sample_good:
    print(f"  [{r:.2f}] '{snip}...'")

print("\nSample: compressed poorly (<=10%):")
for r, snip in sample_bad:
    print(f"  [{r:.2f}] '{snip}...'")


# ---------------------------------------------------------------------------
# Phase 3: Cross-turn compression (10 largest transcripts)
# ---------------------------------------------------------------------------

print()
print("=" * 70)
print("PHASE 3 — Cross-turn multi-block compression (10 largest transcripts)")
print("=" * 70)

# Sort transcripts by total token count of their tool outputs
def transcript_tokens(entry: tuple) -> int:
    _, outputs, _ = entry
    return sum(tok(o) for o in outputs)

ranked = sorted(all_transcripts, key=transcript_tokens, reverse=True)
top_n = ranked[:CROSS_TURN_TOP_N]

print(f"Processing top {len(top_n)} transcripts by tool-output token volume...")
print()

cross_total_before = 0
cross_total_after = 0

for path, outputs, prompt in top_n:
    task = (prompt or "")[:100]
    before_total = sum(tok(o) for o in outputs)

    # Build multi-block input — annotate Read outputs with file_path if guessable
    blocks = []
    for o in outputs:
        # Heuristic: if it looks like file content (long, has code patterns), mark as file_read
        stripped = o.strip()
        kind = "tool_output"
        extra: dict = {}
        # Simple heuristic for file content vs command output
        if len(o) > 500 and ("def " in o or "function " in o or "class " in o or "import " in o):
            kind = "file_read"
        blocks.append({"role": "tool", "kind": kind, "class": "tool", "text": o, **extra})

    out = cull_multi(blocks, task)
    if out is None:
        print(f"  {path.name[:32]} — CULL FAILED")
        continue

    after_total = tok(out)
    ratio = 1.0 - (after_total / before_total) if before_total > 0 else 0.0
    cross_total_before += before_total
    cross_total_after += after_total

    fname = path.name[:32]
    print(f"  {fname}  n_outputs={len(outputs):>3}  "
          f"before={before_total:>6} tok  after={after_total:>6} tok  "
          f"ratio={ratio:.3f}")

if cross_total_before > 0:
    overall_ratio = 1.0 - (cross_total_after / cross_total_before)
    print(f"\nCross-turn totals:")
    print(f"  Before: {cross_total_before:,} tokens")
    print(f"  After:  {cross_total_after:,} tokens")
    print(f"  Ratio:  {overall_ratio:.3f} ({overall_ratio*100:.1f}% compression)")


# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

print()
print("=" * 70)
print("SUMMARY vs synthetic benchmark (68-88%)")
print("=" * 70)
if ratios:
    mean_ratio = sum(ratios) / len(ratios)
    p50_ratio = percentile(ratios_sorted, 50)
    print(f"Per-output (real traffic):  mean={mean_ratio:.1%}  p50={p50_ratio:.1%}")
if cross_total_before > 0:
    print(f"Cross-turn (real traffic):  {overall_ratio:.1%}")
print()
if ratios:
    if mean_ratio < 0.20:
        print("Real traffic compresses MUCH LESS than synthetic (68-88%).")
        print("Likely cause: short outputs, command echoes, error lines — not bulk JSON/file content.")
    elif mean_ratio < 0.50:
        print("Real traffic compresses moderately less than synthetic benchmarks.")
        print("Cross-turn dedup/supersession likely the main value driver on longer sessions.")
    else:
        print("Real traffic compresses well — close to or matching synthetic benchmark numbers.")
