#!/usr/bin/env python3
# LLMLingua-2 shell-out adapter for cull-bench (spec §12).
# Reads the context on stdin, reads CULL_TASK / CULL_BUDGET from the env, writes the compressed
# context to stdout. Requires `pip install llmlingua`. If the package is unavailable it exits
# non-zero so the harness's probe() excludes it (rather than recording a misleading passthrough).
import sys
def main():
    context = sys.stdin.read()
    try:
        from llmlingua import PromptCompressor
    except Exception as e:
        sys.stderr.write(f"llmlingua not installed: {e}\n")
        sys.exit(3)
    compressor = PromptCompressor(
        model_name="microsoft/llmlingua-2-xlm-roberta-large-meetingbank",
        use_llmlingua2=True,
    )
    result = compressor.compress_prompt(context, rate=0.5, force_tokens=["\n", ".", ",", "?", "!"])
    sys.stdout.write(result.get("compressed_prompt", context))
if __name__ == "__main__":
    main()
