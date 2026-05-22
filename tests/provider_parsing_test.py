"""Focused tests for provider CLI argument and output parsing.

These cover the provider abstraction paths that are hard to validate through
the broad smoke test without launching real agent CLIs.
"""
from __future__ import annotations

import io
import json
import sys
import time
from pathlib import Path


def main() -> int:
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

    from refine_server import agent_cli, llm, preflight, target_app
    from refine_server.chat_mgr import ChatManager, ChatSession
    from refine_server.subprocess_mgr import _summarize_codex_event

    # --- Agent CLI abstraction ---------------------------------------------
    assert agent_cli.get_spec(None).name == "claude"
    assert agent_cli.get_spec("unknown").name == "claude"
    assert [s.name for s in agent_cli.all_specs()] == ["claude", "codex", "gemini"]

    codex = agent_cli.get_spec("codex")
    cwd = Path("/tmp/refine-client")
    args = codex.agent_args("/bin/codex", "fix it", cwd=cwd)
    assert args[:2] == ["/bin/codex", "exec"]
    assert "--dangerously-bypass-approvals-and-sandbox" in args
    assert "--color" in args and "never" in args
    assert "--json" in args
    assert args[args.index("-C") + 1] == str(cwd)
    assert args[-1] == "fix it"
    assert "--full-auto" not in args

    fresh_chat = codex.chat_args("/bin/codex", "hello", cwd=cwd)
    assert fresh_chat[:2] == ["/bin/codex", "exec"]
    assert "-C" in fresh_chat and fresh_chat[fresh_chat.index("-C") + 1] == str(cwd)
    assert fresh_chat[-1] == "hello"

    resumed_chat = codex.chat_args(
        "/bin/codex", "continue", session_id="thread-123", cwd=cwd,
    )
    assert resumed_chat[:3] == ["/bin/codex", "exec", "resume"]
    assert resumed_chat[-2:] == ["thread-123", "continue"]
    assert "-C" not in resumed_chat
    assert "--color" not in resumed_chat

    one_shot = codex.one_shot_args(
        "/bin/codex", "extract",
        cwd=cwd,
        output_last_message=Path("/tmp/last.txt"),
        output_schema=Path("/tmp/schema.json"),
        json_output=True,
    )
    assert one_shot[:2] == ["/bin/codex", "exec"]
    assert "--json" in one_shot
    assert one_shot[one_shot.index("--output-last-message") + 1] == "/tmp/last.txt"
    assert one_shot[one_shot.index("--output-schema") + 1] == "/tmp/schema.json"
    assert one_shot[-1] == "extract"

    auth_check = codex.auth_check_args(
        "/bin/codex", "Say exactly hello",
        cwd=cwd,
        output_last_message=Path("/tmp/auth.txt"),
    )
    assert auth_check[:2] == ["/bin/codex", "exec"]
    assert "--json" in auth_check
    assert auth_check[auth_check.index("--output-last-message") + 1] == "/tmp/auth.txt"
    assert auth_check[-1] == "Say exactly hello"

    assert preflight._is_hello_response("hello\n")
    assert preflight._is_hello_response('"hello"')
    assert not preflight._is_hello_response("hello world")
    assert preflight._extract_response_text(json.dumps({
        "type": "item.completed",
        "item": {"type": "agent_message", "text": "hello"},
    })) == "hello"

    # --- Codex JSONL summarization -----------------------------------------
    assert _summarize_codex_event({
        "type": "session.started",
        "session_id": "abcdef123456",
    }) == ["[session] init \u2014 provider=codex session=abcdef12"]
    assert _summarize_codex_event({
        "type": "item.completed",
        "item": {"type": "agent_message", "text": "line one\nline two"},
    }) == ["line one", "line two"]
    assert _summarize_codex_event({
        "type": "item.completed",
        "item": {"type": "local_shell_call", "command": "pytest\nsecond"},
    }) == ["[tool] pytest"]
    assert _summarize_codex_event({
        "type": "item.completed",
        "item": {
            "type": "local_shell_call_output",
            "output": [{"type": "text", "text": "\n tests passed\n"}],
        },
    }) == ["[tool result] tests passed"]
    assert _summarize_codex_event({
        "type": "error",
        "error": {"message": "bad flag"},
    }) == ["[error] {'message': 'bad flag'}"]
    assert _summarize_codex_event({"type": "turn.completed"}) == []
    assert _summarize_codex_event("not a dict") == []

    # --- Import extraction JSON parsing ------------------------------------
    pasted = (
        "notes before\n"
        "[{\"name\":\"Export CSV\",\"actual\":\"No export button exists.\","
        "\"target\":\"Users can export the dashboard as CSV.\"}]\n"
        "notes after"
    )
    drafts = llm._normalize_drafts(llm._parse_json_array(pasted))
    assert drafts == [{
        "name": "Export CSV",
        "actual": "No export button exists.",
        "target": "Users can export the dashboard as CSV.",
        "preview": "Users can export the dashboard as CSV.",
    }]
    assert llm._parse_json_array("prefix [not-json] [{\"actual\":\"A\"}]") == [
        {"actual": "A"},
    ]
    normalized = llm._normalize_drafts([
        "ignore me",
        {"name": "  ", "actual": "", "target": ""},
        {"name": "Feature", "actual": "", "target": "Add keyboard shortcuts."},
    ])
    assert normalized == [{
        "name": "Feature",
        "actual": "",
        "target": "Add keyboard shortcuts.",
        "preview": "Add keyboard shortcuts.",
    }]
    jsonl = "\n".join([
        "this is not json",
        json.dumps({
            "type": "item.completed",
            "item": {"type": "tool_result", "content": "ignored"},
        }),
        json.dumps({
            "type": "item.completed",
            "item": {
                "type": "agent_message",
                "text": "[{\"name\":\"N\",\"actual\":\"A\",\"target\":\"T\"}]",
            },
        }),
    ])
    assert llm._extract_final_text(jsonl) == (
        "[{\"name\":\"N\",\"actual\":\"A\",\"target\":\"T\"}]"
    )
    raw_array = "[{\"name\":\"N\",\"actual\":\"A\",\"target\":\"T\"}]"
    assert llm._extract_final_text(raw_array) == raw_array
    assert llm._extract_final_text("plain response") == "plain response"

    # --- Target-app generated-config parsing -------------------------------
    fenced = "```json\n{\"start_command\":\"npm run dev\"}\n```"
    assert target_app._parse_json_object(fenced) == {
        "start_command": "npm run dev",
    }
    assert target_app._parse_json_object("prefix {\"cwd\":\"web\"} suffix") == {
        "cwd": "web",
    }
    assert target_app._parse_json_object("[{\"not\":\"object\"}]") is None
    target_jsonl = json.dumps({
        "type": "item.completed",
        "item": {
            "type": "agent_message",
            "text": "{\"status_command\":\"curl -fsS http://localhost:3000\"}",
        },
    })
    assert target_app._last_agent_text(target_jsonl) == (
        "{\"status_command\":\"curl -fsS http://localhost:3000\"}"
    )
    normalized_cfg = target_app.normalize_generated_config({
        "start_command": "npm run dev\n-- --host 0.0.0.0",
        "rebuild_command": "npm run build\n-- --mode production",
        "stop_command": "",
        "rebuild_timeout_seconds": "bad",
        "status_timeout_seconds": "bad",
        "tcp_check_port": 3000,
        "env": {"PORT": 3000},
    })
    assert normalized_cfg["start_command"] == "npm run dev -- --host 0.0.0.0"
    assert normalized_cfg["rebuild_command"] == "npm run build -- --mode production"
    assert normalized_cfg["rebuild_timeout_seconds"] == 300
    assert normalized_cfg["status_timeout_seconds"] == 10
    assert normalized_cfg["tcp_check_port"] == "3000"
    assert normalized_cfg["env"] == {"PORT": "3000"}

    # --- Chat stream parsing ------------------------------------------------
    manager = ChatManager(get_standalone_idle_timeout=lambda: 0)
    snapshot_manager = ChatManager(get_standalone_idle_timeout=lambda: 999)
    sid = snapshot_manager.start(Path.cwd(), is_standalone=True, provider="codex")
    snapshot = snapshot_manager.snapshot()
    assert len(snapshot) == 1, snapshot
    assert snapshot[0]["session_id"] == sid, snapshot
    assert snapshot[0]["status"] == "idle", snapshot
    assert snapshot[0]["pid"] is None, snapshot
    assert snapshot[0]["mode"] == "standalone", snapshot
    snapshot_manager.stop_all(reason="test complete")
    session = ChatSession(
        session_id="chat-test",
        cwd=Path.cwd(),
        is_standalone=True,
        provider="codex",
        last_activity_ts=time.monotonic(),
    )
    try:
        decoder = json.JSONDecoder()
        tail = manager._drain_json_objects(
            session,
            decoder,
            (
                json.dumps({"thread_id": "thread-xyz"})
                + "\n"
                + json.dumps({
                    "type": "item.completed",
                    "item": {"type": "agent_message", "text": "hello\nworld"},
                })
                + "\nplain diagnostic\n"
                + json.dumps({
                    "type": "error",
                    "item": {"type": "error", "error": "bad request"},
                })
            ),
            suppress_assistant=False,
        )
        assert tail == ""
        assert session.provider_session_id == "thread-xyz"
        with session.out_lock:
            assert list(session.out_lines) == [
                "hello", "world", "plain diagnostic", "[refine] bad request",
            ]

        hidden = ChatSession(
            session_id="chat-hidden",
            cwd=Path.cwd(),
            is_standalone=True,
            provider="codex",
            last_activity_ts=time.monotonic(),
        )
        manager._drain_json_objects(
            hidden,
            decoder,
            json.dumps({
                "type": "item.completed",
                "item": {"type": "agent_message", "text": "do not show"},
            }),
            suppress_assistant=True,
        )
        with hidden.out_lock:
            assert list(hidden.out_lines) == []

        class DoneProc:
            pid = 123456

            def wait(self, timeout: float | None = None) -> int:  # noqa: ARG002
                return 0

        hidden.watchdog_armed_pids.add(DoneProc.pid)
        manager._result_watchdog(hidden, DoneProc())  # noqa: SLF001
        assert hidden.watchdog_armed_pids == set()

        class StreamProc:
            pid = 123457
            stdout = io.StringIO(json.dumps({"thread_id": "thread-stream"}))

            def poll(self) -> int:
                return 0

        hidden.proc = StreamProc()  # type: ignore[assignment]
        manager._pump_output(  # noqa: SLF001
            hidden, hidden.proc, suppress_assistant=True,
        )
        assert hidden.proc is None
    finally:
        manager.shutdown()

    print("provider parsing tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
