"""Git-synced cluster registry and SSH operations."""
from __future__ import annotations

import json
import re
import shlex
import subprocess
from pathlib import Path
from typing import Any

from . import config
from .gaps import now_iso


_VALID_NODE_ID = re.compile(r"^[a-z0-9][a-z0-9_-]{0,63}$")


def cluster_json_path(root: Path | None = None) -> Path:
    return (root or config.get().volume_root) / "cluster.json"


def read_cluster(*, root: Path | None = None) -> dict[str, Any]:
    path = cluster_json_path(root)
    if not path.exists():
        return {"nodes": [], "updated_at": ""}
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        data = {}
    nodes = data.get("nodes") if isinstance(data, dict) else []
    if not isinstance(nodes, list):
        nodes = []
    return {"nodes": [_normalize_node(n) for n in nodes if isinstance(n, dict)],
            "updated_at": str(data.get("updated_at") or "") if isinstance(data, dict) else ""}


def write_cluster(data: dict[str, Any], *, root: Path | None = None) -> dict[str, Any]:
    path = cluster_json_path(root)
    nodes = [_normalize_node(n) for n in data.get("nodes") or [] if isinstance(n, dict)]
    out = {"nodes": nodes, "updated_at": now_iso()}
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(out, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return out


def list_nodes(*, root: Path | None = None) -> list[dict[str, Any]]:
    return read_cluster(root=root)["nodes"]


def get_node(node_id: str, *, root: Path | None = None) -> dict[str, Any] | None:
    for node in list_nodes(root=root):
        if node.get("id") == node_id:
            return node
    return None


def upsert_node(body: dict[str, Any], *, root: Path | None = None) -> dict[str, Any]:
    node = _normalize_node(body)
    node_id = node["id"]
    if not _VALID_NODE_ID.match(node_id):
        raise ValueError("node id must start with a lowercase letter or digit and contain only lowercase letters, digits, _ or -")
    _validate_ssh_host(node["ssh_host"])
    _ensure_project_node(node, root=root)
    data = read_cluster(root=root)
    nodes = data["nodes"]
    now = now_iso()
    node["updated_at"] = now
    for idx, existing in enumerate(nodes):
        if existing.get("id") == node_id:
            node["created_at"] = existing.get("created_at") or now
            nodes[idx] = {**existing, **node}
            write_cluster({"nodes": nodes}, root=root)
            return nodes[idx]
    node["created_at"] = now
    nodes.append(node)
    write_cluster({"nodes": nodes}, root=root)
    return node


def update_node(node_id: str, body: dict[str, Any], *, root: Path | None = None) -> dict[str, Any]:
    data = read_cluster(root=root)
    nodes = data["nodes"]
    for idx, existing in enumerate(nodes):
        if existing.get("id") != node_id:
            continue
        merged = _normalize_node({**existing, **body, "id": node_id})
        _validate_ssh_host(merged["ssh_host"])
        _ensure_project_node(merged, root=root)
        merged["created_at"] = existing.get("created_at") or now_iso()
        merged["updated_at"] = now_iso()
        nodes[idx] = merged
        write_cluster({"nodes": nodes}, root=root)
        return merged
    raise ValueError(f"unknown cluster node: {node_id}")


def run_remote(node_id: str, refine_args: list[str], *,
               root: Path | None = None,
               timeout: float | None = 300.0) -> dict[str, Any]:
    node = get_node(node_id, root=root)
    if node is None:
        raise ValueError(f"unknown cluster node: {node_id}")
    if not str(node.get("target_app_path") or "").strip():
        raise ValueError("target_app_path is required before bootstrap")
    if not node.get("enabled", True):
        raise ValueError(f"cluster node is disabled: {node_id}")
    if not refine_args:
        raise ValueError("remote refine arguments are required")
    remote_cmd = _remote_refine_command(node, refine_args)
    cmd = _ssh_command(node, remote_cmd)
    result = subprocess.run(cmd, text=True, capture_output=True, timeout=timeout)
    return {
        "node_id": node_id,
        "command": cmd,
        "remote_command": remote_cmd,
        "exit_code": result.returncode,
        "stdout": result.stdout,
        "stderr": result.stderr,
        "ok": result.returncode == 0,
    }


def bootstrap(node_id: str, *, root: Path | None = None,
              timeout: float | None = 900.0) -> dict[str, Any]:
    node = get_node(node_id, root=root)
    if node is None:
        raise ValueError(f"unknown cluster node: {node_id}")
    if not str(node.get("target_app_path") or "").strip():
        raise ValueError("target_app_path is required before bootstrap")
    install = "curl -fsSL https://raw.githubusercontent.com/buwilliams/refine/main/scripts/install.sh | bash"
    checkout = _quote_remote_path(str(node.get("refine_checkout") or "~/refine"))
    target = _quote_remote_path(str(node.get("target_app_path") or ""))
    port = shlex.quote(str(node.get("refine_port") or 8080))
    remote_cmd = (
        f"{install} && cd {checkout} && "
        f"./r target {target} --force && "
        f"./r start {port}"
    )
    cmd = _ssh_command(node, remote_cmd)
    result = subprocess.run(cmd, text=True, capture_output=True, timeout=timeout)
    health = "ok" if result.returncode == 0 else "failed"
    update_node(node_id, {"health": {"status": health, "checked_at": now_iso()}}, root=root)
    return {
        "node_id": node_id,
        "command": cmd,
        "remote_command": remote_cmd,
        "exit_code": result.returncode,
        "stdout": result.stdout,
        "stderr": result.stderr,
        "ok": result.returncode == 0,
    }


def _ssh_command(node: dict[str, Any], remote_cmd: str) -> list[str]:
    cmd = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10"]
    port = int(node.get("ssh_port") or 22)
    if port != 22:
        cmd.extend(["-p", str(port)])
    cmd.extend([str(node["ssh_host"]), remote_cmd])
    return cmd


def _remote_refine_command(node: dict[str, Any], refine_args: list[str]) -> str:
    checkout = _quote_remote_path(str(node.get("refine_checkout") or "~/refine"))
    config_path = _quote_remote_path(
        str(Path(str(node.get("target_app_path") or "")) / ".refine" / "refine.toml")
    )
    args = " ".join(shlex.quote(str(arg)) for arg in refine_args)
    return f"cd {checkout} && ./r --config {config_path} {args}"


def _validate_ssh_host(host: str) -> None:
    if not host:
        raise ValueError("ssh_host is required")
    if "@" in host:
        raise ValueError("ssh_host must not include a user; the current host user is assumed")
    if host.startswith("-"):
        raise ValueError("ssh_host must not start with '-'")


def _quote_remote_path(value: str) -> str:
    if value == "~":
        return "~"
    if value.startswith("~/"):
        return "~/" + shlex.quote(value[2:])
    return shlex.quote(value)


def _normalize_node(raw: dict[str, Any]) -> dict[str, Any]:
    node_id = str(raw.get("id") or "").strip().lower()
    out = {
        "id": node_id,
        "display_name": str(raw.get("display_name") or node_id).strip() or node_id,
        "ssh_host": str(raw.get("ssh_host") or "").strip(),
        "ssh_port": int(raw.get("ssh_port") or 22),
        "refine_checkout": str(raw.get("refine_checkout") or "~/refine").strip(),
        "target_app_path": str(raw.get("target_app_path") or "").strip(),
        "refine_port": int(raw.get("refine_port") or 8080),
        "enabled": bool(raw.get("enabled", True)),
        "health": raw.get("health") if isinstance(raw.get("health"), dict) else {},
        "created_at": str(raw.get("created_at") or ""),
        "updated_at": str(raw.get("updated_at") or ""),
    }
    if not out["id"]:
        raise ValueError("id is required")
    return out


def _ensure_project_node(node: dict[str, Any], *, root: Path | None = None) -> None:
    try:
        from . import project_state

        if project_state.node_by_id(node["id"], root=root) is None:
            project_state.create_node(
                node.get("display_name") or node["id"],
                node_id=node["id"],
                root=root,
            )
        else:
            project_state.update_node(
                node["id"],
                display_name=node.get("display_name") or node["id"],
                archived=not bool(node.get("enabled", True)),
                root=root,
            )
    except Exception:
        pass
