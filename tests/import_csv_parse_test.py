"""Focused tests for backend CSV import parsing."""
from __future__ import annotations


def main() -> int:
    from refine_ui import api

    status, body = api.import_parse_csv({
        "text": (
            "actual;target;reporter;priority;name\n"
            '"Current, quoted";"Target\nwrapped";Alice;HIGH;"Quoted gap"\n'
            "Second current;Second target;Bob;low;\n"
        ),
    })
    assert status == 200, body
    assert body["count"] == 2, body
    first = body["drafts"][0]
    assert first == {
        "name": "Quoted gap",
        "actual": "Current, quoted",
        "target": "Target\nwrapped",
        "reporter": "Alice",
        "priority": "high",
    }, first
    assert body["drafts"][1]["name"] == "", body
    assert body["drafts"][1]["priority"] == "low", body

    status, body = api.import_parse_csv({
        "text": "actual,target,reporter\nA,T,R\n",
    })
    assert status == 400, body
    assert "missing required field" in body["error"]["message"], body

    status, body = api.import_parse_csv({
        "text": "actual,target,reporter,priority\nA,T,R,urgent\n",
    })
    assert status == 400, body
    assert "priority must be low, medium, or high" in body["error"]["message"], body

    print("import csv parse tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
