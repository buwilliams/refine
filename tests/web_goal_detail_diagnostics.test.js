const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

function htmlEscape(value) {
  return String(value ?? "").replace(/[&<>"']/g, (character) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  }[character]));
}

function namedFunctionSource(source, name) {
  const match = source.match(new RegExp(`^function ${name}\\([^\\n]*\\) \\{[\\s\\S]*?^\\}`, "m"));
  assert.ok(match, `expected ${name} in production source`);
  return match[0];
}

function goalDetailRuntime() {
  const normalizeReviewState = (value) => {
    const normalized = String(value || "").trim().toLowerCase();
    if (!normalized || normalized === "none") return "unclassified";
    if (["pass", "passed", "ok", "success", "succeeded"].includes(normalized)) return "passed";
    if (["fail", "failed", "error", "rejected", "violation"].includes(normalized)) return "failed";
    return normalized;
  };
  const context = vm.createContext({
    Set,
    fmtTime: (value) => String(value),
    governanceReviewStatus: (round) => {
      const states = {
        rules: normalizeReviewState(round?.rule_state),
        product: normalizeReviewState(round?.product_state),
        constitution: normalizeReviewState(round?.constitution_state),
        meta: normalizeReviewState(round?.meta_rule_state),
      };
      return {
        visible: states.rules !== "unclassified",
        passed: Object.values(states).every((state) => state === "passed"),
        states,
      };
    },
    htmlEscape,
    normalizeReviewState,
    reviewStateClass: (value, passedClass = "done") => (
      normalizeReviewState(value) === "passed" ? passedClass : "failed"
    ),
  });
  const commonSource = fs.readFileSync(
    path.join(__dirname, "../src/surfaces/web/static/js/common.js"),
    "utf8",
  );
  vm.runInContext(namedFunctionSource(commonSource, "diagnosticDetailsText"), context);
  vm.runInContext(
    fs.readFileSync(
      path.join(__dirname, "../src/surfaces/web/static/js/features/goals-detail.js"),
      "utf8",
    ),
    context,
  );
  vm.runInContext(`
    globalThis.goalDetailDiagnosticsTest = {
      failure: (goal, round) => renderFailureSummary(goal, round),
      governance: (round) => renderGovernanceSummary(round),
      quality: (round) => renderQualitySummary(round),
    };
  `, context);
  return context.goalDetailDiagnosticsTest;
}

test("Governance and Quality details render structured evidence as readable JSON", () => {
  const runtime = goalDetailRuntime();
  const governance = runtime.governance({
    rule_state: "failed",
    product_state: "passed",
    constitution_state: "passed",
    meta_rule_state: "passed",
    governance_details: {
      phase: "post_implementation",
      violations: [{ rule: 9, reason: "SSE transport required" }],
    },
  });
  const quality = runtime.quality({
    quality_state: "failed",
    quality_details: {
      evaluation_scope: "candidate",
      results: [{ test: "cargo test", passed: false }],
    },
  });

  assert.match(governance, /&quot;phase&quot;: &quot;post_implementation&quot;/);
  assert.match(governance, /&quot;violations&quot;:/);
  assert.doesNotMatch(governance, /\[object Object\]/);
  assert.match(quality, /&quot;evaluation_scope&quot;: &quot;candidate&quot;/);
  assert.match(quality, /&quot;passed&quot;: false/);
  assert.doesNotMatch(quality, /\[object Object\]/);
});

test("a failed Goal falls back to current error evidence when legacy failure fields are empty", () => {
  const runtime = goalDetailRuntime();
  const html = runtime.failure({ status: "failed" }, {
    failure_category: "",
    failure_message: "",
    failure_at: "",
    latest_state_log: {
      datetime: "2026-08-04T10:00:01Z",
      category: "state",
      severity: "info",
      message: "Workflow status changed: in-progress -> failed",
    },
    latest_error_log: {
      datetime: "2026-08-04T10:00:00Z",
      category: "provider",
      severity: "error",
      message: "Agent authentication expired",
      details: { provider: "codex", recovery: "Sign in and submit a recovery round" },
    },
  });

  assert.match(html, /data-testid="goal-failure-message">Agent authentication expired/);
  assert.match(html, /data-testid="goal-failure-details"/);
  assert.match(html, /&quot;provider&quot;: &quot;codex&quot;/);
  assert.doesNotMatch(html, /\[object Object\]/);
});
