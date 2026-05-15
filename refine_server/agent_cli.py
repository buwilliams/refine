"""Agent CLI abstraction.

Refine drives an AI coding agent CLI on the host. Originally that was
Claude Code only; this module lets the operator pick between
`claude`, `codex` (OpenAI Codex CLI), and `gemini` (Google Gemini CLI)
via the `agent_cli` setting. Default is `claude`.

The abstraction covers provider-specific subprocess construction for:

  - `subprocess_mgr.SubprocessManager.launch` — Gap agent runs.
  - `conflict_resolver.attempt_auto_resolve` — merger conflict fixer.
  - `preflight.check` — startup "is the CLI installed + authed?" test.
  - `chat_mgr`, `llm`, and `target_app` — Chat, Import extraction, and
    target-app one-shot prompts.

Output parsing: Claude produces `--output-format=stream-json`; Codex
produces `codex exec --json` JSONL. Refine maps both into round-log/chat
entries where possible. Gemini remains plain line passthrough.
"""
from __future__ import annotations

import shutil
from dataclasses import dataclass
from pathlib import Path


CLI_NAMES = ("claude", "codex", "gemini")
DEFAULT_CLI = "claude"


@dataclass(frozen=True)
class CliSpec:
    name: str            # canonical setting value: "claude" | "codex" | "gemini"
    display_name: str    # human-facing label for the Settings dropdown
    binary: str          # name to look up on PATH

    # Output format parser known by refine:
    #   claude_json = Claude Code stream-json
    #   codex_json  = Codex exec JSONL
    #   plain       = line-by-line passthrough
    output_format: str = "plain"

    # An args-builder. We keep it as a method per spec so each CLI
    # can take whatever flags suit its non-interactive mode best.
    def agent_args(self, binary_path: str, prompt: str, *,
                   cwd: Path | None = None) -> list[str]:
        raise NotImplementedError

    def preflight_args(self, binary_path: str) -> list[str]:
        return [binary_path, "--version"]

    def chat_args(self, binary_path: str, prompt: str, *,
                  session_id: str | None = None,
                  cwd: Path | None = None) -> list[str]:
        raise NotImplementedError

    def one_shot_args(self, binary_path: str, prompt: str, *,
                      cwd: Path | None = None,
                      output_last_message: Path | None = None,
                      output_schema: Path | None = None,
                      json_output: bool = False) -> list[str]:
        return self.agent_args(binary_path, prompt, cwd=cwd)


class _ClaudeSpec(CliSpec):
    def __init__(self) -> None:
        super().__init__(
            name="claude", display_name="Claude Code",
            binary="claude", output_format="claude_json",
        )

    def agent_args(self, binary_path: str, prompt: str, *,
                   cwd: Path | None = None) -> list[str]:
        # `--output-format=stream-json` (with required `--verbose`) makes
        # claude emit one JSON event per line — the rich log + result
        # event refine uses for live status. `--dangerously-skip-permissions`
        # is required for non-interactive autonomous runs.
        return [binary_path, "--print",
                "--output-format=stream-json", "--verbose",
                "--dangerously-skip-permissions", prompt]

    def chat_args(self, binary_path: str, prompt: str, *,
                  session_id: str | None = None,
                  cwd: Path | None = None) -> list[str]:
        args = [binary_path, "--print",
                "--output-format=stream-json", "--verbose"]
        if session_id:
            args.extend(["--resume", session_id])
        args.append(prompt)
        return args

    def one_shot_args(self, binary_path: str, prompt: str, *,
                      cwd: Path | None = None,
                      output_last_message: Path | None = None,
                      output_schema: Path | None = None,
                      json_output: bool = False) -> list[str]:
        return [binary_path, "--print",
                "--dangerously-skip-permissions", prompt]


class _CodexSpec(CliSpec):
    def __init__(self) -> None:
        super().__init__(
            name="codex", display_name="OpenAI Codex",
            binary="codex", output_format="codex_json",
        )

    def agent_args(self, binary_path: str, prompt: str, *,
                   cwd: Path | None = None) -> list[str]:
        # `codex exec` is the non-interactive mode. Refine already
        # provides the outer trust boundary (dedicated host / worktree),
        # so we disable approvals and sandboxing to match the autonomous
        # contract used for Claude's dangerous mode.
        args = [binary_path, "exec", *self._automation_flags(), "--json"]
        if cwd is not None:
            args.extend(["-C", str(cwd)])
        args.append(prompt)
        return args

    def chat_args(self, binary_path: str, prompt: str, *,
                  session_id: str | None = None,
                  cwd: Path | None = None) -> list[str]:
        if session_id:
            return [
                binary_path, "exec", "resume",
                *self._resume_automation_flags(), "--json",
                session_id, prompt,
            ]
        args = [binary_path, "exec", *self._automation_flags(), "--json"]
        if cwd is not None:
            args.extend(["-C", str(cwd)])
        args.append(prompt)
        return args

    def one_shot_args(self, binary_path: str, prompt: str, *,
                      cwd: Path | None = None,
                      output_last_message: Path | None = None,
                      output_schema: Path | None = None,
                      json_output: bool = False) -> list[str]:
        args = [binary_path, "exec", *self._automation_flags()]
        if json_output:
            args.append("--json")
        if cwd is not None:
            args.extend(["-C", str(cwd)])
        if output_schema is not None:
            args.extend(["--output-schema", str(output_schema)])
        if output_last_message is not None:
            args.extend(["--output-last-message", str(output_last_message)])
        args.append(prompt)
        return args

    @staticmethod
    def _automation_flags() -> list[str]:
        return [
            "--dangerously-bypass-approvals-and-sandbox",
            "--color", "never",
        ]

    @staticmethod
    def _resume_automation_flags() -> list[str]:
        # `codex exec resume --help` intentionally exposes a smaller flag
        # set than fresh `exec`; cwd filtering comes from the subprocess cwd.
        return ["--dangerously-bypass-approvals-and-sandbox"]


class _GeminiSpec(CliSpec):
    def __init__(self) -> None:
        super().__init__(
            name="gemini", display_name="Gemini", binary="gemini",
        )

    def agent_args(self, binary_path: str, prompt: str, *,
                   cwd: Path | None = None) -> list[str]:
        # Google's `gemini` CLI. `-p` is the non-interactive prompt
        # mode; `--yolo` skips all confirmation prompts (the
        # autonomous-agent contract refine operates under). Auth comes
        # from the user's Google login or `GEMINI_API_KEY`.
        return [binary_path, "--yolo", "-p", prompt]

    def chat_args(self, binary_path: str, prompt: str, *,
                  session_id: str | None = None,
                  cwd: Path | None = None) -> list[str]:
        return self.agent_args(binary_path, prompt, cwd=cwd)


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
