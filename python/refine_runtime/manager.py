"""Central process/resource manager used by Refine heavy work paths."""
from __future__ import annotations

import platform
import subprocess
from pathlib import Path
from typing import Mapping, Sequence

from refine_runtime.backends.base import LaunchSpec, ResourceBackend
from refine_runtime.backends.launchd import LaunchdBackend
from refine_runtime.backends.posix import PosixBackend
from refine_runtime.backends.systemd import SystemdBackend
from refine_runtime.resources import BackendCapabilities, ResourceSettings, detect_capabilities


class ResourceManager:
    def __init__(self, settings: ResourceSettings | None = None,
                 backend: ResourceBackend | None = None) -> None:
        self.settings = settings or ResourceSettings()
        self.backend = backend or self._select_backend()

    def capabilities(self) -> BackendCapabilities:
        return self.backend.capabilities()

    def popen(
        self,
        args: Sequence[str],
        *,
        cwd: Path,
        env: Mapping[str, str],
        kind: str = "worker",
        stdin: object | None = subprocess.DEVNULL,
        stdout: object | None = subprocess.PIPE,
        stderr: object | None = subprocess.STDOUT,
        text: bool = True,
        bufsize: int = 1,
    ) -> subprocess.Popen:
        spec = LaunchSpec(
            args=args,
            cwd=cwd,
            env=env,
            kind=kind,
            stdin=stdin,
            stdout=stdout,
            stderr=stderr,
            text=text,
            bufsize=bufsize,
            settings=self.settings,
        )
        return self.backend.popen(spec)

    def _select_backend(self) -> ResourceBackend:
        system = platform.system().lower()
        caps = detect_capabilities(self.settings.resource_isolation_mode)
        if self.settings.resource_isolation_mode == "enforced" and not caps.enforced:
            raise RuntimeError(caps.details or "enforced resource isolation is unavailable")
        if system == "linux" and caps.enforced:
            return SystemdBackend()
        if system == "darwin":
            return LaunchdBackend()
        return PosixBackend()
