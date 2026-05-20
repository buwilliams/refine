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
        assert result["pulled"] is True, result
        assert (client / ".refine" / "sync-marker.txt").read_text(
            encoding="utf-8",
        ) == "from peer\n"

        from refine_server import config, gap_writer, gaps, project_state

        config.get(path=peer / ".refine" / "refine.toml", reload=True)
        peer_instance = project_state.create_instance("Peer Machine")
        peer_gap = "01PROJECTSYNCPEERGAPAAAAA"
        gap_writer.create_gap(
            gap_id=peer_gap,
            name="Peer-created gap",
            initial_round=gaps.new_round("Peer", "Actual", "Target"),
            status="todo",
            priority="high",
            instance_id=peer_instance["id"],
        )
        git(peer, "add", ".refine")
        git(peer, "commit", "-m", "peer instance state update")
        git(peer, "push")

        config.get(path=client / ".refine" / "refine.toml", reload=True)
        os.chdir(client)
        result = project_sync.sync_latest(conn)
        assert result["ok"], result
        assert result["pulled"] is True, result
        assert any(
            inst["id"] == peer_instance["id"]
            for inst in project_state.list_instances()
        )
        assert project_state.active_instance_id() == "default"
        row = conn.execute(
            "SELECT status, priority, reporter, instance_id "
            "FROM gaps_index WHERE id = ?",
            (peer_gap,),
        ).fetchone()
        assert row["status"] == "todo", dict(row)
        assert row["priority"] == "high", dict(row)
        assert row["reporter"] == "Peer", dict(row)
        assert row["instance_id"] == peer_instance["id"], dict(row)

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
        runtime_path = client / ".refine" / "instances" / "default" / "runtime.json"
        runtime_settings = json.loads(runtime_path.read_text(encoding="utf-8"))
        assert runtime_settings["project_update_pulse_interval_seconds"] == "300"

        runtime_settings["branch_name_pattern"] = "pulse/{gap_id}"
        runtime_path.write_text(json.dumps(runtime_settings, indent=2), encoding="utf-8")
        git(client, "add", ".refine/instances/default/runtime.json")
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
