"""Tests for Refine checkout-local .env support."""
from __future__ import annotations

import os
import shutil
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))


def main() -> int:
    from refine_cli import cli
    from refine_server import config

    root = Path(__file__).resolve().parents[1]
    example = (root / ".env.example").read_text(encoding="utf-8")
    assert "REFINE_SMOKE_AI_PATH" in example
    assert "OPENAI_API_KEY" not in example
    assert "REFINE_UI_PORT" not in example
    gitignore = (root / ".gitignore").read_text(encoding="utf-8")
    assert ".env" in gitignore
    assert ".env.example" not in gitignore

    tmp = Path(tempfile.mkdtemp(prefix="refine-dotenv-"))
    old_smoke = os.environ.get("REFINE_SMOKE_AI_PATH")
    old_existing = os.environ.get("REFINE_DOTENV_EXISTING")
    try:
        clone = tmp / "refine-clone"
        clone.mkdir()
        (clone / "pyproject.toml").write_text("[project]\nname='refine'\n", encoding="utf-8")
        (clone / "refine_cli").mkdir()
        (clone / ".env").write_text(
            "\n".join([
                "# local refine env",
                "REFINE_SMOKE_AI_PATH=/opt/refine/smoke-ai",
                "export REFINE_DOTENV_QUOTED=\"hello world\"",
                "REFINE_DOTENV_HASH=value # comment",
                "1INVALID=ignored",
                "REFINE_DOTENV_EXISTING=from-file",
                "",
            ]),
            encoding="utf-8",
        )
        os.environ["REFINE_DOTENV_EXISTING"] = "from-shell"
        os.environ.pop("REFINE_SMOKE_AI_PATH", None)
        loaded = config.load_dotenv(clone)
        assert loaded["REFINE_SMOKE_AI_PATH"] == "/opt/refine/smoke-ai"
        assert os.environ["REFINE_SMOKE_AI_PATH"] == "/opt/refine/smoke-ai"
        assert os.environ["REFINE_DOTENV_QUOTED"] == "hello world"
        assert os.environ["REFINE_DOTENV_HASH"] == "value"
        assert os.environ["REFINE_DOTENV_EXISTING"] == "from-shell"
        assert "1INVALID" not in os.environ

        client = tmp / "client"
        client.mkdir()
        config.write_binding(clone, client)
        nested = clone / "nested"
        nested.mkdir()
        assert config.find_dotenv(nested) == clone / ".env"

        legacy = clone / ".env"
        legacy.write_text(
            "REFINE_CLIENT_REFINE_DIR=/old/path\n"
            "REFINE_SMOKE_AI_PATH=/kept/smoke-ai\n",
            encoding="utf-8",
        )
        cli._remove_legacy_docker_artifacts(clone)
        assert legacy.read_text(encoding="utf-8") == "REFINE_SMOKE_AI_PATH=/kept/smoke-ai\n"

        legacy.write_text("REFINE_CLIENT_REFINE_DIR=/old/path\n", encoding="utf-8")
        cli._remove_legacy_docker_artifacts(clone)
        assert not legacy.exists()
    finally:
        if old_smoke is None:
            os.environ.pop("REFINE_SMOKE_AI_PATH", None)
        else:
            os.environ["REFINE_SMOKE_AI_PATH"] = old_smoke
        if old_existing is None:
            os.environ.pop("REFINE_DOTENV_EXISTING", None)
        else:
            os.environ["REFINE_DOTENV_EXISTING"] = old_existing
        for key in ("REFINE_DOTENV_QUOTED", "REFINE_DOTENV_HASH"):
            os.environ.pop(key, None)
        shutil.rmtree(tmp, ignore_errors=True)

    print("dotenv tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
