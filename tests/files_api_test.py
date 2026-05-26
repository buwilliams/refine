"""Read-only target-repo file browser API tests."""
from __future__ import annotations

import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-files-api-")
    conn = init_refine(client)
    try:
        from refine_ui import api

        (client / "src").mkdir()
        (client / "src" / "app.py").write_text(
            "def hello():\n    return 'world'\n",
            encoding="utf-8",
        )
        (client / ".hidden").write_text("visible\n", encoding="utf-8")
        (client / "blob.bin").write_bytes(b"abc\x00def")
        (client / "large.txt").write_bytes(b"x" * (api.FILE_PREVIEW_MAX_BYTES + 1))

        status, body = api.files_tree("")
        assert status == 200, body
        names = [entry["name"] for entry in body["entries"]]
        assert "src" in names, names
        assert ".hidden" in names, names

        status, body = api.files_tree("src")
        assert status == 200, body
        assert body["path"] == "src", body
        assert body["entries"][0]["path"] == "src/app.py", body

        status, body = api.files_read("src/app.py")
        assert status == 200, body
        assert body["previewable"] is True, body
        assert body["content"].startswith("def hello"), body

        status, body = api.files_read("blob.bin")
        assert status == 200, body
        assert body["previewable"] is False, body
        assert "Binary" in body["reason"], body

        status, body = api.files_read("large.txt")
        assert status == 200, body
        assert body["previewable"] is False, body
        assert "larger" in body["reason"], body

        status, body = api.files_read("../outside.txt")
        assert status == 403, body

        outside = tmp / "outside.txt"
        outside.write_text("outside\n", encoding="utf-8")
        try:
            os.symlink(outside, client / "outside-link")
        except (AttributeError, OSError):
            pass
        else:
            status, body = api.files_read("outside-link")
            assert status == 403, body
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("files API tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
