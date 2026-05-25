"""Installer smoke checks."""
from __future__ import annotations

import os
import pty
import shutil
import shlex
import signal
import select
import subprocess
import tempfile
import time
from pathlib import Path


def _run_piped_installer_with_pty(
    install_sh: Path,
    root: Path,
    env: dict[str, str],
    answers: list[str],
) -> tuple[int, str]:
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(root)
        os.execvpe(
            "bash",
            ["bash", "-c", f"cat {shlex.quote(str(install_sh))} | bash"],
            env,
        )

    output = bytearray()
    pending = ("\n".join(answers) + "\n").encode()
    wrote_answers = False
    status = 0
    started_at = time.monotonic()
    try:
        while True:
            if time.monotonic() - started_at > 20:
                os.kill(pid, signal.SIGTERM)
                _, status = os.waitpid(pid, 0)
                return 124, output.decode("utf-8", errors="replace")
            if not wrote_answers:
                os.write(fd, pending)
                wrote_answers = True
            ready, _, _ = select.select([fd], [], [], 0.1)
            if ready:
                try:
                    chunk = os.read(fd, 4096)
                except OSError:
                    chunk = b""
                if chunk:
                    output.extend(chunk)
            finished, status = os.waitpid(pid, os.WNOHANG)
            if finished:
                while True:
                    ready, _, _ = select.select([fd], [], [], 0)
                    if not ready:
                        break
                    try:
                        chunk = os.read(fd, 4096)
                    except OSError:
                        break
                    if not chunk:
                        break
                    output.extend(chunk)
                break
    finally:
        os.close(fd)

    if os.WIFEXITED(status):
        code = os.WEXITSTATUS(status)
    elif os.WIFSIGNALED(status):
        code = 128 + os.WTERMSIG(status)
    else:
        code = 1
    return code, output.decode("utf-8", errors="replace")


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

    tmp = Path(tempfile.mkdtemp(prefix="refine-install-pty-test-"))
    try:
        default_workspace = tmp / "default-workspace"
        chosen_workspace = tmp / "chosen-workspace"
        checkout = chosen_workspace / "refine"
        target = tmp / "target-app"
        checkout.mkdir(parents=True)
        target.mkdir()
        subprocess.run(["git", "init", "-q"], cwd=checkout, check=True)
        subprocess.run(["git", "init", "-q"], cwd=target, check=True)

        fake_bin = tmp / "bin"
        fake_bin.mkdir()
        for name in ("codex", "docker", "uv"):
            executable = fake_bin / name
            executable.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            executable.chmod(0o755)

        env = os.environ.copy()
        env.update({
            "NO_COLOR": "1",
            "REFINE_INSTALL_DRY_RUN": "1",
            "REFINE_INSTALL_BASE_DEFAULT": str(default_workspace),
            "PATH": f"{fake_bin}{os.pathsep}{env.get('PATH', '')}",
        })
        code, output = _run_piped_installer_with_pty(
            install_sh,
            root,
            env,
            [
                str(chosen_workspace),
                "n",
                "codex",
                "n",
                str(target),
                "n",
                "18080",
                "n",
            ],
        )
        assert code == 0, output
        assert "Install workspace" in output
        assert "Provider (claude codex gemini copilot)" in output
        assert "Target app path or Git remote" in output
        assert f"Refine checkout: {checkout}" in output
        assert "Provider:         codex" in output
        assert f"Cloned Refine to {default_workspace / 'refine'}" not in output
        print("[ok] install.sh prompts through /dev/tty when piped to bash")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
