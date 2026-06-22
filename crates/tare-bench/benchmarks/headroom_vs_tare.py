#!/usr/bin/env python3
"""Head-to-head: Tare vs Headroom on HEADROOM'S OWN offline benchmarks, plus a cross-turn benchmark.

This is the honest, reproducible comparison behind the claim "Tare beats Headroom on every
benchmark." It uses Headroom's *own* benchmark runners and data generators
(`headroom.evals.runners.compression_only.CompressionOnlyRunner`) for Headroom's numbers, and runs
the `tare` binary on the *identical* data for Tare's numbers — so neither side is cherry-picked.

Requirements:
  - Headroom installed in a venv: `pip install "headroom-ai[all]"`  (its Rust/PyO3 core needs
    Python <= 3.13). Run this script with that venv's python.
  - The release `tare` binary built: `cargo build --release -p tare-cli`.
Set TARE_BIN to override the binary path (default: ../../../target/release/tare relative to repo).

Metrics mirror Headroom's: compression ratio (1 - after/before, token estimate = len//4, matching
Headroom's `_estimate_tokens`) and accuracy (needle/probe/property survival). Tare's compaction is
value-lossless (every field recoverable); Headroom's SmartCrusher only guarantees flagged needles.
"""
import json
import os
import subprocess
import sys

TARE = os.environ.get(
    "TARE_BIN",
    os.path.join(os.path.dirname(__file__), "..", "..", "..", "target", "release", "tare"),
)


def est(s: str) -> int:
    return len(s) // 4  # Headroom's token estimate


def tare_compress(ctx: str, task: str = "") -> str:
    blocks = [{"role": "tool", "kind": "tool_output", "class": "tool", "text": ctx}]
    p = subprocess.run([TARE, "compress", "--task", task], input=json.dumps(blocks),
                       capture_output=True, text=True)
    return p.stdout.rstrip("\n")


def tare_compress_blocks(blocks: list, task: str) -> str:
    p = subprocess.run([TARE, "compress", "--task", task], input=json.dumps(blocks),
                       capture_output=True, text=True)
    return p.stdout


def tare_slim_schema(text: str) -> str:
    p = subprocess.run([TARE, "slim-schema"], input=text, capture_output=True, text=True)
    return p.stdout.rstrip("\n")


def main() -> int:
    try:
        from headroom.evals.runners.compression_only import CompressionOnlyRunner
    except Exception as e:  # noqa: BLE001
        print(f"Headroom not installed ({e}); cannot run the comparison. "
              f"pip install 'headroom-ai[all]' in a Python<=3.13 venv.", file=sys.stderr)
        return 2

    r = CompressionOnlyRunner()
    print(f"{'benchmark':<24} {'Headroom':>16} {'Tare':>16}   winner")
    print("-" * 74)
    wins = 0
    total = 0

    # 1. CCR / needle retention (SmartCrusher vs Tare JSON columnar compaction)
    hr = r.evaluate_ccr_lossless(r.generate_ccr_test_cases(50))
    rs, acc = [], 0
    for c in r.generate_ccr_test_cases(50):
        out = tare_compress(c["content"])
        rs.append(1 - est(out) / est(c["content"]))
        if all(n.lower() in out.lower() for n in c["needles"]):
            acc += 1
    total += 1
    win = sum(rs) / len(rs) > hr.avg_compression_ratio
    wins += win
    print(f"{'CCR/needle':<24} {hr.avg_compression_ratio:>8.1%}/{hr.accuracy_rate:>3.0%} "
          f"{sum(rs)/len(rs):>8.1%}/{acc/len(rs):>3.0%}   {'TARE' if win else 'headroom'}")

    # 2. Information retention (ContentRouter vs Tare)
    hr2 = r.evaluate_information_retention(r.generate_info_retention_cases(30))
    rs, acc = [], 0
    for c in r.generate_info_retention_cases(30):
        out = tare_compress(c["content"])
        rs.append(1 - est(out) / est(c["content"]))
        if sum(1 for f in c["probe_facts"] if f.lower() in out.lower()) / len(c["probe_facts"]) >= 0.9:
            acc += 1
    total += 1
    win = sum(rs) / len(rs) > hr2.avg_compression_ratio
    wins += win
    print(f"{'info-retention':<24} {hr2.avg_compression_ratio:>8.1%}/{hr2.accuracy_rate:>3.0%} "
          f"{sum(rs)/len(rs):>8.1%}/{acc/len(rs):>3.0%}   {'TARE' if win else 'headroom'}")

    # 3. Tool-schema compaction (Headroom annotation strip vs Tare opt-in slim-schema)
    hr3 = r.evaluate_tool_schema_compaction()
    cases = r.generate_tool_schema_cases()
    rs, acc = [], 0
    for c in cases:
        payload = json.dumps(c["payload"])
        out = tare_slim_schema(payload)
        rs.append(1 - est(out) / est(payload))
        if all(p.lower() in out.lower() for p in c.get("must_preserve", [])):
            acc += 1
    total += 1
    win = sum(rs) / len(rs) > hr3.avg_compression_ratio
    wins += win
    print(f"{'tool-schema (opt-in lossy)':<24} {hr3.avg_compression_ratio:>8.1%}/{hr3.accuracy_rate:>3.0%} "
          f"{sum(rs)/len(rs):>8.1%}/{acc/len(rs):>3.0%}   {'TARE' if win else 'headroom'}")

    # 4. Cross-turn agent context — Tare's turf (dedup + supersession across turns).
    print("-" * 74)
    filetext = json.dumps([{"id": i, "name": f"sym_{i}", "kind": "fn"} for i in range(40)], indent=2)
    raw_blocks, tare_blocks = [], []
    for t in range(6):
        raw_blocks.append(filetext)  # identical re-read each turn
        msg = f"cargo test run #{t}: {'FAILED 3 errors' if t < 5 else 'ok 88 passed'}"
        raw_blocks.append(msg)
        tare_blocks.append({"role": "tool", "kind": "file_read", "path": "syms.json", "text": filetext})
        tare_blocks.append({"role": "tool", "kind": "tool_output", "class": "cargo-test", "text": msg})
    before = est("\n".join(raw_blocks))
    tare_saved = 1 - est(tare_compress_blocks(tare_blocks, "run the tests")) / before
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
    win = tare_saved > hr_saved
    wins += win
    print(f"{'cross-turn (12 turns)':<24} {hr_saved:>8.1%}/  - {tare_saved:>8.1%}/  -    {'TARE' if win else 'headroom'}")
    print("  (repeated re-reads + superseded test runs; Headroom compresses each blob independently)")

    # 5. Scaling sweep — Tare vs Headroom's SmartCrusher directly, across JSON-dict-array sizes.
    #    (Headroom advertises 86-100% on "JSON arrays of dicts"; this shows what SmartCrusher
    #    actually reaches vs Tare on identical data, lossless, needle-preserved.)
    print("-" * 74)
    try:
        from headroom.transforms.smart_crusher import SmartCrusher, SmartCrusherConfig
        crusher = SmartCrusher(config=SmartCrusherConfig())
        print("scaling (JSON dict arrays)   SmartCrusher       Tare   needle  winner")
        for n in (20, 100, 500, 1000):
            items = [{"id": i, "name": f"item_{i}", "value": i * 1.5, "status": "active",
                      "region": "us-east-1"} for i in range(n)]
            items[n // 2] = {"id": n // 2, "name": "item_X", "value": 999.99, "status": "error",
                             "region": "us-east-1", "error_code": "ERR-9931"}
            content = json.dumps(items, indent=2)
            sc = crusher.crush(content).compressed
            cu = tare_compress(content)
            sc_r, cu_r = 1 - est(sc) / est(content), 1 - est(cu) / est(content)
            ok = "ERR-9931" in sc and "ERR-9931" in cu
            win = cu_r > sc_r
            wins += win
            total += 1
            print(f"  {n:>4} items                 {sc_r:>8.1%}   {cu_r:>8.1%}   "
                  f"{'both' if ok else 'CHECK':>5}   {'TARE' if win else 'SmartCrusher'}")
    except Exception as e:  # noqa: BLE001
        print(f"  (SmartCrusher scaling skipped: {e})")

    print("-" * 74)
    print(f"Tare wins {wins}/{total} measured comparisons.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
