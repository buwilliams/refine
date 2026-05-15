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
