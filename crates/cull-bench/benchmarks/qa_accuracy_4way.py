#!/usr/bin/env python3
"""4-way downstream QA accuracy: does an LLM still answer correctly on COMPRESSED context?

Compares uncompressed vs Cull vs Headroom vs lean-ctx, the metric that actually matters (and that
Headroom publishes on GSM8K/SQuAD/BFCL): accuracy preserved + compression achieved. Uses a capable
local model (no API key). Cull is lossless, so its accuracy should equal uncompressed while
compressing; lossy competitors trade accuracy for ratio.

Run with the headroom venv python; needs the release `cull` binary + /tmp/leanctx/lean-ctx.
"""
import json, os, subprocess, sys, contextlib, tempfile
import tiktoken

enc = tiktoken.get_encoding("o200k_base")
def ntok(s): return len(enc.encode(s))
CULL = os.environ.get("CULL_BIN", os.path.join(os.path.dirname(__file__), "..", "..", "..", "target", "release", "cull"))
LEANCTX = os.environ.get("LEANCTX_BIN", "/tmp/leanctx/lean-ctx")
MODEL = os.environ.get("QA_MODEL", "Qwen/Qwen2.5-3B-Instruct")

def cull_c(text):
    b = [{"role": "tool", "kind": "tool_output", "class": "tool", "text": text}]
    return subprocess.run([CULL, "compress", "--task", ""], input=json.dumps(b), capture_output=True, text=True).stdout.rstrip("\n")

def headroom_c(text):
    from headroom.compression.universal import compress as uc
    with contextlib.redirect_stdout(sys.stderr):
        return uc(text).compressed

def headroom_smartcrusher_c(text):
    # The lossy JSON path behind Headroom's 90.6% claim: keeps first/last/anomalies, DROPS the
    # middle. Falls back to the original when it can't crush (so non-JSON isn't penalized).
    from headroom.transforms.smart_crusher import SmartCrusher, SmartCrusherConfig
    with contextlib.redirect_stdout(sys.stderr):
        try:
            out = SmartCrusher(config=SmartCrusherConfig()).crush(text).compressed
            return out if out else text
        except Exception:
            return text

def leanctx_c(text):
    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as f:
        f.write(text); path = f.name
    out = subprocess.run([LEANCTX, "-c", f"cat {path}"], capture_output=True, text=True).stdout
    os.unlink(path)
    return out

def build_cases():
    cases = []
    # big JSON array: one anomalous record holds the answer
    users = [{"id": i, "name": f"user_{i}", "role": "user", "dept": "eng", "active": True} for i in range(120)]
    users[73] = {"id": 73, "name": "morgan_lee", "role": "admin", "dept": "security", "active": True}
    cases.append((json.dumps(users, indent=2), "What is the name of the only admin? Give just the name.", "morgan_lee"))
    # server metrics: one critical
    servers = [{"host": f"web-{i}", "cpu": 30 + i % 40, "status": "healthy"} for i in range(80)]
    servers[51] = {"host": "db-primary", "cpu": 98, "status": "critical", "error": "ERR_DISK_FULL_5521"}
    cases.append((json.dumps(servers, indent=2), "Which host is critical? Give just the host name.", "db-primary"))
    cases.append((json.dumps(servers, indent=2), "What is the error code on the critical host?", "ERR_DISK_FULL_5521"))
    # log with one fatal
    log = [f"2024-06-20T10:{i//60:02d}:{i%60:02d}Z INFO worker-{i%6} ok batch={i}" for i in range(200)]
    log[131] = "2024-06-20T10:02:11Z FATAL worker-3 OOMKilled code=ERR_OOM_8842 pod=ingest"
    cases.append(("\n".join(log), "What is the fatal error code in the log?", "ERR_OOM_8842"))
    # orders: highest value
    orders = [{"order": f"o{i}", "amount": (i * 7) % 500, "region": "us"} for i in range(100)]
    orders[88]["amount"] = 99999; orders[88]["order"] = "o-bulk-deal"
    cases.append((json.dumps(orders, indent=2), "Which order has the highest amount? Give the order id.", "o-bulk-deal"))
    # nested API response
    nested = {"status": "ok", "data": {"projects": [{"id": f"p{i}", "budget": i * 100} for i in range(60)]}}
    nested["data"]["projects"][40] = {"id": "p-quantum", "budget": 999999}
    cases.append((json.dumps(nested, indent=2), "Which project has the highest budget? Give the project id.", "p-quantum"))
    return cases

def main():
    from transformers import AutoModelForCausalLM, AutoTokenizer
    with contextlib.redirect_stdout(sys.stderr):
        tok = AutoTokenizer.from_pretrained(MODEL)
        model = AutoModelForCausalLM.from_pretrained(MODEL, torch_dtype="auto", device_map="cpu")

    def answer(ctx, q):
        prompt = f"{ctx}\n\nQuestion: {q}\nAnswer with just the value, nothing else:"
        msgs = [{"role": "user", "content": prompt}]
        text = tok.apply_chat_template(msgs, tokenize=False, add_generation_prompt=True)
        with contextlib.redirect_stdout(sys.stderr):
            ins = tok(text, return_tensors="pt")
            out = model.generate(**ins, max_new_tokens=24, do_sample=False)
        return tok.decode(out[0][ins.input_ids.shape[1]:], skip_special_tokens=True)

    variants = [("uncompressed", lambda t: t), ("cull", cull_c), ("headroom", headroom_c),
                ("headroom-crush", headroom_smartcrusher_c), ("lean-ctx", leanctx_c)]
    cases = build_cases()
    agg = {n: {"correct": 0, "ratios": []} for n, _ in variants}
    print(f"Model: {MODEL}   ({len(cases)} QA cases)\n")
    print(f"{'variant':<14} {'accuracy':>9} {'mean compression':>18}")
    print("-" * 44)
    for ctx, q, gt in cases:
        before = ntok(ctx)
        for name, fn in variants:
            comp = fn(ctx)
            ans = answer(comp, q)
            ok = gt.lower() in ans.lower()
            agg[name]["correct"] += ok
            agg[name]["ratios"].append(1 - ntok(comp) / before if before else 0)
    n = len(cases)
    for name, _ in variants:
        acc = agg[name]["correct"] / n
        comp = sum(agg[name]["ratios"]) / n
        print(f"{name:<14} {acc:>8.0%} {comp:>17.1%}")

if __name__ == "__main__":
    main()
