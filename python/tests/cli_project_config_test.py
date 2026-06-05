"""CLI parity for reporter, guidance, governance, and quality settings."""
from __future__ import annotations

import json
import sys
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def _run_cli(args: list[str]) -> tuple[int, str, str]:
    from refine_cli import cli

    stdout = StringIO()
    stderr = StringIO()
    with redirect_stdout(stdout), redirect_stderr(stderr):
        rc = cli.main(args)
    return rc, stdout.getvalue(), stderr.getvalue()


def _json(out: str) -> dict:
    return json.loads(out)


def main() -> int:
    tmp, client = make_client_repo("refine-cli-config-")
    conn = init_refine(client)
    conn.close()
    cfg = str(client / ".refine" / "refine.toml")
    prefix = ["--config", cfg]
    try:
        rc, out, err = _run_cli([*prefix, "reporter", "add", "CLI Reporter"])
        assert rc == 0, err
        reporter_id = _json(out)["reporter"]["id"]

        rc, out, err = _run_cli([*prefix, "reporter", "list"])
        assert rc == 0, err
        assert any(r["name"] == "CLI Reporter" for r in _json(out)["reporters"])

        guidance = [{
            "name": "CLI note",
            "rule": "Accept CLI parity Gaps.",
            "instructions": "Prefer the CLI path for shared behavior.",
            "enabled": True,
        }]
        rc, out, err = _run_cli([*prefix, "guidance", "replace", json.dumps(guidance)])
        assert rc == 0, err
        assert _json(out)["guidance"][0]["name"] == "CLI note"

        rc, out, err = _run_cli([*prefix, "guidance", "list"])
        assert rc == 0, err
        assert _json(out)["guidance"][0]["rule"] == "Accept CLI parity Gaps."

        rules = [{"text": "Never ship placeholder copy."}]
        rc, out, err = _run_cli([
            *prefix,
            "governance",
            "save",
            "--product",
            "CLI product",
            "--constitution",
            "Keep behavior explicit.",
            "--rules",
            json.dumps(rules),
        ])
        assert rc == 0, err
        governance = _json(out)
        assert governance["configured"] is True
        assert governance["rules"][0]["text"] == "Never ship placeholder copy."

        rc, out, err = _run_cli([*prefix, "governance", "get"])
        assert rc == 0, err
        assert _json(out)["product"] == "CLI product"

        rc, out, err = _run_cli([
            *prefix,
            "quality",
            "save",
            "--enabled",
            "--timing",
            "post_rebuild",
            "--business-requirements",
            "Cover CLI parity.",
            "--instructions",
            "Run focused checks.",
            "--regressions-enabled",
        ])
        assert rc == 0, err
        quality = _json(out)
        assert quality["enabled"] == "1"
        assert quality["timing"] == "post_rebuild"
        assert quality["regressions_enabled"] == "1"

        rc, out, err = _run_cli([
            *prefix,
            "quality",
            "regression",
            "create",
            "--title",
            "CLI regression",
            "--prompt",
            "Check CLI regression plumbing.",
        ])
        assert rc == 0, err
        regression_id = _json(out)["regression"]["id"]

        rc, out, err = _run_cli([
            *prefix,
            "quality",
            "regression",
            "update",
            regression_id,
            "--disabled",
            "--timeout",
            "60",
            "--viewport",
            '{"width":800,"height":600}',
        ])
        assert rc == 0, err
        regression = _json(out)["regression"]
        assert regression["enabled"] is False
        assert regression["timeout_seconds"] == 60
        assert regression["viewport"] == {"width": 800, "height": 600}

        rc, out, err = _run_cli([*prefix, "quality", "regression", "list"])
        assert rc == 0, err
        assert any(r["id"] == regression_id for r in _json(out)["regressions"])

        rc, out, err = _run_cli([*prefix, "quality", "regression", "delete", regression_id])
        assert rc == 0, err
        assert _json(out)["ok"] is True

        rc, out, err = _run_cli([*prefix, "reporter", "delete", str(reporter_id)])
        assert rc == 0, err
        assert _json(out)["ok"] is True
    finally:
        cleanup_tmp(tmp)

    print("CLI project config tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
