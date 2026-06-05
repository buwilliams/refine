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
    cwd: Path | None = None,
) -> tuple[int, str]:
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(cwd or root)
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


def _write_refine_checkout(checkout: Path, *, legacy: bool = False) -> None:
    if legacy:
        (checkout / "refine_cli").mkdir(parents=True, exist_ok=True)
        (checkout / "pyproject.toml").write_text(
            "[project]\nname = \"refine\"\n",
            encoding="utf-8",
        )
        (checkout / "refine_cli" / "cli.py").write_text("# marker\n", encoding="utf-8")
        (checkout / "install.sh").write_text("# marker\n", encoding="utf-8")
    else:
        (checkout / "python" / "refine_cli").mkdir(parents=True, exist_ok=True)
        (checkout / "python" / "pyproject.toml").write_text(
            "[project]\nname = \"refine\"\n",
            encoding="utf-8",
        )
        (checkout / "python" / "refine_cli" / "cli.py").write_text(
            "# marker\n",
            encoding="utf-8",
        )
        (checkout / "scripts").mkdir(exist_ok=True)
        (checkout / "scripts" / "install.sh").write_text("# marker\n", encoding="utf-8")


def main() -> int:
    root = Path(__file__).resolve().parents[2]
    install_sh = root / "scripts" / "install.sh"
    readme = root / "README.md"

    subprocess.run(["bash", "-n", str(install_sh)], cwd=root, check=True)
    print("[ok] install.sh syntax")

    script = install_sh.read_text(encoding="utf-8")
    assert "https://raw.githubusercontent.com/buwilliams/refine/main/scripts/install.sh" in script
    assert "https://astral.sh/uv/install.sh" in script
    assert "Install uv with pipx" in script
    assert "install_packages pipx" in script
    assert "pipx install uv" in script
    assert "sudo apt install pipx && pipx install uv" in script
    assert "https://get.docker.com/rootless" in script
    assert "https://gh.io/copilot-install" in script
    assert "npx --yes playwright install --with-deps chromium" in script
    assert "REFINE_INSTALL_DRY_RUN" in script
    assert "REFINE_INSTALL_UPGRADE" in script
    assert "REFINE_INSTALL_LOG" in script
    assert 'REFINE_INSTALL_BASE_DEFAULT:-$HOME/refine}' in script
    assert "refine-work/refine" not in script
    assert 'INSTALL_LOG="/tmp/refine-install-$$.log"' in script
    assert "print_splash" in script
    assert "choose_install_mode" in script
    assert "           ___            " in script
    assert "          __ _            " not in script
    assert "install, repair, and upgrade script" in script
    assert "local AI workflow setup" not in script
    assert "${BOLD}${CYAN}refine${RESET}" in script
    assert "Continue with Refine install" not in script
    assert "Is this a new Refine install" in script
    assert "Existing Refine checkout path" in script
    assert 'while [ "$attempt" -le 2 ]; do' in script
    assert "Could not find Refine at the provided location." in script
    assert "No existing Refine checkout detected; assuming a fresh install." in script
    assert "[info]" in script
    assert "[ready]" in script
    assert "[warn]" in script
    assert "[error]" in script
    assert "--yes" in script
    assert "--upgrade" in script
    assert "--no-upgrade" in script
    assert 'REFINE_INSTALL_UPGRADE="${REFINE_INSTALL_UPGRADE:-1}"' in script
    assert 'REFINE_INSTALL_PORT="${REFINE_INSTALL_PORT:-}"' in script
    assert 'REFINE_UPDATE_TARGET_APP="${REFINE_UPDATE_TARGET_APP:-1}"' in script
    assert "latest_remote_semver_release_tag" in script
    assert "api.github.com/repos/$slug/releases" in script
    assert "upgrade_refine_checkout" in script
    assert "checkout_ahead_of_semver_tag" in script
    assert "current_refine_checkout" in script
    assert "Using current Refine checkout" in script
    assert "bound_target_app" in script
    assert "Using existing target app binding" in script
    assert "recorded_primary_port" in script
    assert "resolve_refine_port" in script
    assert "restart_refine_after_upgrade" in script
    assert "Restart Refine now to run" in script
    assert 'uv --project "$(refine_project_dir)" run refine restart "$port"' in script
    assert 'uv --project "$(refine_project_dir)" run refine app rebuild --port "$port"' in script
    assert "assuming local development and skipping release upgrade" in script
    assert "not on a semver release tag" in script
    assert 'if [ "$REFINE_UPGRADED" = "1" ]; then' in script
    assert 'confirm "Install or repair Playwright Chromium for regression screenshots" "$default_answer"' in script
    assert 'git clone --branch "$latest" "$REFINE_REPO_URL" "$checkout"' in script
    assert 'uv --project "$(refine_project_dir)" run refine target "$TARGET_APP_PATH" --force --port "$port"' in script
    assert 'REFINE_UI_PORT="$port" REFINE_UI_SCOPE="$port"' in script
    assert "uv --project python run refine install $port" in script
    assert "Some install steps did not complete" in script
    assert "Why it is needed:" in script
    assert "What to do:" in script
    assert "The install.sh script can be used again to: repair or upgrade." in script
    assert 'say "  curl -fsSL $REFINE_RAW_INSTALL_URL | bash"' not in script
    print("[ok] install.sh keeps expected install sources and dry-run hook")

    readme_text = readme.read_text(encoding="utf-8")
    assert "## Quick Start" in readme_text
    assert "### Windows Users" in readme_text
    assert "wsl --install" in readme_text
    assert "curl -fsSL https://raw.githubusercontent.com/buwilliams/refine/main/scripts/install.sh | bash" in readme_text
    print("[ok] README points users at install.sh, including Windows")

    tmp = Path(tempfile.mkdtemp(prefix="refine-install-test-"))
    try:
        checkout = tmp / "refine"
        target = tmp / "target-app"
        checkout.mkdir()
        target.mkdir()
        _write_refine_checkout(checkout)
        subprocess.run(["git", "init", "-q"], cwd=checkout, check=True)
        subprocess.run(["git", "init", "-q"], cwd=target, check=True)

        fake_bin = tmp / "bin"
        fake_bin.mkdir()
        if shutil.which("uv") is None:
            uv = fake_bin / "uv"
            uv.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            uv.chmod(0o755)

        env = os.environ.copy()
        install_log = tmp / "install.log"
        env.update({
            "NO_COLOR": "1",
            "REFINE_INSTALL_ASSUME_DEFAULTS": "1",
            "REFINE_INSTALL_DRY_RUN": "1",
            "REFINE_INSTALL_BASE_DEFAULT": str(checkout),
            "REFINE_INSTALL_TARGET_APP": str(target),
            "REFINE_INSTALL_PROVIDER": "codex",
            "REFINE_INSTALL_LOG": str(install_log),
            "PATH": f"{fake_bin}{os.pathsep}{env.get('PATH', '')}",
        })
        result = subprocess.run(
            ["bash", str(install_sh)],
            cwd=tmp,
            env=env,
            text=True,
            capture_output=True,
            check=True,
        )
        output = result.stdout + result.stderr
        log_text = install_log.read_text(encoding="utf-8")
        assert "refine" in output
        assert "Quiet terminal, detailed log, clear next steps." in output
        assert "Continue with Refine install" not in output
        assert "Is this a new Refine install" not in output
        assert "Dry run mode" in output
        assert "uv --project" not in output
        assert "set Refine setting agent_cli=codex" not in output
        assert "uv --project" in log_text
        assert "run refine target" in log_text
        assert "set Refine setting agent_cli=codex" in log_text
        assert "Provider:         codex" in output
        assert f"Install log: {install_log}" in output
        print("[ok] install.sh dry-run completes without mutating checkout state")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    tmp = Path(tempfile.mkdtemp(prefix="refine-install-pipx-uv-test-"))
    try:
        checkout = tmp / "refine"
        checkout.mkdir()
        _write_refine_checkout(checkout)
        subprocess.run(["git", "init", "-q"], cwd=checkout, check=True)

        fake_bin = tmp / "bin"
        fake_bin.mkdir()
        for name in (
            "awk", "bash", "cat", "chmod", "dirname", "git", "grep", "id",
            "mkdir", "mktemp", "python3", "rm", "sed", "sort", "tail",
            "touch", "tr", "uname",
        ):
            source = shutil.which(name)
            assert source is not None, name
            (fake_bin / name).symlink_to(source)
        node = fake_bin / "node"
        node.write_text("#!/bin/sh\nprintf 'v20.0.0\\n'\n", encoding="utf-8")
        node.chmod(0o755)
        npx = fake_bin / "npx"
        npx.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        npx.chmod(0o755)
        codex = fake_bin / "codex"
        codex.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        codex.chmod(0o755)
        curl = fake_bin / "curl"
        curl.write_text("#!/bin/sh\nexit 1\n", encoding="utf-8")
        curl.chmod(0o755)
        sudo = fake_bin / "sudo"
        sudo.write_text("#!/bin/sh\nexec \"$@\"\n", encoding="utf-8")
        sudo.chmod(0o755)
        apt_get = fake_bin / "apt-get"
        apt_get.write_text(
            "#!/bin/sh\n"
            f"printf '%s\\n' \"$@\" >> {shlex.quote(str(tmp / 'apt-get.log'))}\n"
            "if [ \"$1\" = \"install\" ] && [ \"$3\" = \"pipx\" ]; then\n"
            f"  cat > {shlex.quote(str(fake_bin / 'pipx'))} <<'SH'\n"
            "#!/bin/sh\n"
            f"printf '%s\\n' \"$@\" >> {shlex.quote(str(tmp / 'pipx.log'))}\n"
            "if [ \"$1\" = \"install\" ] && [ \"$2\" = \"uv\" ]; then\n"
            f"  mkdir -p {shlex.quote(str(tmp / '.local' / 'bin'))}\n"
            f"  cat > {shlex.quote(str(tmp / '.local' / 'bin' / 'uv'))} <<'UV'\n"
            "#!/bin/sh\n"
            "exit 0\n"
            "UV\n"
            f"  chmod +x {shlex.quote(str(tmp / '.local' / 'bin' / 'uv'))}\n"
            "fi\n"
            "SH\n"
            f"  chmod +x {shlex.quote(str(fake_bin / 'pipx'))}\n"
            "fi\n"
            "exit 0\n",
            encoding="utf-8",
        )
        apt_get.chmod(0o755)

        env = os.environ.copy()
        install_log = tmp / "install.log"
        env.update({
            "HOME": str(tmp),
            "NO_COLOR": "1",
            "REFINE_INSTALL_PROVIDER": "codex",
            "REFINE_INSTALL_UPGRADE": "0",
            "REFINE_INSTALL_LOG": str(install_log),
            "PATH": str(fake_bin),
        })
        result = subprocess.run(
            ["bash", str(install_sh), "--yes"],
            cwd=checkout,
            env=env,
            text=True,
            capture_output=True,
            check=True,
        )
        output = result.stdout + result.stderr
        log_text = install_log.read_text(encoding="utf-8")
        apt_log = (tmp / "apt-get.log").read_text(encoding="utf-8")
        pipx_log = (tmp / "pipx.log").read_text(encoding="utf-8")
        assert "Could not download uv installer from https://astral.sh/uv/install.sh" in output
        assert "pipx is not installed" in output
        assert "uv installed:" in output
        assert "install\n-y\npipx\n" in apt_log
        assert "install\nuv\n" in pipx_log
        assert "uv is required." not in output
        assert "Failed: uv install" not in output
        assert "uv is required." not in log_text
        print("[ok] install.sh falls back to pipx when Astral uv install is blocked")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    tmp = Path(tempfile.mkdtemp(prefix="refine-install-no-target-test-"))
    try:
        checkout = tmp / "refine"
        checkout.mkdir()
        _write_refine_checkout(checkout)
        subprocess.run(["git", "init", "-q"], cwd=checkout, check=True)

        fake_bin = tmp / "bin"
        fake_bin.mkdir()
        for name in (
            "awk", "bash", "cat", "curl", "dirname", "git", "grep", "python3",
            "sort", "tail", "tr", "uname",
        ):
            source = shutil.which(name)
            assert source is not None, name
            (fake_bin / name).symlink_to(source)
        codex = fake_bin / "codex"
        codex.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        codex.chmod(0o755)
        home_bin = tmp / ".local" / "bin"
        home_bin.mkdir(parents=True)
        uv = home_bin / "uv"
        uv.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        uv.chmod(0o755)

        env = os.environ.copy()
        install_log = tmp / "install.log"
        env.update({
            "HOME": str(tmp),
            "NO_COLOR": "1",
            "REFINE_INSTALL_DRY_RUN": "1",
            "REFINE_INSTALL_BASE_DEFAULT": str(checkout),
            "REFINE_INSTALL_PROVIDER": "codex",
            "REFINE_INSTALL_LOG": str(install_log),
            "PATH": str(fake_bin),
        })
        result = subprocess.run(
            ["bash", str(install_sh), "--yes"],
            cwd=tmp,
            env=env,
            text=True,
            capture_output=True,
            check=True,
        )
        output = result.stdout + result.stderr
        log_text = install_log.read_text(encoding="utf-8")
        assert "A target app path or Git remote is required" not in output
        assert "No target app attached" in output
        assert "Skipping target-app attachment" in output
        assert "link" not in output
        assert "uv is available in this shell" not in output
        assert "link" in log_text and "uv is available in this shell" in log_text
        assert "uv run refine init" not in output
        assert "run refine target" not in output
        assert "Target app:       not attached yet" in output
        print("[ok] install.sh can complete without an initial target app")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    tmp = Path(tempfile.mkdtemp(prefix="refine-install-legacy-checkout-test-"))
    try:
        checkout = tmp / "refine"
        checkout.mkdir()
        _write_refine_checkout(checkout, legacy=True)
        subprocess.run(["git", "init", "-q"], cwd=checkout, check=True)
        subprocess.run(["git", "config", "user.name", "Refine Test"], cwd=checkout, check=True)
        subprocess.run(["git", "config", "user.email", "refine@example.test"], cwd=checkout, check=True)
        subprocess.run(["git", "add", "."], cwd=checkout, check=True)
        subprocess.run(["git", "commit", "-q", "-m", "legacy install"], cwd=checkout, check=True)

        fake_bin = tmp / "bin"
        fake_bin.mkdir()
        for name in (
            "awk", "bash", "cat", "curl", "dirname", "git", "grep", "python3",
            "sort", "tail", "tr", "uname",
        ):
            source = shutil.which(name)
            assert source is not None, name
            (fake_bin / name).symlink_to(source)
        for name in ("codex", "docker", "uv"):
            executable = fake_bin / name
            executable.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            executable.chmod(0o755)

        env = os.environ.copy()
        install_log = tmp / "install.log"
        env.update({
            "HOME": str(tmp),
            "NO_COLOR": "1",
            "REFINE_INSTALL_DRY_RUN": "1",
            "REFINE_INSTALL_DRY_RUN_RELEASE_TAG": "1.0.0",
            "REFINE_INSTALL_BASE_DEFAULT": str(checkout),
            "REFINE_INSTALL_PROVIDER": "codex",
            "REFINE_INSTALL_LOG": str(install_log),
            "PATH": str(fake_bin),
        })
        result = subprocess.run(
            ["bash", str(install_sh), "--yes"],
            cwd=tmp,
            env=env,
            text=True,
            capture_output=True,
            check=True,
        )
        output = result.stdout + result.stderr
        assert "No existing Refine checkout detected; assuming a fresh install." in output
        assert f"Refine checkout exists: {checkout}" in output
        assert "Current Refine checkout is not on a semver release tag." in output
        assert "Skipping release upgrade" not in output
        assert "Refine upgraded to release 1.0.0" in output
        print("[ok] install.sh upgrades a clean legacy checkout from fresh mode")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    tmp = Path(tempfile.mkdtemp(prefix="refine-install-failure-summary-test-"))
    try:
        checkout = tmp / "refine"
        (checkout / "python" / "refine_cli").mkdir(parents=True)
        (checkout / "scripts").mkdir()
        (checkout / "python" / "pyproject.toml").write_text(
            "[project]\nname = \"refine\"\n",
            encoding="utf-8",
        )
        (checkout / "python" / "refine_cli" / "cli.py").write_text("# marker\n", encoding="utf-8")
        (checkout / "scripts" / "install.sh").write_text("# marker\n", encoding="utf-8")
        subprocess.run(["git", "init", "-q"], cwd=checkout, check=True)

        fake_bin = tmp / "bin"
        fake_bin.mkdir()
        for name in (
            "awk", "bash", "cat", "curl", "dirname", "git", "grep", "python3",
            "sed", "sort", "tail", "tr", "uname",
        ):
            source = shutil.which(name)
            assert source is not None, name
            (fake_bin / name).symlink_to(source)
        codex = fake_bin / "codex"
        codex.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        codex.chmod(0o755)
        node = fake_bin / "node"
        node.write_text("#!/bin/sh\nprintf 'v20.0.0\\n'\n", encoding="utf-8")
        node.chmod(0o755)
        for name in ("npm", "npx"):
            executable = fake_bin / name
            executable.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            executable.chmod(0o755)
        uv = fake_bin / "uv"
        uv.write_text(
            "#!/bin/sh\n"
            "echo hidden uv stdout\n"
            "echo hidden uv stderr >&2\n"
            "if [ \"$1\" = \"--project\" ] && [ \"$3\" = \"run\" ] && [ \"$4\" = \"refine\" ] && [ \"$5\" = \"start\" ]; then\n"
            "  exit 1\n"
            "fi\n"
            "exit 0\n",
            encoding="utf-8",
        )
        uv.chmod(0o755)

        env = os.environ.copy()
        install_log = tmp / "install.log"
        env.update({
            "HOME": str(tmp),
            "NO_COLOR": "1",
            "REFINE_INSTALL_PROVIDER": "codex",
            "REFINE_INSTALL_UPGRADE": "0",
            "REFINE_INSTALL_LOG": str(install_log),
            "PATH": str(fake_bin),
        })
        result = subprocess.run(
            ["bash", str(install_sh), "--yes"],
            cwd=checkout,
            env=env,
            text=True,
            capture_output=True,
            check=True,
        )
        output = result.stdout + result.stderr
        log_text = install_log.read_text(encoding="utf-8")
        assert "Needs attention" in output
        assert "Some install steps did not complete:" in output
        assert "- Failed: Refine background start" in output
        assert "Why it is needed: Refine must be running for the browser UI." in output
        assert "What to do: Run manually:" in output
        assert f"Log: {install_log}" in output
        assert "hidden uv stdout" not in output
        assert "hidden uv stderr" not in output
        assert "hidden uv stdout" in log_text
        assert "hidden uv stderr" in log_text
        assert "The install.sh script can be used again to: repair or upgrade." in output
        print("[ok] install.sh summarizes recoverable failures at the end")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    tmp = Path(tempfile.mkdtemp(prefix="refine-install-pty-test-"))
    try:
        default_checkout = tmp / "default-checkout"
        chosen_checkout = tmp / "chosen-checkout"
        checkout = chosen_checkout
        target = tmp / "target-app"
        checkout.mkdir(parents=True)
        target.mkdir()
        _write_refine_checkout(checkout)
        subprocess.run(["git", "init", "-q"], cwd=checkout, check=True)
        subprocess.run(["git", "init", "-q"], cwd=target, check=True)

        fake_bin = tmp / "bin"
        fake_bin.mkdir()
        for name in (
            "awk", "bash", "cat", "curl", "dirname", "git", "grep", "python3",
            "sort", "tail", "tr", "uname",
        ):
            source = shutil.which(name)
            assert source is not None, name
            (fake_bin / name).symlink_to(source)
        for name in ("codex", "docker", "uv"):
            executable = fake_bin / name
            executable.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            executable.chmod(0o755)

        env = os.environ.copy()
        install_log = tmp / "install.log"
        env.update({
            "HOME": str(tmp),
            "NO_COLOR": "1",
            "REFINE_INSTALL_DRY_RUN": "1",
            "REFINE_INSTALL_BASE_DEFAULT": str(default_checkout),
            "REFINE_INSTALL_LOG": str(install_log),
            "PATH": str(fake_bin),
        })
        code, output = _run_piped_installer_with_pty(
            install_sh,
            root,
            env,
            [
                "",
                str(chosen_checkout),
                "codex",
                "n",
                "n",
                "18080",
                "n",
            ],
            cwd=tmp,
        )
        assert code == 0, output
        assert "Refine checkout path" in output
        assert "Installed provider CLIs: codex" in output
        assert "Missing provider CLIs: claude gemini copilot smoke-ai" in output
        assert "Provider (claude codex gemini copilot smoke-ai) [codex]" in output
        assert "Target app path or Git remote" not in output
        assert "No target app attached" in output
        assert "Target app:       not attached yet" in output
        assert f"Refine checkout: {checkout}" in output
        assert "Provider:         codex" in output
        assert f"Cloned Refine to {default_checkout / 'refine'}" not in output
        assert f"Refine checkout: {chosen_checkout / 'refine'}" not in output
        print("[ok] install.sh prompts through /dev/tty when piped to bash")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    tmp = Path(tempfile.mkdtemp(prefix="refine-install-current-checkout-test-"))
    try:
        checkout = tmp / "refine"
        target = tmp / "target-app"
        (checkout / "python" / "refine_cli").mkdir(parents=True)
        (checkout / "scripts").mkdir()
        target.mkdir()
        (checkout / "python" / "pyproject.toml").write_text(
            "[project]\nname = \"refine\"\n",
            encoding="utf-8",
        )
        (checkout / "python" / "refine_cli" / "cli.py").write_text("# marker\n", encoding="utf-8")
        (checkout / "scripts" / "install.sh").write_text("# marker\n", encoding="utf-8")
        (checkout / ".refine-binding").write_text(
            f"# refine binding\n{target}\n",
            encoding="utf-8",
        )
        subprocess.run(["git", "init", "-q"], cwd=checkout, check=True)
        subprocess.run(["git", "init", "-q"], cwd=target, check=True)

        fake_bin = tmp / "bin"
        fake_bin.mkdir()
        for name in (
            "awk", "bash", "cat", "curl", "dirname", "git", "grep", "python3",
            "sort", "tail", "tr", "uname",
        ):
            source = shutil.which(name)
            assert source is not None, name
            (fake_bin / name).symlink_to(source)
        for name in ("codex", "docker", "uv"):
            executable = fake_bin / name
            executable.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            executable.chmod(0o755)

        env = os.environ.copy()
        install_log = tmp / "install.log"
        env.update({
            "HOME": str(tmp),
            "NO_COLOR": "1",
            "REFINE_INSTALL_DRY_RUN": "1",
            "REFINE_INSTALL_LOG": str(install_log),
            "PATH": str(fake_bin),
        })
        code, output = _run_piped_installer_with_pty(
            install_sh,
            root,
            env,
            [
                "codex",
                "n",
                "n",
                "n",
                "18080",
                "n",
            ],
            cwd=checkout,
        )
        assert code == 0, output
        assert "Using current Refine checkout" in output
        assert f"Refine checkout: {checkout}" in output
        assert "Refine checkout path" not in output
        assert "Using existing target app binding" in output
        assert "Target app path or Git remote" not in output
        assert f"Target app:       {target}" in output
        print("[ok] install.sh skips checkout and target prompts from a bound Refine checkout")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    tmp = Path(tempfile.mkdtemp(prefix="refine-install-upgrade-restart-test-"))
    try:
        checkout = tmp / "refine"
        target = tmp / "target-app"
        (checkout / "python" / "refine_cli").mkdir(parents=True)
        (checkout / "scripts").mkdir()
        target.mkdir()
        (checkout / "python" / "pyproject.toml").write_text(
            "[project]\nname = \"refine\"\n",
            encoding="utf-8",
        )
        (checkout / "python" / "refine_cli" / "cli.py").write_text("# marker\n", encoding="utf-8")
        (checkout / "scripts" / "install.sh").write_text("# marker\n", encoding="utf-8")
        (checkout / ".refine-binding").write_text(
            f"# refine binding\n{target}\n",
            encoding="utf-8",
        )
        subprocess.run(["git", "init", "-q"], cwd=checkout, check=True)
        subprocess.run(["git", "config", "user.name", "Refine Test"], cwd=checkout, check=True)
        subprocess.run(["git", "config", "user.email", "refine@example.test"], cwd=checkout, check=True)
        subprocess.run(["git", "add", "."], cwd=checkout, check=True)
        subprocess.run(["git", "commit", "-q", "-m", "release"], cwd=checkout, check=True)
        subprocess.run(["git", "tag", "0.9.0"], cwd=checkout, check=True)
        subprocess.run(["git", "init", "-q"], cwd=target, check=True)

        fake_bin = tmp / "bin"
        fake_bin.mkdir()
        for name in (
            "awk", "bash", "cat", "curl", "dirname", "git", "grep", "python3",
            "sort", "tail", "tr", "uname",
        ):
            source = shutil.which(name)
            assert source is not None, name
            (fake_bin / name).symlink_to(source)
        for name in ("codex", "docker", "uv"):
            executable = fake_bin / name
            executable.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            executable.chmod(0o755)

        env = os.environ.copy()
        install_log = tmp / "install.log"
        env.update({
            "HOME": str(tmp),
            "NO_COLOR": "1",
            "REFINE_INSTALL_DRY_RUN": "1",
            "REFINE_INSTALL_DRY_RUN_RELEASE_TAG": "1.0.0",
            "REFINE_INSTALL_LOG": str(install_log),
            "PATH": str(fake_bin),
        })
        code, output = _run_piped_installer_with_pty(
            install_sh,
            root,
            env,
            [
                "codex",
                "n",
                "n",
                "n",
                "n",
            ],
            cwd=checkout,
        )
        assert code == 0, output
        assert "Refine upgraded to release 1.0.0" in output
        assert "Install or repair Playwright Chromium for regression screenshots [y/N]" in output
        assert "Restart Refine now to run 1.0.0 [Y/n]" in output
        assert "Refine was upgraded but not restarted" in output
        assert "uv --project python run refine restart" in output
        assert "run refine start" not in output
        log_text = install_log.read_text(encoding="utf-8")
        assert "+ uv --project" not in "\n".join(
            line for line in log_text.splitlines()
            if "run refine restart" in line
        )
        assert "run refine app rebuild" not in log_text
        assert "Refresh target application" not in output

        result = subprocess.run(
            ["bash", str(install_sh), "--yes"],
            cwd=checkout,
            env=env,
            text=True,
            capture_output=True,
            check=True,
        )
        output = result.stdout + result.stderr
        log_text = install_log.read_text(encoding="utf-8")
        assert "Refine upgraded to release 1.0.0" in output
        assert "Refresh target application" in output
        assert "uv --project" in log_text
        assert "run refine restart 8080" in log_text
        assert "run refine app rebuild --port 8080" in log_text
        assert "Skipped Playwright. Managed regression screenshots may fail" in output
        assert "+ npx --yes playwright install --with-deps chromium" not in output
        assert "Refine was upgraded but not restarted" not in output
        print("[ok] install.sh prompts for restart after a release upgrade")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
