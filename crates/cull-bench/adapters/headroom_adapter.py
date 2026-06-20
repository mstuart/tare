#!/usr/bin/env python3
# Headroom shell-out adapter for cull-bench (spec §12). Headroom (github.com/chopratejas/headroom,
# pip `headroom-ai`) is a direct competitor: a context-compression layer/proxy for LLM agents.
# Reads the context on stdin, writes the compressed context to stdout. Requires
# `pip install "headroom-ai[all]"` (has a Rust/PyO3 core → needs Python <= 3.13). If unavailable it
# exits non-zero so the harness's probe() excludes it rather than recording a misleading passthrough.
#
# NOTE: this file is named `headroom_adapter.py` (not `headroom.py`) on purpose — a module named
# `headroom.py` would shadow the installed `headroom` package on sys.path[0] and break the import.
#
# Verified API (headroom-ai 0.26.0): `from headroom import compress` is SYNCHRONOUS and returns a
# `CompressResult` with fields {messages, tokens_before, tokens_after, tokens_saved,
# compression_ratio, transforms_applied}; `.messages` is the compressed message list.
import sys
import contextlib


def main():
    context = sys.stdin.read()
    real_stdout = sys.stdout
    parts = []
    # Route any library chatter (loguru/onnxruntime/transformers) to stderr so stdout stays clean.
    with contextlib.redirect_stdout(sys.stderr):
        try:
            from headroom import compress
        except Exception as e:  # not installed / wrong Python → exclude via non-zero exit
            sys.stderr.write(f"headroom not installed: {e}\n")
            sys.exit(3)
        result = compress([{"role": "user", "content": context}], model="claude-3-5-sonnet")
        for m in result.messages:
            c = m.get("content")
            if isinstance(c, str):
                parts.append(c)
            elif isinstance(c, list):
                for blk in c:
                    if isinstance(blk, dict) and isinstance(blk.get("text"), str):
                        parts.append(blk["text"])
    real_stdout.write("\n".join(parts) if parts else context)


if __name__ == "__main__":
    main()
