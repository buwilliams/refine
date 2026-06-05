"""Gap detail failure banner freshness tests."""
from __future__ import annotations

import shutil
import subprocess
from pathlib import Path


def main() -> int:
    node = shutil.which("node")
    if not node:
        print("node unavailable; skipped gap failure banner JS test")
        return 0

    root = Path(__file__).resolve().parents[1]
    gaps_detail_js = (
        root / "refine_ui/static/js/features/gaps-detail.js"
    ).read_text(encoding="utf-8")
    start = gaps_detail_js.index("function currentRoundLog")
    end = gaps_detail_js.index("function computeGovernanceBanner")
    tested_functions = gaps_detail_js[start:end]
    script = f"""
const assert = require("assert");
{tested_functions}

const olderError = {{
  latest_log: {{
    datetime: "2026-01-01T00:00:00Z",
    severity: "error",
    message: "old agent error",
  }},
  latest_error_log: {{
    datetime: "2026-01-01T00:00:00Z",
    severity: "error",
    message: "old agent error",
  }},
  latest_workflow_log: {{
    datetime: "2026-01-02T00:00:00Z",
    severity: "info",
    message: "Workflow status changed: todo → failed",
  }},
}};
assert.strictEqual(
  computeFailureBanner({{ status: "failed" }}, olderError).message,
  "Workflow status changed: todo → failed",
);
assert.strictEqual(computeFailureBanner({{ status: "review" }}, olderError), null);

const resolvedMergeError = {{
  latest_log: {{
    datetime: "2026-01-03T00:00:00Z",
    severity: "info",
    category: "state",
    message: "Target application rebuilt; Gap is ready for review",
  }},
  latest_error_log: {{
    datetime: "2026-01-02T00:00:00Z",
    severity: "error",
    category: "git",
    message: "git merge failed",
  }},
  latest_state_log: {{
    datetime: "2026-01-03T00:00:00Z",
    severity: "info",
    category: "state",
    message: "Target application rebuilt; Gap is ready for review",
  }},
  latest_workflow_log: {{
    datetime: "2026-01-02T00:00:00Z",
    severity: "warn",
    category: "state",
    message: "Workflow status changed: ready-merge → failed; git merge failed",
  }},
}};
assert.strictEqual(computeFailureBanner({{ status: "review" }}, resolvedMergeError), null);

const currentError = {{
  latest_error_log: {{
    datetime: "2026-01-03T00:00:00Z",
    severity: "error",
    message: "current agent error",
  }},
  latest_workflow_log: {{
    datetime: "2026-01-02T00:00:00Z",
    severity: "info",
    message: "Workflow status changed: todo → failed",
  }},
}};
assert.strictEqual(
  computeFailureBanner({{ status: "failed" }}, currentError).message,
  "current agent error",
);
assert.strictEqual(
  computeFailureBanner({{ status: "review" }}, currentError).message,
  "current agent error",
);
"""
    subprocess.run([node, "-e", script], check=True)
    print("gap failure banner freshness tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
