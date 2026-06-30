#!/usr/bin/env python3
"""run_proof.py — Reproducible proof-of-compression for tare.

Runs each corpus item through the appropriate tare subcommand, measures
input vs output token counts (tiktoken o200k_base; falls back to chars/4
with a FALLBACK label), and writes results/proof.json + results/proof_table.md.

Usage:
    /Users/mark/git/.venv/bin/python3.14 crates/tare-bench/run_proof.py

The corpus files under crates/tare-bench/corpus/ are COMMITTED fixed samples
so the numbers here are reproducible against the same binary.
"""

import json
import os
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).parent
REPO_ROOT = HERE.parent.parent
TARE = REPO_ROOT / "target" / "release" / "tare"
CORPUS = HERE / "corpus"
RESULTS = HERE / "results"

# ---------------------------------------------------------------------------
# Tokenizer
# ---------------------------------------------------------------------------
try:
    import tiktoken
    _enc = tiktoken.get_encoding("o200k_base")
    TOKENIZER = "tiktoken o200k_base"
    def tok(s: str) -> int:
        return len(_enc.encode(s, disallowed_special=()))
except Exception:
    TOKENIZER = "chars/4 (FALLBACK — tiktoken unavailable)"
    def tok(s: str) -> int:
        return max(1, len(s) // 4)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
def run_tare(args: list[str], stdin_text: str, timeout: int = 30) -> str:
    result = subprocess.run(
        [str(TARE)] + args,
        input=stdin_text,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    if result.returncode != 0:
        raise RuntimeError(f"tare {' '.join(args)} failed:\n{result.stderr.strip()}")
    return result.stdout


def measure(name: str, content_type: str, cmd_args: list[str],
            input_text: str, display_input: str | None = None) -> dict:
    """Run tare, measure tokens and wall-clock time.

    display_input: optional alternative text to tokenize as "input" (the raw
    content before JSON wrapping, so the ratio reflects real context reduction).
    """
    t0 = time.perf_counter()
    output_text = run_tare(cmd_args, input_text)
    elapsed_ms = round((time.perf_counter() - t0) * 1000, 1)

    count_input = display_input if display_input is not None else input_text
    input_tokens = tok(count_input)
    output_tokens = tok(output_text)
    ratio = round(output_tokens / input_tokens, 4) if input_tokens > 0 else 1.0
    reduction_pct = round((1.0 - ratio) * 100, 1)

    return {
        "name": name,
        "content_type": content_type,
        "tokenizer": TOKENIZER,
        "tare_cmd": " ".join(["tare"] + cmd_args),
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "ratio": ratio,
        "reduction_pct": reduction_pct,
        "ms": elapsed_ms,
    }


# ---------------------------------------------------------------------------
# Corpus items
# ---------------------------------------------------------------------------
def build_corpus_items() -> list[dict]:
    items = []

    # 1. JSON array: cargo metadata packages --------------------------------
    meta_raw = (CORPUS / "cargo_metadata.json").read_text()
    meta_obj = json.loads(meta_raw)
    packages_array = json.dumps(meta_obj["packages"])
    items.append(measure(
        "cargo_packages",
        "json_array",
        ["compact-lossy"],
        packages_array,
        display_input=packages_array,
    ))

    # 2. Tabular: ps aux — pipe raw text; compact-lossy falls through to
    #    compact_text (line-based), which keeps first/last boundary + alert rows.
    ps_raw = (CORPUS / "ps_aux.txt").read_text()
    items.append(measure(
        "ps_aux",
        "tabular",
        ["compact-lossy"],
        ps_raw,
    ))

    # 3. Logs: app.log — raw text piped; compact_text path handles line units.
    #    String arrays in JSON are treated as all-anomalies by the JSON path,
    #    so raw text is the correct input for log/prose compact-lossy.
    log_raw = (CORPUS / "app.log").read_text()
    items.append(measure(
        "app_log",
        "logs",
        ["compact-lossy"],
        log_raw,
    ))

    # 4. Agent compress: multi-block context --------------------------------
    agent_ctx = (CORPUS / "agent_context.json").read_text()
    # Input tokens = all block texts concatenated (what would be in the context)
    agent_blocks = json.loads(agent_ctx)
    agent_raw_text = "\n\n".join(b["text"] for b in agent_blocks)
    items.append(measure(
        "agent_context",
        "agent_context",
        ["compress", "--task", "diagnose error rate spike in logs"],
        agent_ctx,
        display_input=agent_raw_text,
    ))

    # 5. Code: server.rs skeleton -------------------------------------------
    server_src = (CORPUS / "code" / "server.rs").read_text()
    items.append(measure(
        "server_rs",
        "code",
        ["skeletonize", "--path", "server.rs"],
        server_src,
    ))

    # 6. Code: json_crush.rs skeleton ---------------------------------------
    jc_src = (CORPUS / "code" / "json_crush.rs").read_text()
    items.append(measure(
        "json_crush_rs",
        "code",
        ["skeletonize", "--path", "json_crush.rs"],
        jc_src,
    ))

    # 7. Prose: README.md — raw text; compact-lossy uses sentence-splitting
    #    for prose (no JSON path since it's not a valid JSON array).
    prose_raw = (CORPUS / "prose.md").read_text()
    items.append(measure(
        "readme_prose",
        "prose",
        ["compact-lossy"],
        prose_raw,
    ))

    return items


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
def main() -> None:
    if not TARE.exists():
        sys.exit(f"ERROR: tare binary not found at {TARE}. Run: cargo build --release")

    print(f"tare binary : {TARE}")
    print(f"tokenizer   : {TOKENIZER}")
    print()

    RESULTS.mkdir(exist_ok=True)

    items = build_corpus_items()

    # Write proof.json
    proof_json = RESULTS / "proof.json"
    proof_json.write_text(json.dumps(items, indent=2) + "\n")
    print(f"wrote {proof_json}")

    # Write proof_table.md
    md_lines = [
        "# tare Compression Proof",
        "",
        f"**Tokenizer:** {TOKENIZER}",
        f"**Binary:** `target/release/tare` (release build)",
        "",
        "| Name | Content Type | Command | Input Tokens | Output Tokens | Ratio | Reduction | ms |",
        "|------|-------------|---------|-------------|--------------|-------|-----------|-----|",
    ]
    for r in items:
        cmd_short = r["tare_cmd"].split("--task")[0].strip()
        md_lines.append(
            f"| {r['name']} | {r['content_type']} | `{cmd_short}` "
            f"| {r['input_tokens']:,} | {r['output_tokens']:,} "
            f"| {r['ratio']:.3f} | {r['reduction_pct']:.1f}% | {r['ms']} |"
        )
    md_lines += [
        "",
        "## Notes",
        "",
        "- `compact-lossy`: input token count is the raw text (not JSON-wrapped stdin)",
        "  so the ratio reflects real LLM context savings.",
        "- `skeletonize`: bodies elided, signatures/types/imports kept; passthrough if nothing elidable.",
        "- `compress`: input is concatenated block texts; output is the compressed context string.",
        "  Superseded bash outputs are dropped by the dedup pass.",
        "",
        "All numbers are REAL — produced by running the actual release binary on the committed corpus.",
    ]
    proof_md = RESULTS / "proof_table.md"
    proof_md.write_text("\n".join(md_lines) + "\n")
    print(f"wrote {proof_md}")

    print()
    print(f"{'Name':<20} {'Type':<16} {'InTok':>7} {'OutTok':>7} {'Ratio':>6} {'Reduc':>7} {'ms':>6}")
    print("-" * 72)
    for r in items:
        print(
            f"{r['name']:<20} {r['content_type']:<16} "
            f"{r['input_tokens']:>7,} {r['output_tokens']:>7,} "
            f"{r['ratio']:>6.3f} {r['reduction_pct']:>6.1f}% {r['ms']:>6}"
        )


if __name__ == "__main__":
    main()
