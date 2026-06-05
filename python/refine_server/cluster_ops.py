"""Shared cluster registry and SSH operations for API and CLI."""
from __future__ import annotations

import subprocess
from typing import Any, Callable

from . import cluster


SyncCall = Callable[[str], dict[str, Any]]


def list_cluster() -> tuple[int, dict[str, Any]]:
    return 200, cluster.read_cluster()


def upsert_node(
    body: dict[str, Any],
    sync_call: SyncCall,
) -> tuple[int, dict[str, Any]]:
    try:
        node = cluster.upsert_node(body)
    except ValueError as e:
        return _err(400, str(e))
    sync_err, sync = _sync(sync_call, "refine: update cluster node")
    if sync_err is not None:
        return sync_err
    return 200, {"node": node, "sync": sync, **cluster.read_cluster()}


def update_node(
    node_id: str,
    body: dict[str, Any],
    sync_call: SyncCall,
) -> tuple[int, dict[str, Any]]:
    try:
        node = cluster.update_node(node_id, body)
    except ValueError as e:
        return _err(400, str(e))
    sync_err, sync = _sync(sync_call, "refine: update cluster node")
    if sync_err is not None:
        return sync_err
    return 200, {"node": node, "sync": sync, **cluster.read_cluster()}


def run_node(node_id: str, body: dict[str, Any]) -> tuple[int, dict[str, Any]]:
    args = body.get("args")
    if not isinstance(args, list):
        return _err(400, "args must be a list")
    try:
        result = cluster.run_remote(node_id, [str(arg) for arg in args])
    except (ValueError, subprocess.SubprocessError, OSError) as e:
        return _err(400, str(e))
    return (200 if result.get("ok") else 502), result


def bootstrap_node(
    node_id: str,
    sync_call: SyncCall,
) -> tuple[int, dict[str, Any]]:
    try:
        result = cluster.bootstrap(node_id)
    except (ValueError, subprocess.SubprocessError, OSError) as e:
        return _err(400, str(e))
    sync_err, sync = _sync(sync_call, "refine: update cluster node health")
    if sync_err is not None:
        return sync_err
    result["sync"] = sync
    return (200 if result.get("ok") else 502), result


def _sync(
    sync_call: SyncCall,
    message: str,
) -> tuple[tuple[int, dict[str, Any]] | None, dict[str, Any]]:
    sync = sync_call(message)
    if sync.get("ok"):
        return None, sync
    return _err(
        409,
        (
            "Could not sync cluster node health."
            if "health" in message else
            "Could not sync cluster node state."
        ),
        str(sync.get("details") or sync.get("message") or ""),
    ), sync


def _err(
    code: int,
    message: str,
    details: str | None = None,
) -> tuple[int, dict[str, Any]]:
    body: dict[str, Any] = {"error": {"message": message}}
    if details:
        body["error"]["details"] = details
    return code, body
