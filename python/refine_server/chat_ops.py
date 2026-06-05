"""Shared chat operations."""
from __future__ import annotations

from collections.abc import Callable
from typing import Any

from .backend_protocol import M_CHAT_INPUT, M_CHAT_READ, M_CHAT_START, M_CHAT_STOP


RunnerCall = Callable[[str, dict[str, object], float], dict[str, Any]]


def start(runner_call: RunnerCall, body: dict[str, Any]) -> tuple[int, dict[str, Any]]:
    result = runner_call(
        M_CHAT_START,
        {
            "gap_id": body.get("gap_id"),
            "purpose": body.get("purpose"),
        },
        30.0,
    )
    return 201, result


def input(runner_call: RunnerCall, session_id: str, body: dict[str, Any]) -> tuple[int, dict[str, Any]]:
    raw_text = body.get("text", "")
    if raw_text is None:
        raw_text = ""
    if not isinstance(raw_text, str):
        raise ValueError("chat input text must be a string")
    result = runner_call(
        M_CHAT_INPUT,
        {
            "session_id": session_id,
            "text": raw_text,
        },
        30.0,
    )
    return 200, result


def read(runner_call: RunnerCall, session_id: str) -> tuple[int, dict[str, Any]]:
    result = runner_call(M_CHAT_READ, {"session_id": session_id}, 30.0)
    return 200, result


def stop(runner_call: RunnerCall, session_id: str) -> tuple[int, dict[str, Any]]:
    result = runner_call(M_CHAT_STOP, {"session_id": session_id}, 30.0)
    return 200, result
