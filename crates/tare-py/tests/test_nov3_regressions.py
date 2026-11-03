import asyncio
import importlib
import json
import sys
import types

import pytest


@pytest.fixture
def integrations():
    return importlib.import_module("tare.integrations")


@pytest.mark.parametrize(
    "prefix",
    [[], [{"type": "http.request", "body": b"partial", "more_body": True}]],
)
def test_disconnect_is_not_rewritten_as_a_request(prefix, integrations):
    disconnect = {"type": "http.disconnect"}
    incoming = iter([*prefix, disconnect])
    observed = []

    async def receive():
        return next(incoming)

    async def send(event):
        pass

    async def app(scope, app_receive, app_send):
        observed.append(await app_receive())

    asyncio.run(integrations.CompressionMiddleware(app)({"type": "http"}, receive, send))
    assert observed == [disconnect]


def test_compressed_body_has_no_stale_content_length(monkeypatch, integrations):
    monkeypatch.setattr(
        integrations, "compress_messages",
        lambda messages, task: [{"role": "user", "content": "x"}],
    )
    body = json.dumps({"messages": [{"role": "user", "content": "long input" * 50}]}).encode()
    scope = {
        "type": "http",
        "headers": [
            (b"content-type", b"application/json"),
            (b"content-length", str(len(body)).encode()),
        ],
    }
    original_headers = list(scope["headers"])
    observed = []

    async def receive():
        return {"type": "http.request", "body": body, "more_body": False}

    async def send(event):
        pass

    async def app(app_scope, app_receive, app_send):
        headers = dict(app_scope["headers"])
        event = await app_receive()
        observed.append((headers.get(b"content-length"), len(event["body"])))

    asyncio.run(integrations.CompressionMiddleware(app)(scope, receive, send))
    declared, actual = observed[0]
    assert declared is None or int(declared) == actual
    assert scope["headers"] == original_headers


@pytest.mark.parametrize("messages", ["invalid", {"role": "user"}, [42]])
def test_unsupported_message_shapes_pass_through_to_application(messages, integrations):
    body = json.dumps({"messages": messages}).encode()
    middleware = integrations.CompressionMiddleware(None)
    assert middleware._maybe_compress_body(body) == body


def test_strands_preserves_all_text_parts_before_compression(monkeypatch, integrations):
    monkeypatch.setitem(sys.modules, "strands", types.ModuleType("strands"))
    received = []

    def compress(messages, task):
        received.extend(messages)
        return messages

    monkeypatch.setattr(integrations, "compress_messages", compress)

    class Model:
        def __call__(self, messages, **kwargs):
            return messages

    wrapper = integrations.strands_model(Model)()
    wrapper([{"role": "user", "content": [{"text": "first"}, {"text": "second"}]}])
    assert received == [{"role": "user", "content": "first\nsecond"}]
