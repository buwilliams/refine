"""macOS best-effort launchd backend placeholder."""
from __future__ import annotations

from refine_runtime.backends.posix import PosixBackend
from refine_runtime.resources import BackendCapabilities


class LaunchdBackend(PosixBackend):
    name = "launchd"

    def capabilities(self) -> BackendCapabilities:
        return BackendCapabilities(
            name=self.name,
            isolation="best_effort",
            enforced=False,
            details="launchd/process-group controls are best effort",
        )
