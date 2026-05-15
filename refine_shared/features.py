"""Provider-scoped feature flags.

Some refine features (Chat, Import) depend on provider-specific
capabilities — session-resume threading and structured JSON extraction
prompts. Rather than
silently failing when the operator switches `agent_cli`, every such
feature is gated through `is_enabled(conn, feature)` and the Settings
page surfaces the matrix.

How resolution works:

- `_DEFAULTS` is the code-defined "what's known to work" matrix.
  When refine learns to drive Codex's chat or Gemini's extraction,
  the corresponding default flips to True here.
- Operator can override a default by storing a setting under
  `feature_<provider>_<feature>` ("1" / "0"). Empty / unset means
  fall back to the default. Useful for early-access testing.
- `current_provider()` resolves to the `agent_cli` setting (or the
  hardcoded default if no setting). Callers can also pass `provider`
  explicitly when they need to evaluate the matrix for the Settings UI.

Feature names are stable strings; adding a new gate is a new key in
`FEATURES` plus a row in `_DEFAULTS` for every provider.
"""
from __future__ import annotations

import sqlite3


# Feature registry. Each entry documents the dependency so the
# Settings UI can show why a provider lacks support today.
FEATURES = {
    "chat": {
        "label": "Chat",
        "description": (
            "Interactive chat sessions, both the standalone tab and "
            "the per-Gap `Open Chat` affordance. Requires the CLI's "
            "session-resume support to thread "
            "context across turns."
        ),
    },
    "import_gaps": {
        "label": "Import (LLM extraction)",
        "description": (
            "LLM-driven Gap extraction from free-form text (the "
            "Import… button). Requires a one-shot non-interactive "
            "prompt with structured JSON output that refine can "
            "parse."
        ),
    },
}

# Canonical provider list — kept in sync with `agent_cli.CLI_NAMES`
# (we don't import from `refine_runner` here to avoid a layer
# violation; refine_shared sits below refine_runner).
PROVIDERS = ("claude", "codex", "gemini")
DEFAULT_PROVIDER = "claude"


# (provider, feature) → enabled by default.
# Provider defaults reflect what refine knows how to drive directly.
_DEFAULTS: dict[tuple[str, str], bool] = {
    ("claude", "chat"):        True,
    ("claude", "import_gaps"): True,
    ("codex",  "chat"):        True,
    ("codex",  "import_gaps"): True,
    ("gemini", "chat"):        False,
    ("gemini", "import_gaps"): False,
}


def _setting_key(provider: str, feature: str) -> str:
    return f"feature_{provider}_{feature}"


def current_provider(conn: sqlite3.Connection) -> str:
    """The provider feature checks default to."""
    from . import db
    value = (db.get_setting(conn, "agent_cli") or DEFAULT_PROVIDER)
    value = value.strip().lower()
    return value if value in PROVIDERS else DEFAULT_PROVIDER


def is_enabled(conn: sqlite3.Connection, feature: str, *,
               provider: str | None = None) -> bool:
    """The single gate every caller uses. Returns False for unknown
    feature keys so a typo in a guard never silently flips a gate
    open."""
    if feature not in FEATURES:
        return False
    if provider is None:
        provider = current_provider(conn)
    if provider not in PROVIDERS:
        return False
    from . import db
    override = db.get_setting(conn, _setting_key(provider, feature))
    if override is not None and override != "":
        return override in ("1", "true", "True", "yes")
    return _DEFAULTS.get((provider, feature), False)


def get_matrix(conn: sqlite3.Connection) -> dict:
    """Full matrix for the `/api/features` endpoint. Reports the
    *effective* enabled state per (provider, feature), the underlying
    default, and whether the operator has overridden it — so the
    Settings UI can render "default" / "overridden" status."""
    out = {
        "current_provider": current_provider(conn),
        "providers": list(PROVIDERS),
        "features": [
            {"key": k, "label": v["label"], "description": v["description"]}
            for k, v in FEATURES.items()
        ],
        "matrix": {},
    }
    from . import db
    for provider in PROVIDERS:
        for feature in FEATURES:
            override_raw = db.get_setting(
                conn, _setting_key(provider, feature),
            )
            has_override = bool(override_raw is not None and override_raw != "")
            default = _DEFAULTS.get((provider, feature), False)
            effective = is_enabled(conn, feature, provider=provider)
            out["matrix"][f"{provider}.{feature}"] = {
                "default": default,
                "override": has_override,
                "enabled": effective,
            }
    return out


def set_override(conn: sqlite3.Connection, provider: str, feature: str,
                 enabled: bool | None) -> None:
    """`enabled=None` clears the override so the default reapplies."""
    if provider not in PROVIDERS or feature not in FEATURES:
        raise ValueError(f"unknown provider/feature: {provider}/{feature}")
    from . import db
    key = _setting_key(provider, feature)
    if enabled is None:
        db.set_setting(conn, key, "")
        return
    db.set_setting(conn, key, "1" if enabled else "0")
