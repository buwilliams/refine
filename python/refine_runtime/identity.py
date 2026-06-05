"""Runtime identity shared by the UI process and runner worker."""
from __future__ import annotations

import hashlib
import importlib.metadata
import os
import time
from pathlib import Path


def package_version() -> str:
    try:
        return importlib.metadata.version("refine")
    except Exception:
        return "unknown"


def source_fingerprint() -> str:
    root = Path(__file__).resolve().parents[1]
    digest = hashlib.sha256()
    for package in ("refine_runtime", "refine_server", "refine_ui"):
        package_root = root / package
        if not package_root.exists():
            continue
        for path in sorted(package_root.rglob("*.py")):
            if "__pycache__" in path.parts:
                continue
            rel = path.relative_to(root).as_posix()
            digest.update(rel.encode("utf-8"))
            try:
                st = path.stat()
                digest.update(str(st.st_size).encode("ascii"))
                digest.update(str(st.st_mtime_ns).encode("ascii"))
            except OSError:
                continue
    return digest.hexdigest()[:16]


REFINE_VERSION = package_version()
SOURCE_FINGERPRINT = source_fingerprint()
PROCESS_STARTED_AT = time.time()
