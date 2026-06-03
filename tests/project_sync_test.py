"""Project sync pulls remote .refine state and rebuilds local projections."""
from __future__ import annotations

import json
import os

from tests.helpers import cleanup_tmp, git, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-project-sync-", with_remote=True)
    conn = init_refine(client)
    try:
        from refine_server import project_sync

        git(client, "add", ".refine")
        git(client, "commit", "-m", "init refine state")
        git(client, "push")

        runtime_noise = [
            ".refine/app.pid",
            ".refine/app.log",
            ".refine/gaps/01/PROJECTSYNCLOGNOISE/logs.jsonl",
        ]
        for rel in runtime_noise:
            p = client / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text("tracked runtime noise\n", encoding="utf-8")
        git(client, "add", "-f", *runtime_noise)
        git(client, "commit", "-m", "track runtime noise")
        git(client, "push")
        for rel in runtime_noise:
            (client / rel).write_text("changed runtime noise\n", encoding="utf-8")
        result = project_sync.sync_latest(conn)
        assert result["ok"], result
        assert result["stage"] == "synced", result
        assert result["committed_state"] is True, result
        assert result["pushed_state"] is True, result
        assert git(client, "log", "-1", "--format=%s").stdout.strip() == (
            "refine: stop tracking runtime state"
        )
        assert git(client, "ls-files", *runtime_noise).stdout.strip() == ""
        git(client, "push")

        git(tmp, "clone", str(tmp / "origin.git"), "peer")
        peer = tmp / "peer"
        git(peer, "config", "user.email", "t@x")
        git(peer, "config", "user.name", "t")
        marker = peer / ".refine" / "sync-marker.txt"
        marker.write_text("from peer\n", encoding="utf-8")
        git(peer, "add", ".refine/sync-marker.txt")
        git(peer, "commit", "-m", "peer refine state update")
        git(peer, "push")

        os.chdir(client)
        result = project_sync.sync_latest(conn)
        assert result["ok"], result
        assert result["stage"] == "synced", result
        assert result["pulled"] is True, result
        assert (client / ".refine" / "sync-marker.txt").read_text(
            encoding="utf-8",
        ) == "from peer\n"

        peer_marker2 = peer / ".refine" / "sync-peer-before-push.txt"
        peer_marker2.write_text("peer before push\n", encoding="utf-8")
        git(peer, "add", ".refine/sync-peer-before-push.txt")
        git(peer, "commit", "-m", "peer update before local state push")
        git(peer, "push")
        local_marker = client / ".refine" / "sync-local-before-push.txt"
        local_marker.write_text("local before push\n", encoding="utf-8")
        result = project_sync.sync_latest(conn)
        assert result["ok"], result
        assert result["stage"] == "synced", result
        assert result["committed_state"] is True, result
        assert result["pushed_state"] is True, result
        assert peer_marker2.relative_to(peer).as_posix() in git(
            client, "ls-tree", "-r", "--name-only", "HEAD",
        ).stdout
        assert ".refine/sync-local-before-push.txt" in git(
            client, "ls-tree", "-r", "--name-only", "origin/main",
        ).stdout
        git(peer, "pull", "--ff-only")

        local_clean_marker = client / ".refine" / "sync-local-clean-ahead.txt"
        local_clean_marker.write_text("local clean ahead\n", encoding="utf-8")
        git(client, "add", ".refine/sync-local-clean-ahead.txt")
        git(client, "commit", "-m", "local clean state commit")
        peer_marker3 = peer / ".refine" / "sync-peer-after-local-commit.txt"
        peer_marker3.write_text("peer after local commit\n", encoding="utf-8")
        git(peer, "add", ".refine/sync-peer-after-local-commit.txt")
        git(peer, "commit", "-m", "peer update after local clean commit")
        git(peer, "push")
        result = project_sync.sync_latest(conn)
        assert result["ok"], result
        assert result["stage"] == "synced", result
        assert result["pulled"] is True, result
        assert result["pushed_state"] is True, result
        origin_tree = git(client, "ls-tree", "-r", "--name-only", "origin/main").stdout
        assert ".refine/sync-local-clean-ahead.txt" in origin_tree
        assert ".refine/sync-peer-after-local-commit.txt" in origin_tree
        git(peer, "pull", "--ff-only")

        from refine_server import config, gap_writer, gaps, project_state

        config.get(path=peer / ".refine" / "refine.toml", reload=True)
        peer_instance = project_state.create_node("Peer Machine")
        peer_gap = "01PROJECTSYNCPEERGAPAAAAA"
        gap_writer.create_gap(
            gap_id=peer_gap,
            name="Peer-created gap",
            initial_round=gaps.new_round("Peer", "Actual", "Target"),
            status="todo",
            priority="high",
            node_id=peer_instance["id"],
        )
        git(peer, "add", ".refine")
        git(peer, "commit", "-m", "peer node state update")
        git(peer, "push")

        config.get(path=client / ".refine" / "refine.toml", reload=True)
        os.chdir(client)
        result = project_sync.sync_latest(conn)
        assert result["ok"], result
        assert result["stage"] == "synced", result
        assert result["pulled"] is True, result
        assert any(
            inst["id"] == peer_instance["id"]
            for inst in project_state.list_nodes()
        )
        assert project_state.active_node_id() == "default"
        row = conn.execute(
            "SELECT status, priority, reporter, node_id "
            "FROM gaps_index WHERE id = ?",
            (peer_gap,),
        ).fetchone()
        assert row["status"] == "todo", dict(row)
        assert row["priority"] == "high", dict(row)
        assert row["reporter"] == "Peer", dict(row)
        assert row["node_id"] == peer_instance["id"], dict(row)

        marker2 = peer / ".refine" / "pulse-marker.txt"
        marker2.write_text("from pulse\n", encoding="utf-8")
        git(peer, "add", ".refine/pulse-marker.txt")
        git(peer, "commit", "-m", "peer pulse state update")
        git(peer, "push")

        result = project_sync.pulse(conn)
        assert result["ok"], result
        assert result["changed"] is True, result
        assert result["pulled"] is True, result
        assert (client / ".refine" / "pulse-marker.txt").read_text(
            encoding="utf-8",
        ) == "from pulse\n"

        from refine_server import db

        db.set_setting(conn, "project_update_pulse_interval_seconds", "300")
        runtime_path = client / ".refine" / "nodes" / "default" / "runtime.json"
        runtime_settings = json.loads(runtime_path.read_text(encoding="utf-8"))
        assert runtime_settings["project_update_pulse_interval_seconds"] == "300"

        runtime_settings["branch_name_pattern"] = "pulse/{gap_id}"
        runtime_path.write_text(json.dumps(runtime_settings, indent=2), encoding="utf-8")
        git(client, "add", ".refine/nodes/default/runtime.json")
        git(client, "commit", "-m", "local runtime update")
        assert db.get_setting(conn, "branch_name_pattern") != "pulse/{gap_id}"

        result = project_sync.pulse(conn)
        assert result["ok"], result
        assert result["changed"] is True, result
        assert result["stage"] == "refreshed", result
        assert db.get_setting(conn, "branch_name_pattern") == "pulse/{gap_id}"
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("project sync tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
