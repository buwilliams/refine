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
      implementationPlan: (round, history = {}) => renderImplementationPlan(round, 0, history),
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

test("Failure and Quality cards render the actionable summary safely and retain full Details", () => {
  const runtime = goalDetailRuntime();
  const summary = "Quality failed: “UI <script> & keyboard” — supervised command exited with code 7.";
  const round = {
    failure_category: "quality",
    failure_message: summary,
    failure_at: "2026-08-06T12:00:00Z",
    quality_state: "failed",
    quality_message: summary,
    quality_details: {
      operation_id: "quality-1",
      results: [{
        test: "UI <script> & keyboard",
        status: "failed",
        command: "node --test 'special & chars'",
        exit_code: 7,
        evidence: "first line\nsecond <line>",
      }],
      diagnostics: ["complete & authoritative <diagnostic>"],
    },
  };

  const failure = runtime.failure({ status: "failed" }, round);
  const quality = runtime.quality(round);

  assert.match(failure, /goal-failure-message/);
  assert.match(failure, /UI &lt;script&gt; &amp; keyboard/);
  assert.doesNotMatch(failure, /<script>/);
  assert.match(quality, /goal-quality-message/);
  assert.match(quality, /supervised command exited with code 7/);
  assert.match(quality, /&quot;results&quot;:/);
  assert.match(quality, /&quot;command&quot;: &quot;node --test &#39;special &amp; chars&#39;&quot;/);
  assert.match(quality, /&quot;diagnostics&quot;:/);
  assert.match(quality, /complete &amp; authoritative &lt;diagnostic&gt;/);
  assert.doesNotMatch(quality, /\[object Object\]/);
});

test("Implementation Plan renders final checklist, immutable history, outcomes, and failures safely", () => {
  const runtime = goalDetailRuntime();
  const html = runtime.implementationPlan({
    implementation_plan: {
      phase: "implement",
      state: "failed",
      phase_started_at: "2026-08-11T09:20:00Z",
      updated_at: "2026-08-11T10:00:00Z",
      active_process: { operation_id: "op-implement", process_id: "goal-agent-1" },
      proposal: {
        completed_at: "2026-08-11T09:00:00Z",
        result: { summary: "Original <proposal>", checklist: [{ id: "P1", description: "Old & risky" }] },
      },
      criticism: {
        completed_at: "2026-08-11T09:10:00Z",
        result: { summary: "Material gaps", findings: [{ id: "C1", description: "Missing <recovery>", recommendation: "Add & test it" }] },
      },
      final_plan: {
        result: { summary: "Final safe plan", checklist: [{ id: "P1", description: "Implement safely", affected_behavior: ["API & UI"], verification: ["node --test <focused>"] }] },
      },
      implementation: {
        execution: {
          checklist: [{ id: "P1", outcome: "deviated", evidence: "Changed <API> only" }],
          verification: ["cargo test --lib: passed"],
        },
      },
      failure: { phase: "implement", message: "Provider <failed> & retained evidence" },
    },
  }, { "0:proposal": true });

  assert.match(html, /goal-implementation-plan-phase">failed/);
  assert.match(html, /Active operation op-implement · process goal-agent-1/);
  assert.match(html, /goal-implementation-plan-checklist/);
  assert.match(html, /goal-implementation-checklist-outcome">deviated/);
  assert.match(html, /Original &lt;proposal&gt;/);
  assert.match(html, /data-plan-history="proposal"[^>]* open/);
  assert.match(html, /Missing &lt;recovery&gt;/);
  assert.match(html, /Provider &lt;failed&gt; &amp; retained evidence/);
  assert.doesNotMatch(html, /<API>|<recovery>|<failed>/);
});
