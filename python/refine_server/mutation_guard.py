"""Cross-process guard for expensive Refine state mutations."""
from __future__ import annotations

import errno
import fcntl
import json
import os
import time
from contextlib import contextmanager
from typing import Any

from . import config
from .gaps import now_iso


class MutationBusy(RuntimeError):
    def __init__(self, owner: dict[str, Any] | None = None) -> None:
        self.owner = owner or {}
        super().__init__(
            f"Refine state mutation already active: "
            f"{self.owner.get('label') or self.owner.get('kind') or 'unknown'}",
        )


def active() -> dict[str, Any] | None:
    path = _lock_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(path, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as e:
            if e.errno not in (errno.EACCES, errno.EAGAIN):
                raise
            return _read_owner(fd)
        fcntl.flock(fd, fcntl.LOCK_UN)
        return None
    finally:
        os.close(fd)


@contextmanager
def exclusive(label: str, *, kind: str = "operation", blocking: bool = False):
    path = _lock_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(path, os.O_RDWR | os.O_CREAT, 0o600)
    owner = {
        "id": f"{kind}-{os.getpid()}-{time.monotonic_ns()}",
        "kind": kind,
        "label": label,
        "status": "running",
        "pid": os.getpid(),
        "started_at": now_iso(),
    }
    try:
        flags = fcntl.LOCK_EX if blocking else fcntl.LOCK_EX | fcntl.LOCK_NB
        try:
            fcntl.flock(fd, flags)
        except OSError as e:
            if e.errno not in (errno.EACCES, errno.EAGAIN):
                raise
            raise MutationBusy(_read_owner(fd)) from e
        _write_owner(fd, owner)
        try:
            yield owner
        finally:
            try:
                os.ftruncate(fd, 0)
                os.fsync(fd)
            finally:
                fcntl.flock(fd, fcntl.LOCK_UN)
    finally:
        os.close(fd)


def _lock_path():
    return config.local_run_dir() / "mutation.lock"


def _read_owner(fd: int) -> dict[str, Any]:
    try:
        os.lseek(fd, 0, os.SEEK_SET)
        raw = os.read(fd, 8192).decode("utf-8")
        data = json.loads(raw) if raw.strip() else {}
        return data if isinstance(data, dict) else {}
    except Exception:
        return {}


def _write_owner(fd: int, owner: dict[str, Any]) -> None:
    raw = json.dumps(owner, sort_keys=True).encode("utf-8")
    os.ftruncate(fd, 0)
    os.lseek(fd, 0, os.SEEK_SET)
    os.write(fd, raw)
    os.fsync(fd)
