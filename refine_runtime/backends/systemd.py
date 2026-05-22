"""Linux systemd resource backend helpers."""
from __future__ import annotations

import shutil
import subprocess

from refine_runtime.backends.base import LaunchSpec, ResourceBackend
from refine_runtime.resources import (
    BackendCapabilities,
    memory_limit_mb,
    priority_to_cpu_weight,
)


class SystemdBackend(ResourceBackend):
    name = "systemd"

    def capabilities(self) -> BackendCapabilities:
        if shutil.which("systemd-run"):
            return BackendCapabilities(
                name=self.name,
                isolation="enforced",
                enforced=True,
                details="systemd-run available",
            )
        return BackendCapabilities(
            name=self.name,
            isolation="unavailable",
            enforced=False,
            details="systemd-run not found",
        )

    def command(self, spec: LaunchSpec) -> list[str]:
        settings = spec.settings
        cmd = [
            "systemd-run",
            "--user",
            "--scope",
            "--quiet",
            "--same-dir",
            "-p",
            "CPUWeight=100" if spec.kind == "ui"
            else f"CPUWeight={priority_to_cpu_weight(settings.worker_cpu_priority)}",
        ]
        memory_mb = memory_limit_mb(settings, spec.kind)
        if memory_mb:
            cmd.extend(["-p", f"MemoryMax={memory_mb}M"])
        cmd.extend(["--", *list(spec.args)])
        return cmd

    def popen(self, spec: LaunchSpec) -> subprocess.Popen:
        return subprocess.Popen(
            self.command(spec),
            cwd=str(spec.cwd),
            env=dict(spec.env),
            stdin=spec.stdin,
            stdout=spec.stdout,
            stderr=spec.stderr,
            text=spec.text,
            bufsize=spec.bufsize,
            start_new_session=True,
        )
