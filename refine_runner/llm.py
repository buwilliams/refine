"""One-shot LLM calls via the host `claude` CLI.

Currently provides `extract_gaps(text)` for the Import-from-free-text
workflow: hand claude a paste-dump (transcript, bug report, feedback)
and get back a list of `{name, actual, target}` drafts the user can
review before persisting.

Shares the env/PATH plumbing with `chat_mgr` so we use the same OAuth
login the user's interactive `claude` does — no API keys involved.
"""
from __future__ import annotations

import json
import subprocess
from typing import Any

from .chat_mgr import _chat_env, _resolve_claude


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


def extract_gaps(text: str) -> list[dict]:
    """Call `claude --print` with a structured extraction prompt and parse
    its response as a JSON array of `{name, actual, target}` drafts.

    Raises `RuntimeError` if claude can't be invoked or exits non-zero.
    Returns an empty list when claude's response has no parseable JSON
    array (the import UI handles that as "no drafts extracted").
    """
    text = (text or "").strip()
    if not text:
        return []
    env = _chat_env()
    claude = _resolve_claude(env)
    prompt = _EXTRACT_PROMPT_TEMPLATE.format(text=text)
    try:
        out = subprocess.run(
            [claude, "--print", prompt],
            capture_output=True, text=True, timeout=180, env=env,
        )
    except subprocess.TimeoutExpired as e:
        raise RuntimeError("claude timed out after 180s") from e
    except (OSError, FileNotFoundError) as e:
        raise RuntimeError(f"could not launch claude: {e}") from e
    if out.returncode != 0:
        msg = (out.stdout or "").strip() or (out.stderr or "").strip() \
              or f"claude exited {out.returncode}"
        raise RuntimeError(msg)
    return _normalize_drafts(_parse_json_array(out.stdout or ""))


def _parse_json_array(text: str) -> list[Any]:
    """Find the first top-level JSON array in `text`.

    Claude almost always honors "JSON only" but occasionally wraps the
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
