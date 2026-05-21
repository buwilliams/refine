"""Gap Governance settings, AI prompts, and result normalization."""
from __future__ import annotations

import json
import secrets
import subprocess
import tempfile
from pathlib import Path
from typing import Any

from refine_server import db, gaps as shared_gaps, perf_metrics
from refine_server.gaps import now_iso

from . import git_ops
from .agent_cli import get_spec, resolve_binary
from .chat_mgr import _chat_env


RULE_ACTIONS = ("add", "edit", "remove")


def load_settings(conn) -> dict[str, Any]:
    return {
        "product": db.get_setting(conn, "governance_product", "") or "",
        "constitution": db.get_setting(conn, "governance_constitution", "") or "",
        "rules": normalize_rules(
            db.get_setting(conn, "governance_rules_json", "[]") or "[]",
        ),
    }


def is_configured(conn) -> bool:
    settings = load_settings(conn)
    return bool(settings["product"].strip() and settings["constitution"].strip())


def save_settings(conn, *, product: str | None = None,
                  constitution: str | None = None,
                  rules: list[dict[str, Any]] | None = None) -> dict[str, Any]:
    if product is not None:
        db.set_setting(conn, "governance_product", str(product).strip())
    if constitution is not None:
        db.set_setting(conn, "governance_constitution", str(constitution).strip())
    if rules is not None:
        db.set_setting(
            conn,
            "governance_rules_json",
            json.dumps(normalize_rules(rules), ensure_ascii=False),
        )
    return load_settings(conn)


def normalize_rules(value: Any) -> list[dict[str, str]]:
    if isinstance(value, str):
        try:
            value = json.loads(value)
        except json.JSONDecodeError:
            value = []
    if not isinstance(value, list):
        return []
    now = now_iso()
    out: list[dict[str, str]] = []
    seen: set[str] = set()
    for item in value:
        if isinstance(item, str):
            text = _one_line(item)
            raw_id = ""
            created = now
            updated = now
            source = "manual"
        elif isinstance(item, dict):
            text = _one_line(item.get("text") or "")
            raw_id = str(item.get("id") or "").strip()
            created = str(item.get("created") or now)
            updated = str(item.get("updated") or now)
            source = str(item.get("source") or "manual").strip()[:40] or "manual"
        else:
            continue
        if not text:
            continue
        rid = raw_id if raw_id and raw_id not in seen else _new_rule_id()
        while rid in seen:
            rid = _new_rule_id()
        seen.add(rid)
        out.append({
            "id": rid,
            "text": text[:500],
            "created": created,
            "updated": updated,
            "source": source,
        })
    return out


def generate_rules(product: str, constitution: str, *,
                   provider: str | None = None) -> dict[str, Any]:
    prompt = _RULES_PROMPT.format(
        product=product.strip(),
        constitution=constitution.strip(),
    )
    raw = _run_one_shot(
        prompt, provider=provider, timeout=300.0,
        operation="ai.governance_generate_rules",
    )
    rules = normalize_rules(_parse_json_array(raw))
    return {"ok": True, "rules": rules, "raw": raw}


def classify_gap(conn, gap_id: str, *, provider: str | None = None) -> dict[str, Any]:
    settings = load_settings(conn)
    gap = shared_gaps.read_gap_json(gap_id, include_logs=False)
    if not gap or not gap.get("rounds"):
        return normalize_classification({
            "rule_state": "blocked",
            "meta_rule_state": "none",
            "product_state": "fail",
            "constitution_state": "fail",
            "message": "Gap has no round to review.",
            "details": "",
            "rule_actions": [],
        })
    latest = gap["rounds"][-1]
    prompt = _CLASSIFY_PROMPT.format(
        product=settings["product"],
        constitution=settings["constitution"],
        rules=json.dumps(settings["rules"], ensure_ascii=False, indent=2),
        name=gap.get("name", ""),
        actual=latest.get("actual", ""),
        target=latest.get("target", ""),
    )
    raw = _run_one_shot(
        prompt, provider=provider, timeout=300.0,
        operation="ai.governance_classify",
    )
    obj = _parse_json_object(raw) or {}
    result = normalize_classification(obj)
    result["raw"] = raw
    return result


def normalize_classification(obj: dict[str, Any]) -> dict[str, Any]:
    rule_state = str(obj.get("rule_state") or "needs_review").strip()
    if rule_state not in shared_gaps.RULE_STATES:
        rule_state = "needs_review"
    meta_rule_state = str(obj.get("meta_rule_state") or "none").strip()
    if meta_rule_state not in shared_gaps.META_RULE_STATES:
        meta_rule_state = "none"
    product_state = _binary_state(obj.get("product_state"))
    constitution_state = _binary_state(obj.get("constitution_state"))
    actions = []
    for item in obj.get("rule_actions") or obj.get("governance_rule_actions") or []:
        if not isinstance(item, dict):
            continue
        action = str(item.get("action") or "").strip()
        if action not in RULE_ACTIONS:
            continue
        actions.append({
            "action": action,
            "id": str(item.get("id") or "").strip(),
            "text": _one_line(item.get("text") or "")[:500],
            "reason": str(item.get("reason") or "").strip()[:1000],
        })
    return {
        "rule_state": rule_state,
        "meta_rule_state": meta_rule_state,
        "product_state": product_state,
        "constitution_state": constitution_state,
        "governance_message": str(
            obj.get("message") or obj.get("governance_message") or ""
        ).strip()[:1000],
        "governance_details": str(
            obj.get("details") or obj.get("governance_details") or ""
        ).strip()[:4000],
        "governance_rule_actions": actions,
    }


def has_passed(round_obj: dict[str, Any]) -> bool:
    shared_gaps.normalize_round_governance(round_obj)
    return (
        round_obj.get("rule_state") == "passed"
        and round_obj.get("product_state") == "pass"
        and round_obj.get("constitution_state") == "pass"
    )


def latest_round_is_governance_blocked(gap: dict[str, Any] | None) -> bool:
    if not gap or not gap.get("rounds"):
        return False
    latest = gap["rounds"][-1]
    shared_gaps.normalize_round_governance(latest)
    if (
        latest.get("rule_state") == "unclassified"
        and latest.get("product_state") == "unclassified"
        and latest.get("constitution_state") == "unclassified"
    ):
        return False
    return not has_passed(latest)


def apply_rule_actions(conn, actions: list[dict[str, Any]]) -> list[dict[str, Any]]:
    current = load_settings(conn)["rules"]
    by_id = {rule["id"]: rule for rule in current}
    applied: list[dict[str, Any]] = []
    for action in actions:
        kind = action.get("action")
        rid = action.get("id") or ""
        text = _one_line(action.get("text") or "")
        if kind == "add" and text:
            rule = {
                "id": _new_rule_id(),
                "text": text,
                "created": now_iso(),
                "updated": now_iso(),
                "source": "governance_agent",
            }
            current.append(rule)
            applied.append({**action, "id": rule["id"]})
        elif kind == "edit" and rid in by_id and text:
            before = by_id[rid]["text"]
            by_id[rid]["text"] = text
            by_id[rid]["updated"] = now_iso()
            by_id[rid]["source"] = "governance_agent"
            applied.append({**action, "before": before})
        elif kind == "remove" and rid in by_id:
            before = by_id[rid]["text"]
            current = [rule for rule in current if rule["id"] != rid]
            by_id.pop(rid, None)
            applied.append({**action, "before": before})
    if applied:
        save_settings(conn, rules=current)
    return applied


def _run_one_shot(prompt: str, *, provider: str | None,
                  timeout: float, operation: str = "ai.one_shot") -> str:
    metric_start = perf_metrics.now()
    prompt_bytes = len(prompt.encode("utf-8", errors="replace"))
    metric_provider = provider or ""
    env = _chat_env()
    spec = get_spec(provider)
    metric_provider = spec.name
    binary = resolve_binary(spec, env)
    cwd = git_ops.client_repo_path()
    output_last_message: Path | None = None
    tmp: tempfile.TemporaryDirectory | None = None
    if spec.name == "codex":
        tmp = tempfile.TemporaryDirectory(prefix="refine-governance-")
        output_last_message = Path(tmp.name) / "last_message.txt"
    args = spec.one_shot_args(
        binary, prompt, cwd=cwd,
        output_last_message=output_last_message,
        json_output=spec.output_format == "codex_json",
    )
    try:
        out = subprocess.run(
            args, capture_output=True, text=True, timeout=timeout,
            env=env, cwd=str(cwd),
        )
    except subprocess.TimeoutExpired as e:
        perf_metrics.record(
            operation,
            elapsed_ms=perf_metrics.elapsed_ms(metric_start),
            success=False,
            provider=metric_provider,
            bytes_in=prompt_bytes,
            details={"error": "timeout", "timeout": timeout},
        )
        if tmp is not None:
            tmp.cleanup()
        raise RuntimeError(f"{spec.binary} timed out after {int(timeout)}s") from e
    except (OSError, FileNotFoundError) as e:
        perf_metrics.record(
            operation,
            elapsed_ms=perf_metrics.elapsed_ms(metric_start),
            success=False,
            provider=metric_provider,
            bytes_in=prompt_bytes,
            details={"error": repr(e)[:1000]},
        )
        if tmp is not None:
            tmp.cleanup()
        raise RuntimeError(f"could not launch {spec.binary}: {e}") from e
    if out.returncode != 0:
        msg = (out.stderr or out.stdout or f"{spec.binary} exited {out.returncode}").strip()
        perf_metrics.record(
            operation,
            elapsed_ms=perf_metrics.elapsed_ms(metric_start),
            success=False,
            provider=metric_provider,
            bytes_in=prompt_bytes,
            bytes_out=len(((out.stdout or "") + (out.stderr or "")).encode("utf-8", errors="replace")),
            details={"returncode": out.returncode, "message": msg[:1000]},
        )
        if tmp is not None:
            tmp.cleanup()
        raise RuntimeError(msg)
    raw = ""
    if output_last_message is not None and output_last_message.exists():
        raw = output_last_message.read_text(encoding="utf-8", errors="replace")
    if not raw:
        raw = _extract_final_text(out.stdout or "")
    if tmp is not None:
        tmp.cleanup()
    perf_metrics.record(
        operation,
        elapsed_ms=perf_metrics.elapsed_ms(metric_start),
        provider=metric_provider,
        bytes_in=prompt_bytes,
        bytes_out=len(raw.encode("utf-8", errors="replace")),
    )
    return raw


def _parse_json_object(text: str) -> dict[str, Any] | None:
    value = _parse_first_json_value(text)
    return value if isinstance(value, dict) else None


def _parse_json_array(text: str) -> list[Any]:
    value = _parse_first_json_value(text)
    return value if isinstance(value, list) else []


def _parse_first_json_value(text: str) -> Any:
    text = _strip_code_fence(text)
    decoder = json.JSONDecoder()
    starts = [i for i, ch in enumerate(text) if ch in "[{"]
    for start in starts:
        try:
            value, _end = decoder.raw_decode(text, start)
            return value
        except json.JSONDecodeError:
            continue
    return None


def _strip_code_fence(text: str) -> str:
    stripped = text.strip()
    if not stripped.startswith("```"):
        return stripped
    lines = stripped.splitlines()
    if lines and lines[0].startswith("```"):
        lines = lines[1:]
    if lines and lines[-1].startswith("```"):
        lines = lines[:-1]
    return "\n".join(lines).strip()


def _extract_final_text(stdout: str) -> str:
    if not stdout.lstrip().startswith("{"):
        return stdout
    last = ""
    for line in stdout.splitlines():
        try:
            evt = json.loads(line)
        except json.JSONDecodeError:
            continue
        item = evt.get("item") if isinstance(evt.get("item"), dict) else {}
        text = item.get("text") or evt.get("text")
        item_type = item.get("type")
        if text and item_type in ("agent_message", "assistant_message"):
            last = str(text)
    return last or stdout


def _binary_state(value: Any) -> str:
    state = str(value or "fail").strip()
    return state if state in ("pass", "fail", "unclassified") else "fail"


def _one_line(value: Any) -> str:
    return " ".join(str(value or "").strip().splitlines()).strip()


def _new_rule_id() -> str:
    return "rule_" + secrets.token_hex(6)


_RULES_PROMPT = """\
You are maintaining concise governance rules for a software project.

Product:
<<<
{product}
>>>

Constitution:
<<<
{constitution}
>>>

Return ONLY a JSON array of one-line rule strings. Each rule must be specific,
testable, and short enough to scan in a settings UI. Do not include prose,
markdown fences, or explanations.
"""


_CLASSIFY_PROMPT = """\
You are the Governance agent for this project. Classify whether one submitted
Gap should be allowed to proceed to implementation, and optionally maintain the
rule list when the submitted Gap reveals a useful rule change.

Product:
<<<
{product}
>>>

Constitution:
<<<
{constitution}
>>>

Rules JSON:
{rules}

Gap:
Name: {name}
Actual:
<<<
{actual}
>>>
Target:
<<<
{target}
>>>

Return ONLY a JSON object with:
- rule_state: one of unclassified, passed, failed, blocked, needs_review,
  needs_context, exception_requested
- meta_rule_state: one of unclassified, none, candidate_rule,
  rule_review_needed, ambiguous_rule, stale_rule, conflicting_rules
- product_state: pass or fail
- constitution_state: pass or fail
- message: one short sentence for the user
- details: optional concise explanation
- rule_actions: optional array of {{action,id,text,reason}} where action is
  add, edit, or remove. Use existing rule ids for edit/remove.

Only return passed/pass/pass when the Gap is consistent with Product,
Constitution, and Rules.
"""
