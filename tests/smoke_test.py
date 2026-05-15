"""End-to-end smoke test (no real agent CLI / git remote required).

Validates:
- `refine init`-equivalent config bootstrap
- DB init + schema is creatable
- ULID generation
- Reporter add/list
- Gap creation through gap_writer + SQLite index
- Round append + log append
- Friendly classification of various outcomes
- Activity feed read/write
- Runner instantiation against the configured paths
"""
from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def main() -> int:
    tmp = Path(tempfile.mkdtemp(prefix="refine-smoke-"))
    print(f"using tmp dir: {tmp}")
    # Make a fake "client repo" and `refine init` a volume root inside it.
    client = tmp / "client"
    client.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=client, check=True)
    subprocess.run(["git", "-c", "user.email=t@x", "-c", "user.name=t",
                    "commit", "--allow-empty", "-m", "init"], cwd=client, check=True)

    # Drop into the client repo so config discovery works.
    os.chdir(client)

    # Re-import in this clean environment.
    for mod in list(sys.modules):
        if mod.startswith("refine"):
            del sys.modules[mod]

    from refine_shared import config

    # Equivalent of `refine init`
    cfg_path = config.write_defaults(client / ".refine")
    print(f"wrote config: {cfg_path}")
    cfg = config.get()
    print(f"volume root:  {cfg.volume_root}")
    print(f"client repo:  {cfg.client_repo}")

    from refine_shared import db, reporters, activity, gaps as shared_gaps
    from refine_shared.ulid import new_ulid, is_ulid
    from refine_shared.friendly import classify_subprocess_failure, classify_git_failure
    from refine_runner import agent_cli, gap_writer

    # --- DB ------------------------------------------------------------------
    db.init_db()
    conn = db.connect()
    settings = db.list_settings(conn)
    assert settings["parallel_run_cap"] == "3"
    assert settings["agent_idle_timeout_seconds"] == "900"
    assert settings["agent_hard_cap_seconds"] == "86400"
    print("[ok] DB init + defaults seeded")

    # --- Agent CLI abstraction ---------------------------------------------
    codex = agent_cli.get_spec("codex")
    cargs = codex.agent_args("/bin/codex", "do it", cwd=client)
    assert cargs[:2] == ["/bin/codex", "exec"]
    assert "--dangerously-bypass-approvals-and-sandbox" in cargs
    assert "--ask-for-approval" not in cargs
    assert "--sandbox" not in cargs
    assert "--json" in cargs and "-C" in cargs
    assert "--full-auto" not in cargs
    assert codex.chat_args("/bin/codex", "hi", session_id="abc")[:3] == [
        "/bin/codex", "exec", "resume",
    ]
    from refine_runner.llm import _extract_final_text
    from refine_runner.subprocess_mgr import _summarize_codex_event
    codex_jsonl = (
        '{"type":"item.completed","item":{"type":"agent_message",'
        '"text":"[{\\\"name\\\":\\\"N\\\",\\\"actual\\\":\\\"A\\\",'
        '\\"target\\\":\\\"T\\\"}]"}}\n'
    )
    assert _extract_final_text(codex_jsonl).startswith("[")
    assert _summarize_codex_event({
        "type": "item.completed",
        "item": {"type": "agent_message", "text": "done"},
    }) == ["done"]
    from refine_runner.chat_mgr import _chat_env
    old_openai_key = os.environ.get("OPENAI_API_KEY")
    old_openai_base = os.environ.get("OPENAI_BASE_URL")
    old_codex_ci = os.environ.get("CODEX_CI")
    old_codex_thread = os.environ.get("CODEX_THREAD_ID")
    try:
        os.environ["OPENAI_API_KEY"] = "sk-test-should-not-leak"
        os.environ["OPENAI_BASE_URL"] = "https://example.invalid/v1"
        os.environ["CODEX_CI"] = "1"
        os.environ["CODEX_THREAD_ID"] = "test-thread"
        chat_env = _chat_env()
        assert "OPENAI_API_KEY" not in chat_env
        assert "OPENAI_BASE_URL" not in chat_env
        assert "CODEX_CI" not in chat_env
        assert "CODEX_THREAD_ID" not in chat_env
    finally:
        if old_openai_key is None:
            os.environ.pop("OPENAI_API_KEY", None)
        else:
            os.environ["OPENAI_API_KEY"] = old_openai_key
        if old_openai_base is None:
            os.environ.pop("OPENAI_BASE_URL", None)
        else:
            os.environ["OPENAI_BASE_URL"] = old_openai_base
        if old_codex_ci is None:
            os.environ.pop("CODEX_CI", None)
        else:
            os.environ["CODEX_CI"] = old_codex_ci
        if old_codex_thread is None:
            os.environ.pop("CODEX_THREAD_ID", None)
        else:
            os.environ["CODEX_THREAD_ID"] = old_codex_thread
    print("[ok] codex CLI args + JSONL parsing")

    # --- Target-app command/check runtime -----------------------------------
    from refine_runner import target_app
    tcfg = {
        "start_command": "true",
        "stop_command": "true",
        "status_command": "true",
        "cwd": "",
        "env": {},
        "start_timeout_seconds": 5,
        "stop_timeout_seconds": 5,
        "status_timeout_seconds": 5,
    }
    tres = target_app.run_operation("start", tcfg)
    assert tres["ok"] and tres["state"] == "running", tres
    sres = target_app.run_operation("status", tcfg)
    assert sres["ok"] and sres["state"] == "running", sres
    stop_cfg = {**tcfg, "status_command": "false"}
    xres = target_app.run_operation("stop", stop_cfg)
    assert xres["ok"] and xres["state"] == "stopped", xres
    gen = target_app.normalize_generated_config({
        "start_command": "npm run dev\n",
        "stop_command": "pkill -f dev || true",
        "status_command": "pgrep -f dev",
        "env": {"PORT": 3000},
    })
    assert gen["start_command"] == "npm run dev"
    assert gen["env"]["PORT"] == "3000"
    print("[ok] target-app command runtime + config normalization")

    # --- Reporters -----------------------------------------------------------
    jane = reporters.add(conn, "Jane Doe")
    again = reporters.add(conn, "Jane Doe")  # idempotent
    assert jane["name"] == "Jane Doe"
    assert again["id"] == jane["id"]
    print("[ok] reporters: add + dedupe")

    # --- ULID ----------------------------------------------------------------
    uid = new_ulid()
    assert is_ulid(uid) and len(uid) == 26
    print(f"[ok] ulid: {uid}")

    # --- Gap creation + JSON write ------------------------------------------
    round_obj = shared_gaps.new_round(
        reporter="Jane Doe",
        actual="Button is red.",
        target="Button should be blue.",
    )
    gap = gap_writer.create_gap(gap_id=uid, name="Recolor button", initial_round=round_obj)
    assert gap["id"] == uid
    assert len(gap["rounds"]) == 1
    assert gap["rounds"][0]["reporter"] == "Jane Doe"

    from refine_shared.paths import relative_gap_path
    with db.transaction(conn):
        conn.execute(
            "INSERT INTO gaps_index (id, name, status, created, updated, json_path) "
            "VALUES (?, ?, 'todo', ?, ?, ?)",
            (uid, "Recolor button", gap["created"], gap["updated"],
             relative_gap_path(uid)),
        )

    g2 = shared_gaps.read_gap_json(uid)
    assert g2 is not None and g2["name"] == "Recolor button"
    print("[ok] gap created + persisted")

    # --- Append round + edit + log ------------------------------------------
    r2 = shared_gaps.new_round(reporter="Jane Doe", actual="It's purple now.",
                               target="Make it actually blue.")
    gap_writer.append_round(uid, r2)
    g3 = shared_gaps.read_gap_json(uid)
    assert len(g3["rounds"]) == 2

    gap_writer.edit_latest_round(uid, actual="It's purple, not blue.")
    g4 = shared_gaps.read_gap_json(uid)
    assert g4["rounds"][-1]["actual"] == "It's purple, not blue."

    gap_writer.append_round_log(
        gap_id=uid, round_idx=1,
        message="agent started",
        severity="info", category="cli",
    )
    g5 = shared_gaps.read_gap_json(uid)
    assert len(g5["rounds"][1]["logs"]) == 1
    print("[ok] round append + edit + log append")

    # --- Friendly summaries --------------------------------------------------
    idle = classify_subprocess_failure(killed_reason="idle")
    assert "stuck" in idle.message.lower() or "no output" in idle.message.lower()
    auth = classify_subprocess_failure(stderr="invalid api key 401")
    assert auth.category == "auth"
    nopush = classify_git_failure("non-fast-forward; updates were rejected", op="push")
    assert nopush.message.startswith("Push rejected")
    print("[ok] friendly summaries classify correctly")

    # --- Activity feed -------------------------------------------------------
    activity.append(conn, message="Gap created: Recolor button",
                    severity="info", category="state", gap_id=uid, actor="Jane Doe")
    activity.append(conn, message="Agent run started",
                    severity="info", category="cli", gap_id=uid, actor="runner")
    feed = activity.recent(conn, limit=10)
    assert len(feed) >= 2
    print(f"[ok] activity feed has {len(feed)} entries")

    # --- Friendly outcome ----------------------------------------------------
    from refine_runner.friendly_outcome import classify_outcome
    assert classify_outcome(exit_code=0, killed_reason=None, no_new_commits=False).kind == "success"
    assert classify_outcome(exit_code=0, killed_reason=None, no_new_commits=True).kind == "failure"
    assert classify_outcome(exit_code=0, killed_reason="idle", no_new_commits=False).kind == "failure"
    assert classify_outcome(exit_code=0, killed_reason="hard_cap", no_new_commits=False).kind == "failure"
    assert classify_outcome(exit_code=2, killed_reason=None, no_new_commits=False).kind == "failure"
    # No commits but the agent's `result` event reported success
    # ("target already met") — trust the agent over the no-commits heuristic.
    target_met = classify_outcome(exit_code=0, killed_reason=None,
                                   no_new_commits=True,
                                   agent_reported_success=True)
    assert target_met.kind == "success", target_met
    assert "target was already met" in target_met.message, target_met
    # Same applies on the `result_grace` exit path.
    assert classify_outcome(exit_code=0, killed_reason="result_grace",
                            no_new_commits=True,
                            agent_reported_success=True).kind == "success"
    # Without an explicit success signal, the no-commits heuristic still
    # demotes to `failed` (preserves prior behavior).
    assert classify_outcome(exit_code=0, killed_reason=None,
                            no_new_commits=True,
                            agent_reported_success=None).kind == "failure"
    print("[ok] subprocess outcome classification")

    # --- Pre-flight ---------------------------------------------------------
    from refine_runner import preflight
    ok, msg = preflight.check(conn)
    print(f"[ok] preflight ran (ok={ok}, msg={'set' if msg else 'none'})")

    # --- Runner instantiation -----------------------------------------------
    from refine_runner.runner import Runner
    r = Runner()
    assert r.sub_mgr is not None
    assert r.dispatcher is not None
    print("[ok] runner wires up")

    # cleanup
    conn.close()
    os.chdir(tempfile.gettempdir())  # release the cwd before rmtree
    shutil.rmtree(tmp, ignore_errors=True)
    print("\nALL OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
