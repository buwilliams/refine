"""Best-effort POSIX resource backend."""
from __future__ import annotations

import os
import resource
import subprocess

from refine_runtime.backends.base import LaunchSpec, ResourceBackend
from refine_runtime.resources import BackendCapabilities, memory_limit_mb, priority_to_nice


class PosixBackend(ResourceBackend):
    name = "posix"

    def capabilities(self) -> BackendCapabilities:
        return BackendCapabilities(
            name=self.name,
            isolation="best_effort",
            enforced=False,
            details="process groups, niceness, and RLIMIT_AS where available",
        )

    def popen(self, spec: LaunchSpec) -> subprocess.Popen:
        settings = spec.settings

        def preexec() -> None:
            os.setsid()
            nice = 0 if spec.kind == "ui" else priority_to_nice(settings.worker_cpu_priority)
            if nice:
                try:
                    os.nice(nice)
                except OSError:
                    pass
            memory_mb = memory_limit_mb(settings, spec.kind)
            if memory_mb:
                limit = memory_mb * 1024 * 1024
                try:
                    resource.setrlimit(resource.RLIMIT_AS, (limit, limit))
                except (OSError, ValueError):
                    pass

        return subprocess.Popen(
            list(spec.args),
            cwd=str(spec.cwd),
            env=dict(spec.env),
            stdin=spec.stdin,
            stdout=spec.stdout,
            stderr=spec.stderr,
            text=spec.text,
            bufsize=spec.bufsize,
            preexec_fn=preexec,
        )
