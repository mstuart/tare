"""Pytest suite for tare.integrations.

Prerequisites
-------------
Run ``maturin develop`` (or ``pip install -e .``) inside ``crates/tare-py``
before executing these tests so the compiled ``tare._tare`` extension is
available on ``sys.path``.

All tests are skipped gracefully when the native extension is missing so CI
pipelines that don't build Rust still get green-by-skip rather than red.
"""

from __future__ import annotations

import asyncio
import json
import sys
import types

import pytest

# Skip the whole module if the Rust extension hasn't been built yet.
tare = pytest.importorskip(
    "tare",
    reason="tare native extension not available; run 'maturin develop' first",
)


# ---------------------------------------------------------------------------
# Lazy-import safety
# ---------------------------------------------------------------------------


def test_integrations_imports_without_optional_deps() -> None:
    """tare.integrations must import (and re-import) cleanly without any
    optional framework installed.

    Strategy: temporarily evict each optional package from sys.modules so any
    eager top-level import would fail, reload integrations, confirm no exception
    (lazy imports inside methods are the only allowed path to those packages).
    """
    optional = [
        "litellm",
        "langchain",
        "langchain_core",
        "agno",
        "strands",
        "anthropic",
        "openai",
    ]
    saved: dict[str, types.ModuleType] = {}

    for name in optional:
        if name in sys.modules:
            saved[name] = sys.modules.pop(name)

    try:
        import importlib

        # Remove cached integrations module so it re-executes the top-level body.
        sys.modules.pop("tare.integrations", None)
        from tare import integrations  # noqa: F401 — must not raise

        importlib.reload(integrations)  # full re-import with optional deps absent
    finally:
        sys.modules.update(saved)


# ---------------------------------------------------------------------------
# compress_messages
# ---------------------------------------------------------------------------


def test_compress_messages_shrinks_redundant_tool_outputs() -> None:
    """compress_messages should drop superseded same-class tool outputs."""
    from tare.integrations import compress_messages

    # Two tool messages from the same tool (class="bash").  Tare's structural
    # pass supersedes the earlier one, so the compressed output should be
    # strictly shorter than the original.
    filler = "test line\n" * 40
    messages = [
        {"role": "tool", "name": "bash", "content": f"FAILED: test_foo\n{filler}"},
        {"role": "user", "content": "please fix the failing test"},
        {"role": "tool", "name": "bash", "content": f"PASSED: test_foo\n{filler}"},
    ]

    result = compress_messages(messages, task="fix failing test")

    assert isinstance(result, list), "result must be a list"
    assert len(result) >= 1, "result must have at least one element"
    assert all(
        isinstance(m, dict) and "role" in m and "content" in m for m in result
    ), "each element must be a dict with role and content"

    original_chars = sum(len(m.get("content", "")) for m in messages)
    compressed_chars = sum(len(m.get("content", "")) for m in result)
    assert compressed_chars < original_chars, (
        f"Expected compression: compressed={compressed_chars} < original={original_chars}"
    )


def test_compress_messages_empty_returns_empty() -> None:
    from tare.integrations import compress_messages

    assert compress_messages([]) == []


def test_compress_messages_returns_valid_role() -> None:
    from tare.integrations import compress_messages

    messages = [{"role": "user", "content": "hello world"}]
    result = compress_messages(messages)
    assert result[0]["role"] in ("system", "user", "assistant", "tool")


def test_compress_messages_multipart_content() -> None:
    """Content as a list of parts is handled without error."""
    from tare.integrations import compress_messages

    messages = [
        {
            "role": "user",
            "content": [
                {"type": "text", "text": "What is in this image?"},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,abc"}},
            ],
        }
    ]
    result = compress_messages(messages)
    assert isinstance(result, list) and len(result) >= 1


# ---------------------------------------------------------------------------
# CompressionMiddleware
# ---------------------------------------------------------------------------


def test_compression_middleware_constructible() -> None:
    from tare.integrations import CompressionMiddleware

    async def dummy_app(scope: dict, receive: object, send: object) -> None:
        pass

    mw = CompressionMiddleware(dummy_app, task="test task")
    assert mw.app is dummy_app
    assert mw.task == "test task"


def test_compression_middleware_rejects_large_chunked_body() -> None:
    from tare.integrations import CompressionMiddleware

    chunks = [b"1234", b"5678"]
    received_events = iter(
        [
            {"type": "http.request", "body": chunks[0], "more_body": True},
            {"type": "http.request", "body": chunks[1], "more_body": False},
        ]
    )
    sent_events: list[dict] = []

    async def receive() -> dict:
        return next(received_events)

    async def send(event: dict) -> None:
        sent_events.append(event)

    async def app(scope: dict, app_receive: object, app_send: object) -> None:
        await app_receive()  # type: ignore[operator]

    middleware = CompressionMiddleware(app, max_body_bytes=7)
    asyncio.run(middleware({"type": "http"}, receive, send))

    assert sent_events[0]["status"] == 413
    assert sent_events[1]["body"] == b"Request body too large"


def test_compression_middleware_does_not_replace_started_response() -> None:
    import pytest

    from tare.integrations import CompressionMiddleware, _RequestBodyTooLarge

    received_events = iter(
        [
            {"type": "http.request", "body": b"1234", "more_body": True},
            {"type": "http.request", "body": b"5678", "more_body": False},
        ]
    )
    sent_events: list[dict] = []

    async def receive() -> dict:
        return next(received_events)

    async def send(event: dict) -> None:
        sent_events.append(event)

    async def app(scope: dict, app_receive: object, app_send: object) -> None:
        await app_send({"type": "http.response.start", "status": 200})  # type: ignore[operator]
        await app_receive()  # type: ignore[operator]

    middleware = CompressionMiddleware(app, max_body_bytes=7)
    with pytest.raises(_RequestBodyTooLarge):
        asyncio.run(middleware({"type": "http"}, receive, send))

    assert sent_events == [{"type": "http.response.start", "status": 200}]


def test_compression_middleware_rejects_large_content_length_early() -> None:
    from tare.integrations import CompressionMiddleware

    receive_called = False
    app_called = False
    sent_events: list[dict] = []

    async def receive() -> dict:
        nonlocal receive_called
        receive_called = True
        return {"type": "http.request", "body": b""}

    async def send(event: dict) -> None:
        sent_events.append(event)

    async def app(scope: dict, app_receive: object, app_send: object) -> None:
        nonlocal app_called
        app_called = True

    middleware = CompressionMiddleware(app, max_body_bytes=7)
    scope = {"type": "http", "headers": [(b"content-length", b"8")]}
    asyncio.run(middleware(scope, receive, send))

    assert sent_events[0]["status"] == 413
    assert not receive_called
    assert not app_called


def test_compression_middleware_compresses_json_body() -> None:
    """_maybe_compress_body reduces total message content for redundant input."""
    from tare.integrations import CompressionMiddleware

    async def dummy_app(scope: dict, receive: object, send: object) -> None:
        pass

    mw = CompressionMiddleware(dummy_app)
    filler = "line\n" * 40
    payload = {
        "model": "gpt-4o",
        "messages": [
            {"role": "tool", "name": "pytest", "content": f"FAILED\n{filler}"},
            {"role": "user", "content": "fix it"},
            {"role": "tool", "name": "pytest", "content": f"PASSED\n{filler}"},
        ],
    }
    body = json.dumps(payload).encode()
    result_body = mw._maybe_compress_body(body)

    assert isinstance(result_body, bytes)
    result = json.loads(result_body)
    assert "messages" in result

    original_chars = sum(len(m.get("content", "")) for m in payload["messages"])
    compressed_chars = sum(len(m.get("content", "")) for m in result["messages"])
    assert compressed_chars < original_chars


def test_compression_middleware_passthrough_non_json() -> None:
    """Non-JSON bodies pass through unchanged."""
    from tare.integrations import CompressionMiddleware

    async def dummy_app(scope: dict, receive: object, send: object) -> None:
        pass

    mw = CompressionMiddleware(dummy_app)
    body = b"not json at all"
    assert mw._maybe_compress_body(body) == body


def test_compression_middleware_passthrough_no_messages_key() -> None:
    """JSON bodies without a 'messages' key pass through unchanged."""
    from tare.integrations import CompressionMiddleware

    async def dummy_app(scope: dict, receive: object, send: object) -> None:
        pass

    mw = CompressionMiddleware(dummy_app)
    body = json.dumps({"foo": "bar"}).encode()
    assert mw._maybe_compress_body(body) == body


# ---------------------------------------------------------------------------
# Proxy client helpers raise ImportError cleanly
# ---------------------------------------------------------------------------


def test_anthropic_with_tare_raises_without_anthropic(monkeypatch: pytest.MonkeyPatch) -> None:
    from tare import integrations

    monkeypatch.setitem(sys.modules, "anthropic", None)  # type: ignore[arg-type]
    sys.modules.pop("tare.integrations", None)
    import importlib
    fresh = importlib.import_module("tare.integrations")

    with pytest.raises(ImportError, match="anthropic"):
        fresh.anthropic_with_tare()


def test_openai_with_tare_raises_without_openai(monkeypatch: pytest.MonkeyPatch) -> None:
    from tare import integrations

    monkeypatch.setitem(sys.modules, "openai", None)  # type: ignore[arg-type]
    sys.modules.pop("tare.integrations", None)
    import importlib
    fresh = importlib.import_module("tare.integrations")

    with pytest.raises(ImportError, match="openai"):
        fresh.openai_with_tare()
