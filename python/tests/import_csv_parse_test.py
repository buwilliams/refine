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
        "text": (
            "actual,target,reporter,priority\n"
            "Actual only,,Alice,low\n"
            ",Target only,Bob,medium\n"
        ),
    })
    assert status == 200, body
    assert body["count"] == 2, body
    assert body["drafts"][0]["target"] == "", body
    assert body["drafts"][1]["actual"] == "", body

    status, body = api.import_parse_csv({
        "text": (
            "actual,target,reporter,priority\n"
            '"Quoted actual","Target with ""quoted"" words, and comma",Durgesh,low\n'
        ),
    })
    assert status == 200, body
    assert body["drafts"][0]["target"] == 'Target with "quoted" words, and comma', body

    large_csv = "actual,target,reporter,priority,name\n" + "\n".join(
        f"Large actual {i},Large target {i},Reporter,medium,Large gap {i}"
        for i in range(1, 701)
    )
    status, body = api.import_parse_csv({"text": large_csv})
    assert status == 200, body
    assert body["count"] == 700, body
    assert body["drafts"][699]["name"] == "Large gap 700", body
    assert body["drafts"][699]["priority"] == "medium", body

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

    status, body = api.import_parse_csv({
        "text": "actual,target,reporter,priority\n,,R,low\n",
    })
    assert status == 400, body
    assert "actual or target" in body["error"]["message"], body

    print("import csv parse tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
