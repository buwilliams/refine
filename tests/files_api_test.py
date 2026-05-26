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
        (client / "src" / "helpers.py").write_text(
            "def helper():\n    return 'ok'\n",
            encoding="utf-8",
        )
        (client / "features" / "file_search").mkdir(parents=True)
        (client / "features" / "file_search" / "panel.js").write_text(
            "export const panel = true;\n",
            encoding="utf-8",
        )
        (client / "depth" / "a" / "b" / "c" / "d").mkdir(parents=True)
        (client / "depth" / "a" / "b" / "c" / "d" / "too-deep.txt").write_text(
            "hidden by tree depth\n",
            encoding="utf-8",
        )
        (client / "many").mkdir()
        for idx in range(api.FILES_TREE_MAX_ENTRIES + 5):
            (client / "many" / f"{idx:03d}.txt").write_text(
                f"{idx}\n",
                encoding="utf-8",
            )
        (client / ".hidden").write_text("visible\n", encoding="utf-8")
        (client / ".git" / "refine-hidden-search.txt").write_text(
            "must stay hidden\n",
            encoding="utf-8",
        )
        (client / "image.png").write_bytes(b"\x89PNG\r\n\x1a\npreview")
        (client / "blob.bin").write_bytes(b"abc\x00def")
        (client / "large.txt").write_bytes(b"x" * (api.FILE_PREVIEW_MAX_BYTES + 1))

        status, body = api.files_tree("")
        assert status == 200, body
        names = [entry["name"] for entry in body["entries"]]
        assert "src" in names, names
        assert ".hidden" in names, names
        assert ".git" not in names, names

        status, body = api.files_tree("src")
        assert status == 200, body
        assert body["path"] == "src", body
        assert body["entries"][0]["path"] == "src/app.py", body
        assert body["max_depth"] == api.FILES_TREE_MAX_DEPTH, body
        assert body["max_entries"] == api.FILES_TREE_MAX_ENTRIES, body

        status, body = api.files_tree("many")
        assert status == 200, body
        assert len(body["entries"]) == api.FILES_TREE_MAX_ENTRIES, body
        assert body["truncated"] is True, body

        status, body = api.files_tree("depth", recursive=True)
        assert status == 200, body
        assert "depth" in body["entries_by_path"], body
        assert "depth/a/b/c" in body["entries_by_path"], body
        assert "depth/a/b/c/d" not in body["entries_by_path"], body
        assert body["meta_by_path"]["depth/a/b/c"]["depth"] == api.FILES_TREE_MAX_DEPTH, body

        status, body = api.files_tree("", recursive=True)
        assert status == 200, body
        assert ".git" not in body["entries_by_path"], body

        status, body = api.files_search("helper")
        assert status == 200, body
        assert body["query"] == "helper", body
        assert body["entries"][0]["path"] == "src/helpers.py", body

        status, body = api.files_search("srcapp")
        assert status == 200, body
        assert body["entries"][0]["path"] == "src/app.py", body

        status, body = api.files_search("fs panel")
        assert status == 200, body
        assert body["entries"][0]["path"] == "features/file_search/panel.js", body

        status, body = api.files_search("refine-hidden-search")
        assert status == 200, body
        assert body["entries"] == [], body

        status, body = api.files_search("txt", max_entries=3)
        assert status == 200, body
        assert len(body["entries"]) == 3, body
        assert body["truncated"] is True, body
        assert body["max_entries"] == 3, body

        status, body = api.files_search("")
        assert status == 200, body
        assert body["entries"] == [], body

        status, body = api.files_read("src/app.py")
        assert status == 200, body
        assert body["previewable"] is True, body
        assert body["content"].startswith("def hello"), body

        status, body = api.files_read("blob.bin")
        assert status == 200, body
        assert body["previewable"] is False, body
        assert body["reason"] == "Binary data", body

        status, body = api.files_read("image.png")
        assert status == 200, body
        assert body["previewable"] is True, body
        assert body["kind"] == "image", body
        assert body["data_url"].startswith("data:image/png;base64,"), body

        status, body = api.files_read("large.txt")
        assert status == 200, body
        assert body["previewable"] is True, body
        assert body["large"] is True, body
        assert body["has_more"] is True, body
        assert len(body["content"]) == api.FILE_TEXT_CHUNK_BYTES, body
        status, next_body = api.files_read("large.txt", offset=body["next_offset"])
        assert status == 200, next_body
        assert next_body["offset"] == body["next_offset"], next_body
        assert next_body["content"], next_body

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
