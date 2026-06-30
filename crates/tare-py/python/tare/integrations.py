"""tare framework adapters.

All framework imports are **lazy** (inside methods/functions) so this module
loads cleanly even when litellm, langchain, agno, strands, anthropic, or
openai are not installed.

Public API
----------
compress_messages      Core helper — converts OpenAI messages → tare blocks,
                       compresses, returns condensed list[dict].

LiteLLMHandler         litellm CustomLogger-compatible callback class.
CompressionMiddleware  ASGI middleware (Starlette / FastAPI / raw ASGI).
langchain_chat_model   Subclass factory for any LangChain BaseChatModel.
agno_model             Subclass factory for any Agno Model.
strands_model          Subclass factory for any Strands Model.
anthropic_with_tare    Return an anthropic.Anthropic client via the tare proxy.
openai_with_tare       Return an openai.OpenAI client via the tare proxy.
"""

from __future__ import annotations

import json
from typing import Any

# Import from the compiled extension directly so this module is self-contained.
from tare._tare import compress


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------

def _content_to_str(content: Any) -> str:
    """Coerce OpenAI-style content (str | list[part]) to plain text."""
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts: list[str] = []
        for part in content:
            if isinstance(part, dict):
                if part.get("type") == "text":
                    parts.append(part.get("text", ""))
                else:
                    parts.append(json.dumps(part))
            else:
                parts.append(str(part))
        return "\n".join(parts)
    return str(content) if content is not None else ""


def _messages_to_blocks(messages: list[dict]) -> list[dict]:
    """Convert OpenAI-style messages to tare's block format.

    Mapping:
    - system  → kind=system_prompt
    - tool    → kind=tool_output, class=<msg["name"] or "tool">
    - user    → kind=conversation_turn
    - assistant → kind=conversation_turn
    """
    blocks: list[dict] = []
    for msg in messages:
        role = msg.get("role", "user")
        text = _content_to_str(msg.get("content", ""))
        if role == "system":
            blocks.append({"role": "system", "kind": "system_prompt", "text": text})
        elif role == "tool":
            tool_class = msg.get("name") or msg.get("tool_call_id") or "tool"
            blocks.append(
                {"role": "tool", "kind": "tool_output", "class": tool_class, "text": text}
            )
        else:
            tare_role = role if role in ("user", "assistant") else "user"
            blocks.append({"role": tare_role, "kind": "conversation_turn", "text": text})
    return blocks


# ---------------------------------------------------------------------------
# Core primitive
# ---------------------------------------------------------------------------

def compress_messages(messages: list[dict], task: str = "") -> list[dict]:
    """Compress a list of OpenAI-style messages using the tare pipeline.

    Converts each message to a tare block, runs the full compression
    pipeline (structural + query passes), and returns the surviving content
    as a condensed list.  The return is always a ``list[dict]`` so it can be
    passed directly to any OpenAI-compatible API.

    Parameters
    ----------
    messages:
        OpenAI-style messages, e.g. ``[{"role": "user", "content": "..."}]``.
        ``content`` may be a string or a list of content-part dicts.
    task:
        Optional task hint that guides relevance scoring inside tare.

    Returns
    -------
    list[dict]
        Compressed messages list.  Typically a single element containing the
        surviving text, preserving the role of the last message.
    """
    if not messages:
        return messages

    blocks = _messages_to_blocks(messages)
    compressed_text = compress(json.dumps(blocks), task)

    # Preserve the last message's role so API constraints (last msg = user) hold.
    last_role = messages[-1].get("role", "user")
    # Clamp to roles the API accepts in the messages array.
    if last_role not in ("system", "user", "assistant", "tool"):
        last_role = "user"

    return [{"role": last_role, "content": compressed_text}]


# ---------------------------------------------------------------------------
# LiteLLM integration
# ---------------------------------------------------------------------------

class LiteLLMHandler:
    """Duck-typed litellm callback that compresses messages before each call.

    Implements the ``CustomLogger`` protocol via duck-typing so the class does
    not require litellm to be installed at import time.

    Register with litellm::

        import litellm
        litellm.callbacks = [LiteLLMHandler(task="")]

    Parameters
    ----------
    task:
        Optional task hint forwarded to :func:`compress_messages`.
    """

    def __init__(self, task: str = "") -> None:
        self.task = task

    # --- sync hook -----------------------------------------------------------

    def log_pre_api_call(self, model: str, messages: list, kwargs: dict) -> None:
        """Mutate ``messages`` in-place before the synchronous API call."""
        if messages:
            compressed = compress_messages(list(messages), self.task)
            messages.clear()
            messages.extend(compressed)

    # --- async hook ----------------------------------------------------------

    async def async_pre_call_hook(
        self,
        user_api_key_dict: Any = None,
        cache: Any = None,
        data: dict | None = None,
        call_type: str = "",
    ) -> dict | None:
        """Compress ``data['messages']`` before the async API call.

        Requires litellm only at call time; the import is intentionally
        deferred so the class can be instantiated without litellm installed.
        """
        import litellm as _litellm  # noqa: F401 — confirm litellm is available

        if data and "messages" in data:
            data["messages"] = compress_messages(data["messages"], self.task)
        return data


# ---------------------------------------------------------------------------
# ASGI middleware
# ---------------------------------------------------------------------------

class CompressionMiddleware:
    """ASGI middleware that compresses JSON request bodies containing 'messages'.

    Transparently intercepts HTTP requests whose body is a JSON object with a
    ``messages`` key (OpenAI-style chat requests) and replaces it with the
    tare-compressed equivalent before passing to the inner app.

    Compatible with Starlette, FastAPI, and any raw ASGI server::

        from fastapi import FastAPI
        from tare.integrations import CompressionMiddleware

        app = FastAPI()
        app.add_middleware(CompressionMiddleware, task="")

    Parameters
    ----------
    app:
        The inner ASGI application.
    task:
        Optional task hint forwarded to :func:`compress_messages`.
    """

    def __init__(self, app: Any, task: str = "") -> None:
        self.app = app
        self.task = task

    async def __call__(self, scope: dict, receive: Any, send: Any) -> None:
        if scope.get("type") != "http":
            await self.app(scope, receive, send)
            return

        consumed: list[bytes] = []

        async def patched_receive() -> dict:
            if consumed:
                return {
                    "type": "http.request",
                    "body": b"".join(consumed),
                    "more_body": False,
                }
            event = await receive()
            body = event.get("body", b"")
            while event.get("more_body", False):
                event = await receive()
                body += event.get("body", b"")
            body = self._maybe_compress_body(body)
            consumed.append(body)
            return {"type": "http.request", "body": body, "more_body": False}

        await self.app(scope, patched_receive, send)

    def _maybe_compress_body(self, body: bytes) -> bytes:
        """Return compressed body if it contains a 'messages' key; else passthrough."""
        if not body:
            return body
        try:
            payload = json.loads(body)
        except (json.JSONDecodeError, UnicodeDecodeError):
            return body
        if not isinstance(payload, dict) or "messages" not in payload:
            return body
        payload["messages"] = compress_messages(payload["messages"], self.task)
        return json.dumps(payload).encode()


# ---------------------------------------------------------------------------
# LangChain integration
# ---------------------------------------------------------------------------

def langchain_chat_model(base: Any) -> Any:
    """Return a tare-compressing subclass of a LangChain ``BaseChatModel``.

    The returned class adds a ``tare_task`` field and overrides ``_generate``
    to compress the message list before delegating to the parent.

    Requires ``langchain-core``::

        from langchain_openai import ChatOpenAI
        from tare.integrations import langchain_chat_model

        TareChat = langchain_chat_model(ChatOpenAI)
        llm = TareChat(model="gpt-4o", tare_task="summarise logs")

    Parameters
    ----------
    base:
        Any LangChain ``BaseChatModel`` subclass.

    Raises
    ------
    ImportError
        If ``langchain-core`` is not installed.
    """
    try:
        from langchain_core.messages import BaseMessage, HumanMessage  # noqa: F401
    except ImportError as exc:
        raise ImportError(
            "langchain_core is required for langchain_chat_model. "
            "Install with: pip install langchain-core"
        ) from exc

    from langchain_core.messages import HumanMessage

    class TareChatModel(base):  # type: ignore[valid-type,misc]
        tare_task: str = ""

        def _to_compressed(self, messages: list) -> list:
            dicts = [
                {
                    "role": getattr(m, "type", "user"),
                    "content": getattr(m, "content", str(m)),
                }
                for m in messages
            ]
            compressed = compress_messages(dicts, self.tare_task)
            return [HumanMessage(content=c["content"]) for c in compressed]

        def _generate(
            self,
            messages: list,
            stop: list | None = None,
            **kwargs: Any,
        ) -> Any:
            return super()._generate(self._to_compressed(messages), stop=stop, **kwargs)

    TareChatModel.__name__ = f"Tare{base.__name__}"
    TareChatModel.__qualname__ = f"Tare{base.__qualname__}"
    return TareChatModel


# ---------------------------------------------------------------------------
# Agno integration
# ---------------------------------------------------------------------------

def agno_model(base: Any) -> Any:
    """Return a tare-compressing subclass of an Agno ``Model``.

    Overrides ``invoke`` to compress the message list before delegation.

    Requires ``agno``::

        from agno.models.openai import OpenAIChat
        from tare.integrations import agno_model

        TareChat = agno_model(OpenAIChat)
        model = TareChat(id="gpt-4o", tare_task="")

    Parameters
    ----------
    base:
        Any agno ``Model`` subclass.

    Raises
    ------
    ImportError
        If ``agno`` is not installed.
    """
    try:
        import agno  # noqa: F401
    except ImportError as exc:
        raise ImportError(
            "agno is required for agno_model. Install with: pip install agno"
        ) from exc

    class TareAgnoModel(base):  # type: ignore[valid-type,misc]
        tare_task: str = ""

        def invoke(self, messages: list, **kwargs: Any) -> Any:
            dicts = [
                {
                    "role": getattr(m, "role", "user"),
                    "content": getattr(m, "content", str(m)),
                }
                for m in messages
            ]
            compressed = compress_messages(dicts, self.tare_task)
            # Reconstruct using the same message type(s) as the original list.
            msg_type = type(messages[0]) if messages else None
            if msg_type is not None:
                try:
                    rebuilt = [
                        msg_type(role=c["role"], content=c["content"])
                        for c in compressed
                    ]
                except Exception:
                    rebuilt = compressed  # fall back to plain dicts
            else:
                rebuilt = compressed
            return super().invoke(rebuilt, **kwargs)

    TareAgnoModel.__name__ = f"Tare{base.__name__}"
    TareAgnoModel.__qualname__ = f"Tare{base.__qualname__}"
    return TareAgnoModel


# ---------------------------------------------------------------------------
# Strands integration
# ---------------------------------------------------------------------------

def strands_model(base: Any) -> Any:
    """Return a tare-compressing subclass of a Strands ``Model``.

    Strands models are callable: ``model(messages, system_prompt, ...)``.
    The wrapper compresses ``messages`` (Bedrock-style) before delegating.

    Requires ``strands-agents``::

        from strands.models.bedrock import BedrockModel
        from tare.integrations import strands_model

        TareModel = strands_model(BedrockModel)
        model = TareModel(model_id="...", tare_task="")

    Parameters
    ----------
    base:
        Any strands ``Model`` subclass.

    Raises
    ------
    ImportError
        If ``strands-agents`` is not installed.
    """
    try:
        import strands  # noqa: F401
    except ImportError as exc:
        raise ImportError(
            "strands-agents is required for strands_model. "
            "Install with: pip install strands-agents"
        ) from exc

    class TareStrandsModel(base):  # type: ignore[valid-type,misc]
        tare_task: str = ""

        def __call__(self, messages: list, **kwargs: Any) -> Any:
            # Strands uses Bedrock-style: [{"role": ..., "content": [{"text": ...}]}]
            dicts = []
            for m in messages:
                if isinstance(m, dict):
                    content_parts = m.get("content", [])
                    text = (
                        content_parts[0].get("text", "")
                        if content_parts and isinstance(content_parts[0], dict)
                        else str(content_parts)
                    )
                    dicts.append({"role": m.get("role", "user"), "content": text})
                else:
                    dicts.append({"role": "user", "content": str(m)})
            compressed = compress_messages(dicts, self.tare_task)
            bedrock_msgs = [
                {"role": c["role"], "content": [{"text": c["content"]}]}
                for c in compressed
            ]
            return super().__call__(bedrock_msgs, **kwargs)

    TareStrandsModel.__name__ = f"Tare{base.__name__}"
    TareStrandsModel.__qualname__ = f"Tare{base.__qualname__}"
    return TareStrandsModel


# ---------------------------------------------------------------------------
# Proxy client helpers
# ---------------------------------------------------------------------------

def anthropic_with_tare(
    client_kwargs: dict | None = None,
    base_url: str = "http://127.0.0.1:8787",
) -> Any:
    """Return an ``anthropic.Anthropic`` client pointed at the tare proxy.

    The tare proxy (default: ``http://127.0.0.1:8787``) applies server-side
    compression on every request before forwarding to Anthropic.

    Requires ``anthropic``::

        from tare.integrations import anthropic_with_tare

        client = anthropic_with_tare()
        client.messages.create(model="claude-opus-4-5", max_tokens=1024, messages=[...])

    Parameters
    ----------
    client_kwargs:
        Extra keyword arguments passed to ``anthropic.Anthropic()``, e.g.
        ``{"api_key": "sk-..."}`` (the default reads from the environment).
    base_url:
        Base URL of the running tare proxy.

    Raises
    ------
    ImportError
        If ``anthropic`` is not installed.
    """
    try:
        import anthropic
    except ImportError as exc:
        raise ImportError(
            "anthropic is required for anthropic_with_tare. "
            "Install with: pip install anthropic"
        ) from exc

    kwargs = dict(client_kwargs or {})
    kwargs["base_url"] = base_url
    return anthropic.Anthropic(**kwargs)


def openai_with_tare(
    client_kwargs: dict | None = None,
    base_url: str = "http://127.0.0.1:8787",
) -> Any:
    """Return an ``openai.OpenAI`` client pointed at the tare proxy.

    The tare proxy (default: ``http://127.0.0.1:8787``) applies server-side
    compression on every request before forwarding to OpenAI.

    Requires ``openai``::

        from tare.integrations import openai_with_tare

        client = openai_with_tare()
        client.chat.completions.create(model="gpt-4o", messages=[...])

    Parameters
    ----------
    client_kwargs:
        Extra keyword arguments passed to ``openai.OpenAI()``, e.g.
        ``{"api_key": "sk-..."}`` (the default reads from the environment).
    base_url:
        Base URL of the running tare proxy.

    Raises
    ------
    ImportError
        If ``openai`` is not installed.
    """
    try:
        import openai
    except ImportError as exc:
        raise ImportError(
            "openai is required for openai_with_tare. "
            "Install with: pip install openai"
        ) from exc

    kwargs = dict(client_kwargs or {})
    kwargs["base_url"] = base_url
    return openai.OpenAI(**kwargs)
