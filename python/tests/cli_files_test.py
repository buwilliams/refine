"""CLI parity for target-repo file browser operations."""
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
    tmp, client = make_client_repo("refine-cli-files-")
    conn = init_refine(client)
    conn.close()
    try:
        (client / "src").mkdir()
        (client / "src" / "app.py").write_text(
            "def hello():\n    return 'world'\n",
            encoding="utf-8",
        )
        (client / "node_modules" / "pkg").mkdir(parents=True)
        (client / "node_modules" / "pkg" / "index.js").write_text(
            "module.exports = true;\n",
            encoding="utf-8",
        )
        cfg = str(client / ".refine" / "refine.toml")
        prefix = ["--config", cfg]

        rc, out, err = _run_cli([*prefix, "files", "tree"])
        assert rc == 0, err
        payload = _json(out)
        names = [entry["name"] for entry in payload["entries"]]
        assert "src" in names, names
        assert "node_modules" not in names, names

        rc, out, err = _run_cli([*prefix, "files", "search", "app"])
        assert rc == 0, err
        payload = _json(out)
        assert any(entry["path"] == "src/app.py" for entry in payload["entries"]), payload

        rc, out, err = _run_cli([*prefix, "files", "read", "src/app.py"])
        assert rc == 0, err
        payload = _json(out)
        assert payload["previewable"] is True, payload
        assert payload["content"].startswith("def hello"), payload

        rc, out, err = _run_cli([*prefix, "files", "read", "../outside.txt"])
        assert rc == 1, err
        assert _json(out)["error"]["message"] == "path must stay inside the target repo"
    finally:
        cleanup_tmp(tmp)

    print("CLI files tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
