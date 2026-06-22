#!/usr/bin/env python3
"""Local-LLM answer-equivalence + columnar-readability validation for Tare.

Measures two things:
1. Answer equivalence: does Tare-compressed context preserve the LLM's ability to answer
   factual questions, vs the original context and headroom-compressed context?
2. Columnar readability: can the LLM read Tare's ⟪jc1⟫ columnar format and extract values?

Requirements:
  - /tmp/tare-headroom-venv313/bin/python (transformers + torch + headroom-ai + tiktoken)
  - /Users/mark/git/tare/target/release/tare binary

Run:
  /tmp/tare-headroom-venv313/bin/python answer_equivalence.py
"""

import json
import os
import subprocess
import sys
import textwrap

TARE = os.environ.get(
    "TARE_BIN",
    os.path.join(os.path.dirname(__file__), "..", "..", "..", "target", "release", "tare"),
)

MODEL_CANDIDATES = [
    "Qwen/Qwen2.5-0.5B-Instruct",
    "Qwen/Qwen2.5-1.5B-Instruct",
    "HuggingFaceTB/SmolLM2-1.7B-Instruct",
]

MAX_NEW_TOKENS = 64


# ---------------------------------------------------------------------------
# Compression helpers
# ---------------------------------------------------------------------------

def tare_compress(ctx: str, task: str = "") -> str:
    blocks = [{"role": "tool", "kind": "tool_output", "class": "tool", "text": ctx}]
    p = subprocess.run(
        [TARE, "compress", "--task", task],
        input=json.dumps(blocks),
        capture_output=True,
        text=True,
    )
    return p.stdout.rstrip("\n")


def headroom_compress(ctx: str) -> str:
    try:
        from headroom.compression.universal import compress as uc
        return uc(ctx).compressed
    except Exception:  # noqa: BLE001
        return ctx


def token_count(text: str) -> int:
    import tiktoken
    enc = tiktoken.get_encoding("o200k_base")
    return len(enc.encode(text))


def compression_ratio(original: str, compressed: str) -> float:
    before = token_count(original)
    if before == 0:
        return 0.0
    after = token_count(compressed)
    return 1.0 - after / before


# ---------------------------------------------------------------------------
# Model loading
# ---------------------------------------------------------------------------

def load_model():
    from transformers import AutoTokenizer, AutoModelForCausalLM
    import torch

    for model_id in MODEL_CANDIDATES:
        print(f"  Trying {model_id} ...", flush=True)
        try:
            tokenizer = AutoTokenizer.from_pretrained(model_id)
            model = AutoModelForCausalLM.from_pretrained(
                model_id,
                device_map="cpu",
                torch_dtype="auto",
            )
            print(f"  Loaded: {model_id}")
            return tokenizer, model, model_id
        except Exception as e:  # noqa: BLE001
            print(f"  Failed ({e}), trying next ...", flush=True)

    print("ERROR: all model candidates failed to load.", file=sys.stderr)
    sys.exit(1)


def generate(tokenizer, model, model_id: str, prompt: str) -> str:
    import torch

    messages = [{"role": "user", "content": prompt}]
    text = tokenizer.apply_chat_template(messages, tokenize=False, add_generation_prompt=True)
    inputs = tokenizer(text, return_tensors="pt")
    with torch.no_grad():
        out = model.generate(
            **inputs,
            max_new_tokens=MAX_NEW_TOKENS,
            do_sample=False,
        )
    # Strip the prompt tokens from output
    new_tokens = out[0][inputs["input_ids"].shape[-1]:]
    return tokenizer.decode(new_tokens, skip_special_tokens=True).strip()


def contains_ground_truth(answer: str, ground_truth: str) -> bool:
    return ground_truth.lower() in answer.lower()


# ---------------------------------------------------------------------------
# Part 1: Answer equivalence on the 8 headroom tool_output_samples
# ---------------------------------------------------------------------------

def run_answer_equivalence(tokenizer, model, model_id: str):
    from headroom.evals.datasets import load_tool_output_samples

    cases = list(load_tool_output_samples())
    n = len(cases)
    print(f"\n=== Part 1: Answer Equivalence ({n} cases) ===\n")
    print(f"Model: {model_id}\n")

    variants = ["original", "tare", "headroom"]
    correct = {v: 0 for v in variants}
    ratios = {v: [] for v in variants}

    for i, case in enumerate(cases):
        ctx_orig = case.context
        ctx_tare = tare_compress(ctx_orig, case.query)
        ctx_head = headroom_compress(ctx_orig)

        ctxs = {
            "original": ctx_orig,
            "tare": ctx_tare,
            "headroom": ctx_head,
        }

        hits = {}
        for variant, ctx in ctxs.items():
            prompt = f"{ctx}\n\nQuestion: {case.query}\nAnswer concisely:"
            answer = generate(tokenizer, model, model_id, prompt)
            hit = contains_ground_truth(answer, case.ground_truth)
            hits[variant] = hit
            if hit:
                correct[variant] += 1
            ratio = compression_ratio(ctx_orig, ctx) if variant != "original" else 0.0
            ratios[variant].append(ratio)

        print(
            f"  case {i}: query={repr(case.query)!s:<50} gt={repr(case.ground_truth)!s:<20}"
            f"  orig={hits['original']} tare={hits['tare']} head={hits['headroom']}"
        )

    # Actually we already called generate above — collect results from correct dict
    print(f"\n{'Variant':<12}  {'Correct':>7}  {'Accuracy':>8}  {'Mean Compression':>16}")
    print("-" * 52)
    for v in variants:
        mean_ratio = sum(ratios[v]) / len(ratios[v]) if ratios[v] else 0.0
        acc = correct[v] / n
        print(f"  {v:<10}  {correct[v]:>4}/{n:<3}  {acc:>8.1%}  {mean_ratio:>16.1%}")

    return correct, ratios, n


# ---------------------------------------------------------------------------
# Part 2: Columnar readability probe
# ---------------------------------------------------------------------------

COLUMNAR_CASES = [
    {
        "description": "servers with one critical status",
        "query": "Which server has status critical?",
        "ground_truth": "server-b",
        "data": [
            {"id": i, "name": f"server-{'abcdefgh'[i]}", "status": "critical" if i == 1 else "ok", "cpu": 20 + i * 10}
            for i in range(8)
        ],
    },
    {
        "description": "users with one admin role",
        "query": "Who is the admin user?",
        "ground_truth": "charlie",
        "data": [
            {"id": i, "username": name, "role": "admin" if name == "charlie" else "user", "active": True}
            for i, name in enumerate(["alice", "bob", "charlie", "diana", "eve", "frank", "grace", "henry"])
        ],
    },
    {
        "description": "deployments with one failed",
        "query": "Which deployment failed?",
        "ground_truth": "deploy-003",
        "data": [
            {"deploy_id": f"deploy-{str(i).zfill(3)}", "env": "prod", "status": "failed" if i == 3 else "success", "duration_s": 45 + i}
            for i in range(1, 9)
        ],
    },
    {
        "description": "products with highest price",
        "query": "Which product costs 299.99?",
        "ground_truth": "widget-pro",
        "data": [
            {"sku": f"widget-{'pro' if i == 4 else str(i).zfill(3)}", "price": 299.99 if i == 4 else 9.99 + i, "in_stock": True, "category": "tools"}
            for i in range(8)
        ],
    },
    {
        "description": "services with one degraded health",
        "query": "Which service is degraded?",
        "ground_truth": "payment-svc",
        "data": [
            {"service": name, "health": "degraded" if name == "payment-svc" else "healthy", "latency_ms": 10 + i * 5, "replicas": 3}
            for i, name in enumerate(["auth-svc", "payment-svc", "cart-svc", "catalog-svc", "search-svc", "notify-svc", "media-svc", "api-gw"])
        ],
    },
]


def run_columnar_readability(tokenizer, model, model_id: str):
    print(f"\n=== Part 2: Columnar Readability Probe ({len(COLUMNAR_CASES)} cases) ===\n")
    print(f"Model: {model_id}\n")

    correct = 0
    print(f"  {'#':<2}  {'Description':<40}  {'GT':<15}  {'Answer':<30}  {'Hit'}")
    print("  " + "-" * 100)

    for i, case in enumerate(COLUMNAR_CASES):
        raw_json = json.dumps(case["data"], indent=2)
        columnar = tare_compress(raw_json, case["query"])

        # Check we actually got columnar form (⟪jc1⟫ marker)
        is_columnar = "⟪jc1⟫" in columnar

        prompt = textwrap.dedent(f"""\
            The following data is in a compact columnar format.
            The first line after ⟪jc1⟫ lists the column names as a JSON array.
            Each subsequent line is a JSON array of values for those columns.
            Use this format to answer the question.

            {columnar}

            Question: {case["query"]}
            Answer concisely with just the value:""")

        answer = generate(tokenizer, model, model_id, prompt)
        hit = contains_ground_truth(answer, case["ground_truth"])
        if hit:
            correct += 1

        truncated_answer = answer[:30] if len(answer) > 30 else answer
        print(
            f"  {i+1:<2}  {case['description']:<40}  {case['ground_truth']:<15}  "
            f"{truncated_answer:<30}  {'YES' if hit else 'NO'}"
            f"  {'[columnar]' if is_columnar else '[NOT columnar!]'}"
        )

    print(f"\nColumnar readability: {correct}/{len(COLUMNAR_CASES)}")
    return correct, len(COLUMNAR_CASES)


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main() -> int:
    print("Loading model ...", flush=True)
    tokenizer, model, model_id = load_model()

    # Part 1 — answer equivalence
    correct, ratios, n = run_answer_equivalence(tokenizer, model, model_id)

    # Part 2 — columnar readability
    col_correct, col_total = run_columnar_readability(tokenizer, model, model_id)

    # Summary
    print("\n" + "=" * 60)
    print("SUMMARY")
    print("=" * 60)
    print(f"Model used: {model_id}")
    print()
    print("Answer Equivalence (8 cases):")
    print(f"  original:  {correct['original']}/{n} = {correct['original']/n:.0%}")
    print(f"  tare:      {correct['tare']}/{n} = {correct['tare']/n:.0%}")
    print(f"  headroom:  {correct['headroom']}/{n} = {correct['headroom']/n:.0%}")
    tare_ratio = sum(ratios['tare']) / n if n else 0.0
    head_ratio = sum(ratios['headroom']) / n if n else 0.0
    print(f"\nMean compression ratio:")
    print(f"  tare:      {tare_ratio:.1%}")
    print(f"  headroom:  {head_ratio:.1%}")
    print()

    # Verdict
    tare_acc = correct['tare'] / n
    orig_acc = correct['original'] / n
    if tare_acc >= orig_acc:
        print(f"VERDICT: Tare PRESERVES task accuracy ({tare_acc:.0%} vs original {orig_acc:.0%})")
    else:
        print(f"VERDICT: Tare DEGRADES task accuracy ({tare_acc:.0%} vs original {orig_acc:.0%}) — CONCERN")

    col_rate = col_correct / col_total
    print(f"\nColumnar Readability: {col_correct}/{col_total} = {col_rate:.0%}")
    if col_rate >= 0.8:
        print("VERDICT: LLM reads columnar format RELIABLY")
    elif col_rate >= 0.4:
        print("VERDICT: LLM reads columnar format PARTIALLY — CONCERN")
    else:
        print("VERDICT: LLM CANNOT reliably read columnar format — CRITICAL FINDING")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
