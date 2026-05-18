"""HTTP server (stdlib only). Routes JSON API + serves static files + SSE."""
from __future__ import annotations

import json
import os
import re
import sys
import time
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Callable
from urllib.parse import parse_qs, urlparse

from . import api, sse


STATIC_DIR = Path(__file__).parent / "static"

# Route table: (METHOD, regex pattern) -> handler(handler_self, match, body, query)
Handler = Callable[[Any, "re.Match", dict | None, dict], tuple[int, Any]]
ROUTES: list[tuple[str, re.Pattern, Handler]] = []


def route(method: str, pattern: str) -> Callable[[Handler], Handler]:
    def deco(fn: Handler) -> Handler:
        ROUTES.append((method.upper(), re.compile(f"^{pattern}$"), fn))
        return fn
    return deco


# ---- API handlers (thin wrappers around api.py) ------------------------------


@route("GET", r"/api/dashboard")
def _h_dashboard(_h, _m, _b, _q):
    return api.dashboard_summary()


@route("GET", r"/api/gaps")
def _h_list_gaps(_h, _m, _b, q):
    facets = _get_one(q, "facets")
    return api.list_gaps(
        status=_get_one(q, "status"),
        q=_get_one(q, "q"),
        severity=_get_one(q, "severity"),
        category=_get_one(q, "category"),
        actor=_get_one(q, "actor"),
        reporter=_get_one(q, "reporter"),
        limit=int(_get_one(q, "limit", "200")),
        offset=int(_get_one(q, "offset", "0")),
        sort=_get_one(q, "sort"),
        direction=_get_one(q, "dir"),
        include_facets=bool(facets and facets != "0"),
    )


@route("POST", r"/api/gaps")
def _h_create_gap(_h, _m, body, _q):
    return api.create_gap(body or {})


@route("POST", r"/api/gaps/bulk")
def _h_bulk_update_gaps(_h, _m, body, _q):
    return api.bulk_update_gaps(body or {})


@route("POST", r"/api/gaps/bulk/delete")
def _h_bulk_delete_gaps(_h, _m, body, _q):
    return api.bulk_delete_gaps(body or {})


@route("GET", r"/api/gaps/([0-9A-Za-z]{26})")
def _h_get_gap(_h, m, _b, _q):
    return api.get_gap(m.group(1).upper())


@route("PATCH", r"/api/gaps/([0-9A-Za-z]{26})")
def _h_update_gap(_h, m, body, _q):
    return api.update_gap_name(m.group(1).upper(), body or {})


@route("DELETE", r"/api/gaps/([0-9A-Za-z]{26})")
def _h_delete_gap(_h, m, _b, _q):
    return api.delete_gap(m.group(1).upper())


@route("POST", r"/api/gaps/([0-9A-Za-z]{26})/rounds")
def _h_append_round(_h, m, body, _q):
    return api.append_round(m.group(1).upper(), body or {})


@route("PATCH", r"/api/gaps/([0-9A-Za-z]{26})/rounds/latest")
def _h_edit_round(_h, m, body, _q):
    return api.edit_latest_round(m.group(1).upper(), body or {})


@route("POST", r"/api/gaps/([0-9A-Za-z]{26})/verify")
def _h_verify(_h, m, _b, _q):
    return api.verify(m.group(1).upper())


@route("POST", r"/api/gaps/([0-9A-Za-z]{26})/retry")
def _h_retry(_h, m, _b, _q):
    return api.retry(m.group(1).upper())


@route("POST", r"/api/gaps/([0-9A-Za-z]{26})/cancel")
def _h_cancel(_h, m, _b, _q):
    return api.cancel(m.group(1).upper())


@route("GET", r"/api/changes")
def _h_list_changes(_h, _m, _b, q):
    return api.list_changes(
        limit=int(_get_one(q, "limit", "50")),
        offset=int(_get_one(q, "offset", "0")),
        q=_get_one(q, "q"),
        status=_get_one(q, "status"),
        priority=_get_one(q, "priority"),
    )


@route("POST", r"/api/changes/undo")
def _h_undo_change(_h, _m, body, _q):
    return api.undo_change(body or {})


@route("GET", r"/api/reporters")
def _h_list_reporters(_h, _m, _b, _q):
    return api.list_reporters()


@route("POST", r"/api/reporters")
def _h_create_reporter(_h, _m, body, _q):
    return api.create_reporter(body or {})


@route("PATCH", r"/api/reporters/(\d+)")
def _h_rename_reporter(_h, m, body, _q):
    return api.rename_reporter(int(m.group(1)), body or {})


@route("DELETE", r"/api/reporters/(\d+)")
def _h_delete_reporter(_h, m, _b, _q):
    return api.delete_reporter(int(m.group(1)))


@route("GET", r"/api/settings")
def _h_get_settings(_h, _m, _b, _q):
    return api.list_settings()


@route("PATCH", r"/api/settings")
def _h_patch_settings(_h, _m, body, _q):
    return api.update_settings(body or {})


@route("GET", r"/api/governance")
def _h_governance_get(_h, _m, _b, _q):
    return api.governance_get()


@route("PATCH", r"/api/governance")
def _h_governance_save(_h, _m, body, _q):
    return api.governance_save(body or {})


@route("POST", r"/api/governance/generate-rules")
def _h_governance_generate_rules(_h, _m, body, _q):
    return api.governance_generate_rules(body or {})


@route("POST", r"/api/settings/recheck-auth")
def _h_recheck(_h, _m, _b, _q):
    return api.recheck_auth()


@route("GET", r"/api/features")
def _h_get_features(_h, _m, _b, _q):
    return api.list_features()


@route("POST", r"/api/features/override")
def _h_set_feature_override(_h, _m, body, _q):
    return api.set_feature_override(body or {})


@route("GET", r"/api/diagnostics")
def _h_diag(_h, _m, _b, _q):
    return api.backend_diagnostics()


@route("GET", r"/api/activity")
def _h_activity(_h, _m, _b, q):
    sid = _get_one(q, "since_id")
    since = int(sid) if sid else None
    facets = _get_one(q, "facets")
    return api.list_activity(
        limit=int(_get_one(q, "limit", "100")),
        gap_id=_get_one(q, "gap_id"),
        since_id=since,
        severity=_get_one(q, "severity"),
        category=_get_one(q, "category"),
        actor=_get_one(q, "actor"),
        q=_get_one(q, "q"),
        offset=int(_get_one(q, "offset", "0")),
        include_facets=bool(facets and facets != "0"),
    )


@route("POST", r"/api/activity/cleanup")
def _h_activity_cleanup(_h, _m, body, _q):
    return api.cleanup_logs(body or {})


@route("POST", r"/api/import/extract")
def _h_import_extract(_h, _m, body, _q):
    return api.import_extract(body or {})


@route("POST", r"/api/import/persist")
def _h_import_persist(_h, _m, body, _q):
    return api.import_persist(body or {})


@route("POST", r"/api/chat/start")
def _h_chat_start(_h, _m, body, _q):
    return api.chat_start(body or {})


@route("POST", r"/api/chat/([0-9A-Za-z]+)/input")
def _h_chat_input(_h, m, body, _q):
    return api.chat_input(m.group(1), body or {})


@route("GET", r"/api/chat/([0-9A-Za-z]+)/read")
def _h_chat_read(_h, m, _b, _q):
    return api.chat_read(m.group(1))


@route("POST", r"/api/chat/([0-9A-Za-z]+)/stop")
def _h_chat_stop(_h, m, _b, _q):
    return api.chat_stop(m.group(1))


# ---- Target application ------------------------------------------------------


@route("GET", r"/api/project/status")
def _h_project_status(_h, _m, _b, _q):
    return api.project_status()


@route("GET", r"/api/projects")
def _h_project_list(_h, _m, _b, _q):
    return api.project_list()


@route("POST", r"/api/project/attach")
def _h_project_attach(_h, _m, body, _q):
    return api.project_attach(body or {})


@route("DELETE", r"/api/projects")
def _h_project_remove(_h, _m, body, _q):
    return api.project_remove(body or {})


@route("GET", r"/api/target-app/status")
def _h_target_app_status(_h, _m, _b, _q):
    return api.target_app_status()


@route("POST", r"/api/target-app/start")
def _h_target_app_start(_h, _m, body, _q):
    return api.target_app_start(body or {})


@route("POST", r"/api/target-app/stop")
def _h_target_app_stop(_h, _m, body, _q):
    return api.target_app_stop(body or {})


@route("POST", r"/api/target-app/health")
def _h_target_app_health(_h, _m, body, _q):
    return api.target_app_health(body or {})


@route("POST", r"/api/target-app/generate-instructions")
def _h_target_app_generate(_h, _m, body, _q):
    return api.target_app_generate(body or {})


# ---- helpers -----------------------------------------------------------------

def _get_one(q: dict, key: str, default: str | None = None) -> str | None:
    v = q.get(key)
    if not v:
        return default
    return v[0] if isinstance(v, list) else v


def _read_body(handler: BaseHTTPRequestHandler) -> dict | None:
    length = int(handler.headers.get("Content-Length", "0") or "0")
    if length <= 0:
        return None
    raw = handler.rfile.read(length)
    if not raw:
        return None
    try:
        return json.loads(raw.decode("utf-8"))
    except Exception:
        return None


# ---- handler -----------------------------------------------------------------


class RefineHandler(BaseHTTPRequestHandler):
    server_version = "refine-ui/1.0"

    def log_message(self, fmt, *args):  # noqa: D401, ARG002
        sys.stderr.write("[refine-ui] " + (fmt % args) + "\n")

    # one handler per method that all delegate to _dispatch
    def do_GET(self) -> None:  # noqa: N802
        self._dispatch("GET")

    def do_POST(self) -> None:  # noqa: N802
        self._dispatch("POST")

    def do_PATCH(self) -> None:  # noqa: N802
        self._dispatch("PATCH")

    def do_DELETE(self) -> None:  # noqa: N802
        self._dispatch("DELETE")

    def do_OPTIONS(self) -> None:  # noqa: N802
        self.send_response(204)
        self.send_header("Allow", "GET, POST, PATCH, DELETE, OPTIONS")
        self.end_headers()

    def _dispatch(self, method: str) -> None:
        url = urlparse(self.path)
        path = url.path
        query = parse_qs(url.query)

        # SSE first
        if method == "GET" and path == "/api/sse":
            self._serve_sse()
            return

        # Static files
        if method == "GET" and (path == "/" or path == "/index.html"):
            self._serve_static("index.html")
            return
        if method == "GET" and path.startswith("/static/"):
            rel = path[len("/static/"):]
            self._serve_static(rel)
            return

        for m, pat, fn in ROUTES:
            if m != method:
                continue
            match = pat.match(path)
            if not match:
                continue
            body = _read_body(self) if method in ("POST", "PATCH", "PUT", "DELETE") else None
            try:
                status, result = fn(self, match, body, query)
            except Exception as e:
                self._send_json(500, {"error": {"message": repr(e)}})
                return
            self._send_json(status, result)
            return

        self._send_json(404, {"error": {"message": "not found"}})

    # ---- responders ---------------------------------------------------------

    def _send_json(self, status: int, body: Any) -> None:
        data = json.dumps(body, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        if data:
            self.wfile.write(data)

    def _serve_static(self, rel: str) -> None:
        # forbid path traversal
        rel = rel.lstrip("/")
        if ".." in rel.split("/"):
            self._send_json(403, {"error": {"message": "forbidden"}})
            return
        full = STATIC_DIR / rel
        if not full.is_file():
            self._send_json(404, {"error": {"message": "not found"}})
            return
        ctype = _guess_type(full)
        data = full.read_bytes()
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        # Static files are served from the checkout in dev so edits show up on
        # refresh; avoid stale browser-cached assets.
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def _serve_sse(self) -> None:
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream; charset=utf-8")
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Accel-Buffering", "no")
        self.send_header("Connection", "keep-alive")
        self.end_headers()
        q = sse.subscribe()
        # initial comment to flush headers + helo
        try:
            self.wfile.write(b": welcome\n\n")
            self.wfile.flush()
            last_ping = time.monotonic()
            while True:
                try:
                    item = q.get(timeout=15.0)
                except Exception:
                    item = None
                if item is None:
                    # heartbeat
                    self.wfile.write(b": ping\n\n")
                    self.wfile.flush()
                    last_ping = time.monotonic()
                    continue
                evt_id, evt_type, data = item
                self.wfile.write(sse.format_event(evt_id, evt_type, data))
                self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError, OSError):
            pass
        finally:
            sse.unsubscribe(q)


def _guess_type(path: Path) -> str:
    suffix = path.suffix.lower()
    return {
        ".html": "text/html; charset=utf-8",
        ".css": "text/css; charset=utf-8",
        ".js": "application/javascript; charset=utf-8",
        ".json": "application/json; charset=utf-8",
        ".svg": "image/svg+xml",
        ".ico": "image/x-icon",
        ".png": "image/png",
    }.get(suffix, "application/octet-stream")


def run(host: str = "0.0.0.0", port: int = 8080) -> None:
    httpd = ThreadingHTTPServer((host, port), RefineHandler)
    sys.stderr.write(f"[refine-ui] listening on http://{host}:{port}\n")
    httpd.serve_forever()
