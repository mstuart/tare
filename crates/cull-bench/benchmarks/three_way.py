#!/usr/bin/env python3
"""Three-way benchmark: Cull vs Headroom vs RTK on commands + JSON + logs.
Measures compression ratio, latency, and fidelity (needle survival). RTK applies to commands only
(it intercepts specific shell commands); Headroom/Cull compress arbitrary text.

Run with the headroom venv python; needs the release `cull` binary + `rtk` on PATH.
"""
import json, os, subprocess, sys, time, contextlib
import tiktoken

enc = tiktoken.get_encoding("o200k_base")
def ntok(s): return len(enc.encode(s))
CULL = os.environ.get("CULL_BIN", os.path.join(os.path.dirname(__file__), "..", "..", "..", "target", "release", "cull"))

def best_ms(fn, runs=5):
    best, out = 1e18, None
    for _ in range(runs):
        t = time.perf_counter(); out = fn(); dt = (time.perf_counter() - t) * 1000
        best = min(best, dt)
    return out, best

def cull_compress(text, task=""):
    b = [{"role": "tool", "kind": "tool_output", "class": "tool", "text": text}]
    p = subprocess.run([CULL, "compress", "--task", task], input=json.dumps(b), capture_output=True, text=True)
    return p.stdout.rstrip("\n")

def headroom_compress(text):
    from headroom.compression.universal import compress as uc
    with contextlib.redirect_stdout(sys.stderr):
        return uc(text).compressed

def run_cmd(cmd):
    return subprocess.run(cmd, shell=True, capture_output=True, text=True).stdout

# ---- corpus: (name, raw_text, needle, rtk_cmd|None) ----
def build_corpus():
    items = []
    cmds = [("ps-aux", "ps aux"), ("ls-usrbin", "ls -la /usr/bin"),
            ("git-log", "git -C /Users/mark/git/cull log --oneline -60"),
            ("df", "df -h"), ("env", "env")]
    for name, cmd in cmds:
        raw = run_cmd(cmd)
        if raw.strip():
            needle = raw.strip().split("\n")[len(raw.strip().split("\n")) // 2][:30]  # a middle line fragment
            items.append((name, raw, needle, cmd))
    # JSON array (Headroom/Cull turf; RTK n/a)
    users = [{"id": i, "name": f"user_{i}", "role": "user", "region": "us-east-1", "active": True} for i in range(200)]
    users[137]["role"] = "admin"; users[137]["name"] = "zara_admin"
    items.append(("json-200", json.dumps(users, indent=2), "zara_admin", None))
    # build log
    log = "\n".join(f"2024-06-20T10:{i//60:02d}:{i%60:02d}Z INFO worker-{i%8} processed batch {i} ok latency={20+i%30}ms" for i in range(400))
    log = log.split("\n"); log[263] = "2024-06-20T10:04:23Z FATAL worker-3 OOM code=ERR_OOM_9931"; log = "\n".join(log)
    items.append(("log-400", log, "ERR_OOM_9931", None))
    return items

def main():
    corpus = build_corpus()
    print(f"{'input':<12} {'tokens':>7} | {'CULL':>22} | {'HEADROOM':>22} | {'RTK':>20}")
    print(f"{'':12} {'':>7} | {'ratio':>6} {'ms':>6} {'fid':>4} | {'ratio':>6} {'ms':>7} {'fid':>4} | {'ratio':>6} {'ms':>6} {'fid':>3}")
    print("-" * 96)
    for name, raw, needle, rtk_cmd in corpus:
        before = ntok(raw)
        cull_out, cull_ms = best_ms(lambda: cull_compress(raw))
        cr = 1 - ntok(cull_out) / before if before else 0
        cf = "ok" if needle in cull_out else "LOSS"
        hr_out, hr_ms = best_ms(lambda: headroom_compress(raw), runs=3)
        hr = 1 - ntok(hr_out) / before if before else 0
        hf = "ok" if needle in hr_out else "LOSS"
        if rtk_cmd:
            rtk_out, rtk_ms = best_ms(lambda: run_cmd(f"rtk {rtk_cmd}"), runs=3)
            rr = 1 - ntok(rtk_out) / before if before else 0
            rf = "ok" if needle in rtk_out else "?"
            rtk_cell = f"{rr:>5.0%} {rtk_ms:>6.0f} {rf:>3}"
        else:
            rtk_cell = f"{'n/a':>20}"
        print(f"{name:<12} {before:>7} | {cr:>5.0%} {cull_ms:>6.1f} {cf:>4} | {hr:>5.0%} {hr_ms:>7.1f} {hf:>4} | {rtk_cell}")

if __name__ == "__main__":
    main()
