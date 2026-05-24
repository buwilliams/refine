"""Installer smoke checks."""
from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
from pathlib import Path


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    install_sh = root / "install.sh"
    readme = root / "README.md"

    subprocess.run(["bash", "-n", str(install_sh)], cwd=root, check=True)
    print("[ok] install.sh syntax")

    script = install_sh.read_text(encoding="utf-8")
    assert "https://raw.githubusercontent.com/buwilliams/refine/main/install.sh" in script
    assert "https://astral.sh/uv/install.sh" in script
    assert "https://get.docker.com/rootless" in script
    assert "https://gh.io/copilot-install" in script
    assert "REFINE_INSTALL_DRY_RUN" in script
    print("[ok] install.sh keeps expected install sources and dry-run hook")

    readme_text = readme.read_text(encoding="utf-8")
    assert "## Quick Start" in readme_text
    assert "### Windows Users" in readme_text
    assert "wsl --install" in readme_text
    assert "curl -fsSL https://raw.githubusercontent.com/buwilliams/refine/main/install.sh | bash" in readme_text
    print("[ok] README points users at install.sh, including Windows")

    tmp = Path(tempfile.mkdtemp(prefix="refine-install-test-"))
    try:
        checkout = tmp / "refine"
        target = tmp / "target-app"
        checkout.mkdir()
        target.mkdir()
        subprocess.run(["git", "init", "-q"], cwd=checkout, check=True)
        subprocess.run(["git", "init", "-q"], cwd=target, check=True)

        fake_bin = tmp / "bin"
        fake_bin.mkdir()
        if shutil.which("uv") is None:
            uv = fake_bin / "uv"
            uv.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            uv.chmod(0o755)

        env = os.environ.copy()
        env.update({
            "NO_COLOR": "1",
            "REFINE_INSTALL_ASSUME_DEFAULTS": "1",
            "REFINE_INSTALL_DRY_RUN": "1",
            "REFINE_INSTALL_BASE_DEFAULT": str(tmp),
            "REFINE_INSTALL_TARGET_APP": str(target),
            "REFINE_INSTALL_PROVIDER": "codex",
            "PATH": f"{fake_bin}{os.pathsep}{env.get('PATH', '')}",
        })
        result = subprocess.run(
            ["bash", str(install_sh)],
            cwd=root,
            env=env,
            text=True,
            capture_output=True,
            check=True,
        )
        output = result.stdout + result.stderr
        assert "Dry run mode" in output
        assert "set Refine setting agent_cli=codex" in output
        assert "Provider:         codex" in output
        print("[ok] install.sh dry-run completes without mutating checkout state")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
