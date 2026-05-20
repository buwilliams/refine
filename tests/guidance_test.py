"""Guidance storage, classification normalization, and prompt composition."""
from __future__ import annotations

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-guidance-")
    conn = init_refine(client)
    try:
        from refine_server import guidance, project_state

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
