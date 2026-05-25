"""Managed Playwright regressions for target-application QA."""
from __future__ import annotations

import base64
import json
import re
import shutil
import subprocess
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from refine_server import config, db, target_app
from refine_server.gaps import now_iso
from refine_server.ulid import new_ulid


SCHEMA_VERSION = 1
MANIFEST_FILENAME = "manifest.json"
DEFAULT_TIMEOUT_SECONDS = 120
DEFAULT_VIEWPORT = {"width": 1440, "height": 900}
_VALID_ID = re.compile(r"^[a-z0-9][a-z0-9_-]{1,63}$")
_SAFE_SPEC = re.compile(r"^specs/[a-z0-9][a-z0-9_-]{1,63}\.js$")
_DATA_URL_LIMIT = 1_500_000


def regressions_dir(root: Path | None = None) -> Path:
    return (root or config.get().volume_root) / "regressions"


def manifest_path(root: Path | None = None) -> Path:
    return regressions_dir(root) / MANIFEST_FILENAME


def specs_dir(root: Path | None = None) -> Path:
    return regressions_dir(root) / "specs"


def runs_dir(root: Path | None = None) -> Path:
    return regressions_dir(root) / "runs"


def load_manifest(root: Path | None = None) -> dict[str, Any]:
    path = manifest_path(root)
    if not path.exists():
        return {"schema_version": SCHEMA_VERSION, "regressions": []}
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        return {"schema_version": SCHEMA_VERSION, "regressions": []}
    regs = raw.get("regressions") if isinstance(raw, dict) else []
    if not isinstance(regs, list):
        regs = []
    clean = []
    for item in regs:
        normalized = normalize_regression(item)
        if normalized:
            clean.append(normalized)
    return {"schema_version": SCHEMA_VERSION, "regressions": clean}


def save_manifest(manifest: dict[str, Any], root: Path | None = None) -> None:
    directory = regressions_dir(root)
    directory.mkdir(parents=True, exist_ok=True)
    path = manifest_path(root)
    payload = {
        "schema_version": SCHEMA_VERSION,
        "regressions": [
            item for item in (
                normalize_regression(reg) for reg in manifest.get("regressions", [])
            )
            if item
        ],
    }
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def normalize_regression(item: Any) -> dict[str, Any] | None:
    if not isinstance(item, dict):
        return None
    rid = str(item.get("id") or "").strip().lower()
    if not _VALID_ID.match(rid):
        return None
    spec_path = str(item.get("spec_path") or f"specs/{rid}.js").strip()
    if not _SAFE_SPEC.match(spec_path):
        return None
    viewport = item.get("viewport") if isinstance(item.get("viewport"), dict) else {}
    width = _positive_int(viewport.get("width"), DEFAULT_VIEWPORT["width"])
    height = _positive_int(viewport.get("height"), DEFAULT_VIEWPORT["height"])
    return {
        "id": rid,
        "title": str(item.get("title") or "Untitled regression").strip()[:160],
        "description": str(item.get("description") or "").strip(),
        "enabled": bool(item.get("enabled", True)),
        "spec_path": spec_path,
        "viewport": {"width": width, "height": height},
        "wait_until": str(item.get("wait_until") or "networkidle").strip() or "networkidle",
        "timeout_seconds": _positive_int(
            item.get("timeout_seconds"), DEFAULT_TIMEOUT_SECONDS,
        ),
        "created_at": str(item.get("created_at") or now_iso()),
        "updated_at": str(item.get("updated_at") or now_iso()),
    }


def list_regressions(root: Path | None = None, *, include_latest: bool = True) -> list[dict[str, Any]]:
    regs = load_manifest(root).get("regressions", [])
    if include_latest:
        for reg in regs:
            reg["latest_run"] = latest_run(reg["id"], root=root)
    return regs


def create_regression(
    *,
    title: str,
    description: str = "",
    prompt: str = "",
    root: Path | None = None,
) -> dict[str, Any]:
    title = str(title or "").strip() or "Untitled regression"
    root = root or config.get().volume_root
    manifest = load_manifest(root)
    rid = _new_regression_id(title, {r["id"] for r in manifest["regressions"]})
    now = now_iso()
    reg = {
        "id": rid,
        "title": title[:160],
        "description": str(description or prompt or "").strip(),
        "enabled": True,
        "spec_path": f"specs/{rid}.js",
        "viewport": dict(DEFAULT_VIEWPORT),
        "wait_until": "networkidle",
        "timeout_seconds": DEFAULT_TIMEOUT_SECONDS,
        "created_at": now,
        "updated_at": now,
    }
    specs_dir(root).mkdir(parents=True, exist_ok=True)
    spec_file(root, reg).write_text(_spec_template(reg, prompt=prompt), encoding="utf-8")
    manifest["regressions"].append(reg)
    save_manifest(manifest, root)
    return reg


def update_regression(
    regression_id: str,
    updates: dict[str, Any],
    *,
    root: Path | None = None,
) -> dict[str, Any] | None:
    manifest = load_manifest(root)
    for idx, reg in enumerate(manifest["regressions"]):
        if reg["id"] != regression_id:
            continue
        next_reg = dict(reg)
        for key in ("title", "description", "enabled", "viewport", "wait_until", "timeout_seconds"):
            if key in updates:
                next_reg[key] = updates[key]
        next_reg["updated_at"] = now_iso()
        normalized = normalize_regression(next_reg)
        if not normalized:
            return None
        manifest["regressions"][idx] = normalized
        save_manifest(manifest, root)
        return normalized
    return None


def delete_regression(regression_id: str, *, root: Path | None = None) -> bool:
    root = root or config.get().volume_root
    manifest = load_manifest(root)
    kept = [r for r in manifest["regressions"] if r["id"] != regression_id]
    if len(kept) == len(manifest["regressions"]):
        return False
    manifest["regressions"] = kept
    save_manifest(manifest, root)
    spec = specs_dir(root) / f"{regression_id}.js"
    try:
        spec.unlink()
    except FileNotFoundError:
        pass
    return True


def enabled(conn) -> bool:
    return (db.get_setting(conn, "quality_regressions_enabled", "0") or "0") == "1"


def set_enabled(conn, value: Any) -> str:
    enabled_value = "1" if str(value).strip().lower() in {"1", "true", "yes", "on"} else "0"
    db.set_setting(conn, "quality_regressions_enabled", enabled_value)
    return enabled_value


def summarize_for_prompt(result: dict[str, Any]) -> str:
    if not result.get("enabled"):
        return "Regression checks are disabled."
    runs = result.get("runs") or []
    if not runs:
        return result.get("message") or "No regression checks are configured."
    lines = [
        "Managed Playwright regression results:",
        f"- overall: {'passed' if result.get('ok') else 'failed'}",
    ]
    for run in runs:
        status = "passed" if run.get("ok") else "failed"
        lines.append(f"- {run.get('title') or run.get('id')}: {status}")
        if run.get("message"):
            lines.append(f"  message: {run['message']}")
        if run.get("screenshot_path"):
            lines.append(f"  screenshot: {run['screenshot_path']}")
        if run.get("summary_path"):
            lines.append(f"  result: {run['summary_path']}")
    if result.get("infra"):
        lines.append("Treat missing tools, broken selectors, and invalid regression specs as regression infrastructure problems. Repair regression files when possible; if they cannot be repaired, exit with failure.")
    else:
        lines.append("If screenshots or regression failures show product behavior is wrong, exit with failure. If only a regression spec is stale or broken, repair it and rerun the relevant check.")
    return "\n".join(lines)


def run_all(
    conn,
    *,
    root: Path | None = None,
    target_root: Path | None = None,
    only_enabled: bool = True,
) -> dict[str, Any]:
    root = root or config.get().volume_root
    regs = list_regressions(root, include_latest=False)
    if only_enabled:
        regs = [r for r in regs if r.get("enabled")]
    if not regs:
        return {"enabled": True, "ok": True, "infra": False, "runs": [], "message": "No regression checks configured."}

    settings = db.list_settings(conn)
    app_url = (settings.get("target_app_url") or "").strip()
    cfg = target_app.config_from_settings(settings)
    if target_root is not None:
        cfg["root"] = str(target_root)
    if not app_url:
        return _infra_result("No target_app_url configured for regression checks.", regs)
    if not (cfg.get("start_command") or "").strip():
        return _infra_result("No target-app start command configured for regression checks.", regs)
    if shutil.which("npx") is None:
        return _infra_result("npx is not available; install Node.js/npm and Playwright.", regs)

    start = target_app.run_operation("start", cfg)
    if not start.get("ok"):
        return _infra_result(
            "Target application could not be started for regression checks: "
            + str(start.get("message") or "start failed"),
            regs,
        )
    runs: list[dict[str, Any]] = []
    try:
        for reg in regs:
            runs.append(run_one(reg, app_url=app_url, root=root, target_root=target_root))
    finally:
        if (cfg.get("stop_command") or "").strip():
            target_app.run_operation("stop", cfg)
    ok = all(r.get("ok") for r in runs)
    return {
        "enabled": True,
        "ok": ok,
        "infra": any(r.get("infra") for r in runs),
        "runs": runs,
        "message": f"{sum(1 for r in runs if r.get('ok'))}/{len(runs)} regression checks passed",
    }


def run_one(
    reg: dict[str, Any],
    *,
    app_url: str,
    root: Path | None = None,
    target_root: Path | None = None,
) -> dict[str, Any]:
    root = root or config.get().volume_root
    reg = normalize_regression(reg) or reg
    run_id = new_ulid().lower()
    out_dir = runs_dir(root) / reg["id"] / run_id
    out_dir.mkdir(parents=True, exist_ok=True)
    screenshot = out_dir / "screenshot.png"
    json_report = out_dir / "playwright-report.json"
    summary = out_dir / "summary.json"
    cfg_path = out_dir / "playwright.config.cjs"
    cfg_path.write_text(_playwright_config(reg, json_report), encoding="utf-8")
    spec = spec_file(root, reg)
    env = target_app._command_env({})  # noqa: SLF001
    env.update({
        "REFINE_TARGET_APP_URL": app_url,
        "REFINE_REGRESSION_SCREENSHOT": str(screenshot),
        "REFINE_REGRESSION_TITLE": reg.get("title") or reg["id"],
    })
    started = _utc_now()
    cmd = [
        "npx", "--yes", "playwright", "test", str(spec),
        "--config", str(cfg_path),
    ]
    try:
        proc = subprocess.run(
            cmd,
            cwd=str(target_root or target_app.resolve_cwd("")),
            env=env,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=int(reg.get("timeout_seconds") or DEFAULT_TIMEOUT_SECONDS),
        )
        ok = proc.returncode == 0 and screenshot.exists()
        message = "passed" if ok else (
            _last_line(proc.stderr) or _last_line(proc.stdout) or f"playwright exited {proc.returncode}"
        )
        payload = {
            "id": reg["id"],
            "title": reg.get("title") or reg["id"],
            "run_id": run_id,
            "ok": ok,
            "infra": not spec.exists() or not screenshot.exists(),
            "message": message,
            "started_at": started,
            "finished_at": _utc_now(),
            "command": " ".join(cmd),
            "screenshot_path": str(screenshot) if screenshot.exists() else "",
            "summary_path": str(summary),
            "json_report_path": str(json_report) if json_report.exists() else "",
            "stdout_tail": _tail(proc.stdout or ""),
            "stderr_tail": _tail(proc.stderr or ""),
        }
    except subprocess.TimeoutExpired as e:
        payload = {
            "id": reg["id"],
            "title": reg.get("title") or reg["id"],
            "run_id": run_id,
            "ok": False,
            "infra": True,
            "message": f"playwright timed out after {reg.get('timeout_seconds') or DEFAULT_TIMEOUT_SECONDS}s",
            "started_at": started,
            "finished_at": _utc_now(),
            "command": " ".join(cmd),
            "screenshot_path": str(screenshot) if screenshot.exists() else "",
            "summary_path": str(summary),
            "stdout_tail": _tail(e.stdout or ""),
            "stderr_tail": _tail(e.stderr or ""),
        }
    summary.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return payload


def latest_run(regression_id: str, *, root: Path | None = None) -> dict[str, Any] | None:
    base = runs_dir(root) / regression_id
    if not base.exists():
        return None
    candidates = sorted(base.glob("*/summary.json"), key=lambda p: p.stat().st_mtime, reverse=True)
    if not candidates:
        return None
    try:
        payload = json.loads(candidates[0].read_text(encoding="utf-8"))
    except Exception:
        return None
    screenshot = payload.get("screenshot_path") or ""
    if screenshot:
        data_url = _screenshot_data_url(Path(screenshot))
        if data_url:
            payload["screenshot_data_url"] = data_url
    return payload


def spec_file(root: Path, reg: dict[str, Any]) -> Path:
    return (root / "regressions" / reg["spec_path"]).resolve()


def _new_regression_id(title: str, existing: set[str]) -> str:
    slug = re.sub(r"[^a-z0-9]+", "-", title.lower()).strip("-")[:36] or "regression"
    candidate = slug
    suffix = 1
    while candidate in existing or not _VALID_ID.match(candidate):
        suffix += 1
        candidate = f"{slug}-{suffix}"
    return candidate


def _spec_template(reg: dict[str, Any], *, prompt: str = "") -> str:
    wait_until = reg.get("wait_until") or "networkidle"
    title = json.dumps(reg.get("title") or reg["id"])
    note = f"// Initial prompt: {prompt.strip()}\n" if prompt.strip() else ""
    return f"""const {{ test, expect }} = require("@playwright/test");

{note}test({title}, async ({{ page }}) => {{
  const targetUrl = process.env.REFINE_TARGET_APP_URL;
  const screenshotPath = process.env.REFINE_REGRESSION_SCREENSHOT;
  if (!targetUrl) throw new Error("REFINE_TARGET_APP_URL is required");
  if (!screenshotPath) throw new Error("REFINE_REGRESSION_SCREENSHOT is required");

  await page.goto(targetUrl, {{ waitUntil: {json.dumps(wait_until)} }});
  await expect(page.locator("body")).toBeVisible();
  await page.screenshot({{ path: screenshotPath, fullPage: true }});
}});
"""


def _playwright_config(reg: dict[str, Any], json_report: Path) -> str:
    viewport = reg.get("viewport") or DEFAULT_VIEWPORT
    timeout_ms = int(reg.get("timeout_seconds") or DEFAULT_TIMEOUT_SECONDS) * 1000
    return f"""module.exports = {{
  timeout: {timeout_ms},
  reporter: [["json", {{ outputFile: {json.dumps(str(json_report))} }}]],
  use: {{
    browserName: "chromium",
    headless: true,
    viewport: {{
      width: {int(viewport.get("width") or DEFAULT_VIEWPORT["width"])},
      height: {int(viewport.get("height") or DEFAULT_VIEWPORT["height"])},
    }},
  }},
}};
"""


def _infra_result(message: str, regs: list[dict[str, Any]]) -> dict[str, Any]:
    return {
        "enabled": True,
        "ok": False,
        "infra": True,
        "message": message,
        "runs": [
            {
                "id": r["id"],
                "title": r.get("title") or r["id"],
                "ok": False,
                "infra": True,
                "message": message,
                "screenshot_path": "",
                "summary_path": "",
            }
            for r in regs
        ],
    }


def _positive_int(value: Any, default: int) -> int:
    try:
        n = int(value)
    except (TypeError, ValueError):
        return default
    return n if n > 0 else default


def _tail(text: str, limit: int = 8000) -> str:
    return text[-limit:] if len(text) > limit else text


def _last_line(text: str) -> str:
    for line in reversed(str(text or "").splitlines()):
        stripped = line.strip()
        if stripped:
            return stripped[:500]
    return ""


def _utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


def _screenshot_data_url(path: Path) -> str:
    try:
        if not path.is_file() or path.stat().st_size > _DATA_URL_LIMIT:
            return ""
        data = base64.b64encode(path.read_bytes()).decode("ascii")
    except OSError:
        return ""
    return f"data:image/png;base64,{data}"
