"""Base resource backend contract."""
from __future__ import annotations

import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping, Sequence

from refine_runtime.resources import BackendCapabilities, ResourceSettings


@dataclass
class LaunchSpec:
    args: Sequence[str]
    cwd: Path
    env: Mapping[str, str]
    kind: str = "worker"
    stdout: object | None = subprocess.PIPE
    stderr: object | None = subprocess.STDOUT
    stdin: object | None = subprocess.DEVNULL
    text: bool = True
    bufsize: int = 1
    settings: ResourceSettings = ResourceSettings()


class ResourceBackend:
    name = "base"

    def capabilities(self) -> BackendCapabilities:
        return BackendCapabilities(self.name, "best_effort", False)

    def popen(self, spec: LaunchSpec) -> subprocess.Popen:
        raise NotImplementedError
