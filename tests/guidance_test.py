"""Guidance storage, classification normalization, and prompt composition."""
from __future__ import annotations

from tests.helpers import cleanup_tmp, create_indexed_gap, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-guidance-")
    conn = init_refine(client)
    try:
        from refine_server import gaps, guidance, project_state

        gap = {
            "name": "Improve button contrast",
            "rounds": [{
                "actual": "The save button has low contrast.",
                "target": "The save button should meet WCAG contrast.",
            }],
        }

        selected, raw = guidance.select_for_gap(
            conn, gap,
            run_one_shot=lambda _prompt: (_ for _ in ()).throw(
                AssertionError("classifier should be skipped without guidance"),
            ),
        )
        assert selected == [], selected
        assert raw == "", raw

        saved = project_state.write_guidance([
            {
                "name": "Accessibility",
                "rule": "Accept when the Gap changes visible UI.",
                "instructions": "Preserve keyboard access and color contrast.",
            },
            {
                "name": "Database",
                "rule": "Accept for schema or migration changes.",
                "instructions": "Keep migrations reversible.",
                "enabled": False,
            },
        ])
        assert [item["name"] for item in saved] == ["Accessibility", "Database"]
        assert [item["enabled"] for item in saved] == [True, False]
        assert (client / ".refine" / "guidance.json").exists()
        other = project_state.create_instance("Laptop")
        project_state.set_active_instance(other["id"])
        assert [item["name"] for item in project_state.list_guidance()] == [
            "Accessibility", "Database",
        ]

        captured_prompt = ""

        def fake_classifier(prompt: str) -> str:
            nonlocal captured_prompt
            captured_prompt = prompt
            return '{"decisions":[{"index":0,"decision":"accept"}]}'

        selected, raw = guidance.select_for_gap(conn, gap, run_one_shot=fake_classifier)
        assert raw.startswith("{"), raw
        assert [item["name"] for item in selected] == ["Accessibility"], selected
        assert "Improve button contrast" in captured_prompt
        assert "Accessibility" in captured_prompt
        assert "Database" not in captured_prompt

        prompt = guidance.prepend_to_prompt("Gap instructions", selected)
        assert prompt.startswith("Additional guidance for this Gap:"), prompt
        assert "Preserve keyboard access and color contrast." in prompt
        assert prompt.endswith("Gap instructions"), prompt

        gid = "01GUIDANCELOGAAAAAAAAAAAAAA"
        create_indexed_gap(conn, gid)
        logged_gap = gaps.read_gap_json(gid)
        logged_selected, logged_raw = guidance.select_for_gap(
            conn, logged_gap, run_one_shot=fake_classifier,
        )
        assert [item["name"] for item in logged_selected] == ["Accessibility"]
        assert logged_raw.startswith("{"), logged_raw
        logged_gap = gaps.read_gap_json(gid)
        decision = logged_gap["rounds"][-1].get("guidance_decision")
        assert decision, logged_gap["rounds"][-1]
        assert decision["accepted_names"] == ["Accessibility"], decision
        row = conn.execute(
            "SELECT accepted_json, details_json FROM guidance_decisions "
            "WHERE gap_id = ? AND round_idx = 0",
            (gid,),
        ).fetchone()
        assert row is not None
        assert "Accessibility" in row["accepted_json"], row["accepted_json"]
        assert "classifier_response" in row["details_json"], row["details_json"]
        cached_selected, cached_raw = guidance.select_for_gap(
            conn, logged_gap,
            run_one_shot=lambda _prompt: (_ for _ in ()).throw(
                AssertionError("cached decision should skip classifier"),
            ),
        )
        assert [item["name"] for item in cached_selected] == ["Accessibility"]
        assert cached_raw == "", cached_raw
        conn.execute("DELETE FROM guidance_decisions WHERE gap_id = ?", (gid,))
        project_state.rebuild_sqlite_cache(conn)
        rebuilt = conn.execute(
            "SELECT accepted_json FROM guidance_decisions WHERE gap_id = ?",
            (gid,),
        ).fetchone()
        assert rebuilt is not None

        guidance.log_selection(conn, logged_gap, selected, raw)
        logged_gap = gaps.read_gap_json(gid)
        messages = [log["message"] for log in logged_gap["rounds"][-1]["logs"]]
        assert "Guidance accepted: Accessibility" in messages, messages

        gid_reject = "01GUIDANCEREJECTAAAAAAAAAA"
        create_indexed_gap(conn, gid_reject)
        rejected_gap = gaps.read_gap_json(gid_reject)
        guidance.log_selection(
            conn,
            rejected_gap,
            [],
            '{"decisions":[{"index":0,"decision":"reject"}]}',
        )
        rejected_gap = gaps.read_gap_json(gid_reject)
        messages = [
            log["message"] for log in rejected_gap["rounds"][-1]["logs"]
        ]
        assert "Guidance reviewed; no guidance matched this Gap" in messages, messages

        saved = project_state.write_guidance([{
            "name": "Paused",
            "rule": "Accept for every Gap.",
            "instructions": "This should not be used.",
            "enabled": "false",
        }])
        assert saved[0]["enabled"] is False
        selected, raw = guidance.select_for_gap(
            conn, gap,
            run_one_shot=lambda _prompt: (_ for _ in ()).throw(
                AssertionError("classifier should be skipped without enabled guidance"),
            ),
        )
        assert selected == [], selected
        assert raw == "", raw
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("guidance tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
