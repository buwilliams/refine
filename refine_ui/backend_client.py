"""Backend client used by HTTP handlers.

The runner is reached through the local supervisor runtime's Unix socket.
"""
from __future__ import annotations

from refine_runtime.ipc import IpcError


class BackendError(Exception):
    def __init__(self, code: str, message: str, details: str | None = None) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.details = details


class BackendClient:
    def call(self, method: str, params: dict | None = None, *, timeout: float = 30.0) -> dict:  # noqa: ARG002
        from . import runtime
        from refine_server import config

        try:
            return runtime.runner_call(method, params or {}, timeout=timeout)
        except config.ConfigError as e:
            raise BackendError("backend_unavailable", str(e)) from e
        except IpcError as e:
            raise BackendError(e.code, e.message, e.details) from e
        except KeyError as e:
            raise BackendError("unknown_method", str(e)) from e
        except ValueError as e:
            if "owned by another instance" in str(e):
                raise BackendError("instance_ownership", str(e)) from e
            raise BackendError("bad_request", str(e)) from e
        except Exception as e:
            raise BackendError("internal", repr(e)) from e

    def ping(self) -> dict:
        return self.call("ping")

    def is_reachable(self) -> bool:
        try:
            self.ping()
            return True
        except BackendError:
            return False


_singleton: BackendClient | None = None


def get_client() -> BackendClient:
    global _singleton
    if _singleton is None:
        _singleton = BackendClient()
    return _singleton
