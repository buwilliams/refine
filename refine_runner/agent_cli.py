"""Agent CLI abstraction.

Refine drives an AI coding agent CLI on the host. Originally that was
Claude Code only; this module lets the operator pick between
`claude`, `codex` (OpenAI Codex CLI), and `gemini` (Google Gemini CLI)
via the `agent_cli` setting. Default is `claude`.

The abstraction covers three call sites:

  - `subprocess_mgr.SubprocessManager.launch` — Gap agent runs.
  - `conflict_resolver.attempt_auto_resolve` — merger conflict fixer.
  - `preflight.check` — startup "is the CLI installed + authed?" test.

Chat (`chat_mgr`) still uses Claude exclusively because it relies on
Claude's `--resume <session-id>` to thread context across turns;
Codex and Gemini don't have a 1:1 equivalent and the chat UX would
degrade. The Settings UI surfaces this limitation.

Output parsing: only Claude produces a rich `--output-format=stream-json`
stream that refine maps to round-log entries with tool-use summaries
and a terminal `result` event for the agent-reported-success signal.
Codex and Gemini get a simpler line-by-line pass-through; the
`is_error` short-circuit and tool summaries are Claude-only. Idle
timeout / hard cap / pgroup-SIGTERM all keep working — they don't
depend on stream-json.
"""
from __future__ import annotations

import shutil
from dataclasses import dataclass


CLI_NAMES = ("claude", "codex", "gemini")
DEFAULT_CLI = "claude"


@dataclass(frozen=True)
class CliSpec:
    name: str            # canonical setting value: "claude" | "codex" | "gemini"
    display_name: str    # human-facing label for the Settings dropdown
    binary: str          # name to look up on PATH

    # Whether refine's stream-json event parser knows this CLI's
    # output format. Currently only Claude.
    structured_output: bool = False

    # An args-builder. We keep it as a method per spec so each CLI
    # can take whatever flags suit its non-interactive mode best.
    def agent_args(self, binary_path: str, prompt: str) -> list[str]:
        raise NotImplementedError

    def preflight_args(self, binary_path: str) -> list[str]:
        return [binary_path, "--version"]


class _ClaudeSpec(CliSpec):
    def __init__(self) -> None:
        super().__init__(
            name="claude", display_name="Claude Code",
            binary="claude", structured_output=True,
        )

    def agent_args(self, binary_path: str, prompt: str) -> list[str]:
        # `--output-format=stream-json` (with required `--verbose`) makes
        # claude emit one JSON event per line — the rich log + result
        # event refine uses for live status. `--dangerously-skip-permissions`
        # is required for non-interactive autonomous runs.
        return [binary_path, "--print",
                "--output-format=stream-json", "--verbose",
                "--dangerously-skip-permissions", prompt]


class _CodexSpec(CliSpec):
    def __init__(self) -> None:
        super().__init__(
            name="codex", display_name="OpenAI Codex",
            binary="codex", structured_output=False,
        )

    def agent_args(self, binary_path: str, prompt: str) -> list[str]:
        # OpenAI's `codex` (the GitHub `openai/codex` CLI). `exec` is
        # the non-interactive one-shot mode; `--full-auto` runs without
        # confirmation prompts (the autonomous-agent contract refine
        # operates under). Auth comes from `~/.codex/auth.json` or
        # `OPENAI_API_KEY`.
        return [binary_path, "exec", "--full-auto", prompt]


class _GeminiSpec(CliSpec):
    def __init__(self) -> None:
        super().__init__(
            name="gemini", display_name="Gemini",
            binary="gemini", structured_output=False,
        )

    def agent_args(self, binary_path: str, prompt: str) -> list[str]:
        # Google's `gemini` CLI. `-p` is the non-interactive prompt
        # mode; `--yolo` skips all confirmation prompts (the
        # autonomous-agent contract refine operates under). Auth comes
        # from the user's Google login or `GEMINI_API_KEY`.
        return [binary_path, "--yolo", "-p", prompt]


_SPECS: dict[str, CliSpec] = {
    "claude": _ClaudeSpec(),
    "codex":  _CodexSpec(),
    "gemini": _GeminiSpec(),
}


def get_spec(name: str | None) -> CliSpec:
    """Resolve a setting value to a CliSpec. Unknown names fall back to
    the default so a corrupted setting doesn't break the runner."""
    norm = (name or "").strip().lower() or DEFAULT_CLI
    return _SPECS.get(norm, _SPECS[DEFAULT_CLI])


def all_specs() -> list[CliSpec]:
    """Used by the Settings page to render the dropdown options."""
    return [_SPECS[n] for n in CLI_NAMES]


def resolve_binary(spec: CliSpec, env: dict[str, str]) -> str:
    """Resolve the binary on the supplied PATH (typically the user's
    interactive-login PATH, captured once in chat_mgr). Falls back to
    the bare name so the resulting subprocess.Popen produces a
    `FileNotFoundError` with a useful message."""
    return shutil.which(spec.binary, path=env.get("PATH")) or spec.binary
