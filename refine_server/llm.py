"""One-shot LLM calls via the configured host agent CLI.

Currently provides `extract_gaps(text)` for the Import-from-free-text
workflow: hand the agent a paste-dump (transcript, bug report, feedback)
and get back a list of `{name, actual, target}` drafts the user can
review before persisting.

Shares the env/PATH plumbing with `chat_mgr` so we use the same host auth
the user's interactive CLI does.
"""
from __future__ import annotations

import json
import re
import subprocess
import tempfile
from pathlib import Path
from typing import Any

from . import git_ops
from .agent_cli import get_spec, resolve_binary
from .chat_mgr import _chat_env


_EXTRACT_PROMPT_TEMPLATE = """\
You are extracting distinct software-change requests ("Gaps") from a
free-form text dump (meeting transcript, bug report, customer feedback,
feature request, etc.). Each Gap describes ONE thing the team wants
to change about the software — either a BUG (current behavior is
wrong) or a FEATURE (something that doesn't exist yet but should).

For each Gap, produce a JSON object with exactly three string keys:
  - "name":   a short title (max 80 characters).
  - "actual": what is happening — or NOT happening — today (1-3 sentences).
              For a bug, describe the broken current behavior.
              For a feature request, the current behavior is the *absence*
              of the feature. Say so explicitly, e.g. "There is no Tetris
              app in the project today." or "The dashboard has no export
              button." NEVER skip a Gap just because the source text
              doesn't describe a current behavior — for feature requests,
              the absence IS the current behavior.
  - "target": what should happen instead (1-3 sentences).

Return a JSON array of those objects. Return [] only when the text
contains no actionable software change — pure social talk, weather
small-talk, "thanks for the demo", etc. A request to build something
new IS an actionable software change.

IMPORTANT: Output ONLY the JSON array — no prose, no markdown code
fences, no commentary. Your entire response must parse as JSON.

Source text:
<<<
{text}
>>>
"""

_LINE_LIST_MIN_ITEMS = 50
_LINE_LIST_MIN_PLAIN_ITEMS = 100
_LINE_LIST_MAX_ITEM_CHARS = 500
_LINE_LIST_MAX_LONG_RATIO = 0.10
_LIST_MARKER_RE = re.compile(
    r"^\s*(?:[-*]|\d+[.)\]]|[A-Za-z][.)\]]|\[[ xX]\])\s+"
)
_SPEAKER_RE = re.compile(r"^[A-Z][A-Za-z0-9 ._-]{0,30}:\s+\S")
_ACTUAL_TARGET_RE = re.compile(
    r"^\s*actual\s*:\s*(?P<actual>.*?)\s+target\s*:\s*(?P<target>.+)\s*$",
    re.IGNORECASE,
)
_ARROW_RE = re.compile(r"\s+(?:-{1,2}>|=>|\u2192)\s+")


def extract_gaps(text: str, *, provider: str | None = None) -> list[dict]:
    """Call the configured agent CLI with a structured extraction prompt and parse
    its response as a JSON array of `{name, actual, target}` drafts.

    Raises `RuntimeError` if the CLI can't be invoked or exits non-zero.
    Returns an empty list when the response has no parseable JSON
    array (the import UI handles that as "no drafts extracted").
    """
    text = (text or "").strip()
    if not text:
        return []
    line_drafts = _extract_line_list_drafts(text)
    if line_drafts is not None:
        return line_drafts
    env = _chat_env()
    spec = get_spec(provider)
    binary = resolve_binary(spec, env)
    prompt = _EXTRACT_PROMPT_TEMPLATE.format(text=text)
    cwd = git_ops.client_repo_path()
    output_last_message: Path | None = None
    tmp: tempfile.TemporaryDirectory | None = None
    if spec.name == "codex":
        tmp = tempfile.TemporaryDirectory(prefix="refine-codex-import-")
        tdir = Path(tmp.name)
        output_last_message = tdir / "last_message.txt"
    args = spec.one_shot_args(
        binary, prompt, cwd=cwd,
        output_last_message=output_last_message,
        json_output=spec.output_format == "codex_json",
    )
    try:
        out = subprocess.run(
            args,
            capture_output=True, text=True, timeout=180, env=env,
            cwd=str(cwd),
        )
    except subprocess.TimeoutExpired as e:
        if tmp is not None:
            tmp.cleanup()
        raise RuntimeError(f"{spec.binary} timed out after 180s") from e
    except (OSError, FileNotFoundError) as e:
        if tmp is not None:
            tmp.cleanup()
        raise RuntimeError(f"could not launch {spec.binary}: {e}") from e
    if out.returncode != 0:
        msg = (out.stdout or "").strip() or (out.stderr or "").strip() \
              or f"{spec.binary} exited {out.returncode}"
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
    return _normalize_drafts(_parse_json_array(raw))


def _extract_line_list_drafts(text: str) -> list[dict] | None:
    """Return drafts for large newline-delimited gap lists.

    Imports commonly come from spreadsheets or issue trackers where each
    non-empty line is already one requested change. Sending hundreds of
    those through a one-shot model call is slow and can hit provider
    timeouts, so this keeps obviously line-oriented input local while
    leaving transcripts and prose dumps on the LLM path.
    """
    raw_items = [line.strip() for line in text.splitlines() if line.strip()]
    marked_items = sum(1 for line in raw_items if _LIST_MARKER_RE.match(line))
    structured_items = sum(
        1 for line in raw_items if _ACTUAL_TARGET_RE.match(line)
    )
    speaker_items = sum(
        1 for line in raw_items
        if _SPEAKER_RE.match(line)
        and not line.lower().startswith(("actual:", "target:", "name:"))
    )
    lines = [_clean_line_item(line) for line in raw_items]
    items = [line for line in lines if line]
    if len(items) < _LINE_LIST_MIN_ITEMS:
        return None
    if (
        marked_items == 0
        and structured_items == 0
        and len(items) < _LINE_LIST_MIN_PLAIN_ITEMS
    ):
        return None
    if speaker_items / len(items) > 0.25:
        return None
    long_items = sum(1 for item in items if len(item) > _LINE_LIST_MAX_ITEM_CHARS)
    if long_items / len(items) > _LINE_LIST_MAX_LONG_RATIO:
        return None
    return _normalize_drafts([_line_item_to_draft(item) for item in items])


def _clean_line_item(line: str) -> str:
    line = line.strip()
    if not line:
        return ""
    line = _LIST_MARKER_RE.sub("", line).strip()
    return line


def _line_item_to_draft(line: str) -> dict[str, str]:
    actual = ""
    target = line
    m = _ACTUAL_TARGET_RE.match(line)
    if m:
        actual = m.group("actual").strip()
        target = m.group("target").strip()
    else:
        parts = _ARROW_RE.split(line, maxsplit=1)
        if len(parts) == 2:
            actual, target = parts[0].strip(), parts[1].strip()
    source = target or actual or line
    return {
        "name": _draft_name(source),
        "actual": actual,
        "target": target,
    }


def _draft_name(text: str) -> str:
    first_line = (text or "Untitled Gap").strip().split("\n", 1)[0]
    first_sentence = re.split(r"[.!?]", first_line, maxsplit=1)[0].strip()
    name = first_sentence or first_line or "Untitled Gap"
    if len(name) > 80:
        name = name[:77].rstrip() + "..."
    return name


def _parse_json_array(text: str) -> list[Any]:
    """Find the first top-level JSON array in `text`.

    Agents usually honor "JSON only" but can occasionally wrap the
    response in a markdown code fence or prefixes a sentence. We walk
    each `[` candidate until one parses cleanly. Returns an empty list
    if nothing parseable is found.
    """
    decoder = json.JSONDecoder()
    i = 0
    while True:
        start = text.find("[", i)
        if start == -1:
            return []
        try:
            value, _end = decoder.raw_decode(text, start)
        except json.JSONDecodeError:
            i = start + 1
            continue
        if isinstance(value, list):
            return value
        i = start + 1


def _normalize_drafts(raw: list[Any]) -> list[dict]:
    out: list[dict] = []
    for item in raw:
        if not isinstance(item, dict):
            continue
        name = str(item.get("name") or "").strip()[:200]
        actual = str(item.get("actual") or "").strip()
        target = str(item.get("target") or "").strip()
        if not actual and not target:
            continue
        out.append({
            "name": name,
            "actual": actual,
            "target": target,
            "preview": (target or actual).split("\n", 1)[0],
        })
    return out


def _extract_final_text(stdout: str) -> str:
    """Return final assistant text from Codex JSONL, or raw stdout."""
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
