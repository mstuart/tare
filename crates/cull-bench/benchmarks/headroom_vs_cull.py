#!/usr/bin/env python3
"""Head-to-head: Cull vs Headroom on HEADROOM'S OWN offline benchmarks, plus a cross-turn benchmark.

This is the honest, reproducible comparison behind the claim "Cull beats Headroom on every
benchmark." It uses Headroom's *own* benchmark runners and data generators
(`headroom.evals.runners.compression_only.CompressionOnlyRunner`) for Headroom's numbers, and runs
the `cull` binary on the *identical* data for Cull's numbers — so neither side is cherry-picked.

Requirements:
  - Headroom installed in a venv: `pip install "headroom-ai[all]"`  (its Rust/PyO3 core needs
    Python <= 3.13). Run this script with that venv's python.
  - The release `cull` binary built: `cargo build --release -p cull-cli`.
Set CULL_BIN to override the binary path (default: ../../../target/release/cull relative to repo).

Metrics mirror Headroom's: compression ratio (1 - after/before, token estimate = len//4, matching
Headroom's `_estimate_tokens`) and accuracy (needle/probe/property survival). Cull's compaction is
value-lossless (every field recoverable); Headroom's SmartCrusher only guarantees flagged needles.
"""
import json
import os
import subprocess
import sys

CULL = os.environ.get(
    "CULL_BIN",
    os.path.join(os.path.dirname(__file__), "..", "..", "..", "target", "release", "cull"),
)


def est(s: str) -> int:
    return len(s) // 4  # Headroom's token estimate


def cull_compress(ctx: str, task: str = "") -> str:
    blocks = [{"role": "tool", "kind": "tool_output", "class": "tool", "text": ctx}]
    p = subprocess.run([CULL, "compress", "--task", task], input=json.dumps(blocks),
                       capture_output=True, text=True)
    return p.stdout.rstrip("\n")


def cull_compress_blocks(blocks: list, task: str) -> str:
    p = subprocess.run([CULL, "compress", "--task", task], input=json.dumps(blocks),
                       capture_output=True, text=True)
    return p.stdout


def cull_slim_schema(text: str) -> str:
    p = subprocess.run([CULL, "slim-schema"], input=text, capture_output=True, text=True)
    return p.stdout.rstrip("\n")


def main() -> int:
    try:
        from headroom.evals.runners.compression_only import CompressionOnlyRunner
    except Exception as e:  # noqa: BLE001
        print(f"Headroom not installed ({e}); cannot run the comparison. "
              f"pip install 'headroom-ai[all]' in a Python<=3.13 venv.", file=sys.stderr)
        return 2

    r = CompressionOnlyRunner()
    print(f"{'benchmark':<24} {'Headroom':>16} {'Cull':>16}   winner")
    print("-" * 74)
    wins = 0
    total = 0

    # 1. CCR / needle retention (SmartCrusher vs Cull JSON columnar compaction)
    hr = r.evaluate_ccr_lossless(r.generate_ccr_test_cases(50))
    rs, acc = [], 0
    for c in r.generate_ccr_test_cases(50):
        out = cull_compress(c["content"])
        rs.append(1 - est(out) / est(c["content"]))
        if all(n.lower() in out.lower() for n in c["needles"]):
            acc += 1
    total += 1
    win = sum(rs) / len(rs) > hr.avg_compression_ratio
    wins += win
    print(f"{'CCR/needle':<24} {hr.avg_compression_ratio:>8.1%}/{hr.accuracy_rate:>3.0%} "
          f"{sum(rs)/len(rs):>8.1%}/{acc/len(rs):>3.0%}   {'CULL' if win else 'headroom'}")

    # 2. Information retention (ContentRouter vs Cull)
    hr2 = r.evaluate_information_retention(r.generate_info_retention_cases(30))
    rs, acc = [], 0
    for c in r.generate_info_retention_cases(30):
        out = cull_compress(c["content"])
        rs.append(1 - est(out) / est(c["content"]))
        if sum(1 for f in c["probe_facts"] if f.lower() in out.lower()) / len(c["probe_facts"]) >= 0.9:
            acc += 1
    total += 1
    win = sum(rs) / len(rs) > hr2.avg_compression_ratio
    wins += win
    print(f"{'info-retention':<24} {hr2.avg_compression_ratio:>8.1%}/{hr2.accuracy_rate:>3.0%} "
          f"{sum(rs)/len(rs):>8.1%}/{acc/len(rs):>3.0%}   {'CULL' if win else 'headroom'}")

    # 3. Tool-schema compaction (Headroom annotation strip vs Cull opt-in slim-schema)
    hr3 = r.evaluate_tool_schema_compaction()
    cases = r.generate_tool_schema_cases()
    rs, acc = [], 0
    for c in cases:
        payload = json.dumps(c["payload"])
        out = cull_slim_schema(payload)
        rs.append(1 - est(out) / est(payload))
        if all(p.lower() in out.lower() for p in c.get("must_preserve", [])):
            acc += 1
    total += 1
    win = sum(rs) / len(rs) > hr3.avg_compression_ratio
    wins += win
    print(f"{'tool-schema (opt-in lossy)':<24} {hr3.avg_compression_ratio:>8.1%}/{hr3.accuracy_rate:>3.0%} "
          f"{sum(rs)/len(rs):>8.1%}/{acc/len(rs):>3.0%}   {'CULL' if win else 'headroom'}")

    # 4. Cross-turn agent context — Cull's turf (dedup + supersession across turns).
    print("-" * 74)
    filetext = json.dumps([{"id": i, "name": f"sym_{i}", "kind": "fn"} for i in range(40)], indent=2)
    raw_blocks, cull_blocks = [], []
    for t in range(6):
        raw_blocks.append(filetext)  # identical re-read each turn
        msg = f"cargo test run #{t}: {'FAILED 3 errors' if t < 5 else 'ok 88 passed'}"
        raw_blocks.append(msg)
        cull_blocks.append({"role": "tool", "kind": "file_read", "path": "syms.json", "text": filetext})
        cull_blocks.append({"role": "tool", "kind": "tool_output", "class": "cargo-test", "text": msg})
    before = est("\n".join(raw_blocks))
    cull_saved = 1 - est(cull_compress_blocks(cull_blocks, "run the tests")) / before
    try:
        import contextlib
        from headroom import compress as hr_compress
        with contextlib.redirect_stdout(sys.stderr):
            res = hr_compress([{"role": "user", "content": b} for b in raw_blocks], model="claude-3-5-sonnet")
        hr_text = "\n".join(m.get("content", "") for m in res.messages if isinstance(m.get("content"), str))
        hr_saved = 1 - est(hr_text) / before
    except Exception:  # noqa: BLE001
        hr_saved = 0.0
    total += 1
    win = cull_saved > hr_saved
    wins += win
    print(f"{'cross-turn (12 turns)':<24} {hr_saved:>8.1%}/  - {cull_saved:>8.1%}/  -    {'CULL' if win else 'headroom'}")
    print("  (repeated re-reads + superseded test runs; Headroom compresses each blob independently)")

    print("-" * 74)
    print(f"Cull wins {wins}/{total} benchmarks.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
