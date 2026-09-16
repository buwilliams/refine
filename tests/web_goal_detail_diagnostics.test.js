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
  vm.runInContext(namedFunctionSource(commonSource, "governanceReviewStatus"), context);
  vm.runInContext(
    fs.readFileSync(
      path.join(__dirname, "../src/surfaces/web/static/js/features/goals-detail.js"),
      "utf8",
    ),
    context,
  );
  vm.runInContext(fs.readFileSync(path.join(__dirname, "../src/surfaces/web/static/js/features/goals-failures.js"), "utf8"), context);
  vm.runInContext(`
    globalThis.goalDetailDiagnosticsTest = {
      failed: (goal, round, latest = true) => renderRoundFailures(goal, round, latest),
      collect: (goal, round, latest = true) => roundFailureEvidence(goal, round, latest),
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
    governance_rule_actions: [{ action: "repair", reason: "Retain this evidence" }],
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

  assert.doesNotMatch(governance + quality, /<details|<summary/);
  assert.match(governance, /Retain this evidence/);
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
        result: { summary: "Final safe plan", checklist: [{ id: "P1", description: "Implement safely", affected_behavior: ["API & UI"], verification: ["node --test <focused>"] }, { id: "P2", description: "Retain existing behavior" }] },
      },
      implementation: {
        execution: {
          checklist: [{ id: "P1", outcome: "deviated", evidence: "Changed <API> only" }, { id: "P2", outcome: "no_change_needed", evidence: "Already correct" }],
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
  assert.match(html, /status-pill done" data-testid="goal-implementation-checklist-outcome">no_change_needed/);
  assert.match(html, /Original &lt;proposal&gt;/);
  assert.match(html, /data-plan-history="proposal"[^>]* open/);
  assert.match(html, /Missing &lt;recovery&gt;/);
  assert.match(html, /Provider &lt;failed&gt; &amp; retained evidence/);
  assert.doesNotMatch(html, /<API>|<recovery>|<failed>/);
});

test("Governance displays only the AI decision regardless of legacy grades", () => {
  const runtime = goalDetailRuntime();
  for (const legacy of [{}, { product_state: "failed", constitution_state: "failed", meta_rule_state: "failed" }]) {
    const html = runtime.governance({ rule_state: "passed", ...legacy });
    assert.match(html, /class="status-pill done"[^>]*>passed</);
    assert.doesNotMatch(html, /product:|constitution:|meta:|rules:/);
  }
  const failed = runtime.governance({ rule_state: "failed", product_state: "passed", constitution_state: "passed" });
  assert.match(failed, /class="status-pill failed"[^>]*>failed</);
});


test("Skill contract failure leads with the recorded cause and retains invocation context inline", () => {
  const runtime = goalDetailRuntime();
  const message = "Skill output contract failed: Skill invocation abc123 required bindings [default-quality] cannot authorize a gate: Error; Skill result identity does not match this invocation";
  const html = runtime.failure({ status: "failed" }, { failure_message: message });
  assert.match(html, /goal-failure-message">Skill result identity does not match this invocation<\/p>/);
  assert.ok(html.includes(message));
  assert.doesNotMatch(html, /<details|<summary/);
  assert.ok(html.indexOf("goal-failure-message") < html.indexOf("Skill invocation abc123"));
});

test("Failed prefers the recorded integration cause, retains passing gates, and separates unrelated logs", () => {
  const runtime = goalDetailRuntime();
  const round = {
    failure_category: "integration", failure_message: "Candidate branch moved", failure_at: "2026-09-15T12:00:00Z",
    quality_state: "passed", quality_details: { checks: "passed", candidate_commit: "old-tip" },
    rule_state: "passed", governance_details: { verdict: "accepted" },
    latest_error_log: { datetime: "2026-09-16T12:00:00Z", severity: "error", message: "Unrelated later error", details: { token: "later-evidence" } },
  };
  const html = runtime.failed({ status: "failed" }, round);
  assert.ok(html.indexOf("Candidate branch moved") < html.indexOf("Supporting diagnostics"));
  assert.match(html, /integration/);
  assert.match(html, /old-tip/);
  assert.match(html, /accepted/);
  assert.match(html, /later-evidence/);
  assert.doesNotMatch(runtime.failure({ status: "failed" }, round), /later-evidence|Unrelated/);
});

test("Failed gathers Event, Skill contract, plan, and gate diagnostics with recorded context safely", () => {
  const runtime = goalDetailRuntime();
  const round = {
    event_results: { "invocation-1": {
      source: "workflow.governance.enter", generation: 12, state: "error", candidate_commit: "candidate-1",
      results: { validation: { role: "quality", binding_id: "default-quality", invocation_id: "invocation-1",
        outcome: "error", summary: "Skill result identity does not match this invocation",
        evidence: ["Raw <script> & evidence"], artifacts: { stderr: "contract rejected\n<output>" } } },
    } },
    implementation_plan: { state: "failed", failure: { phase: "implement", failed_at: "yesterday", message: "Provider interrupted" } },
    quality_state: "failed", quality_message: "Tests failed", quality_details: { exit_code: 7 },
    rule_state: "failed", governance_rule_actions: [{ action: "repair", reason: "Retain context" }],
  };
  const html = runtime.failed({ status: "implement" }, round);
  for (const value of ["invocation-1", "workflow.governance.enter", "candidate-1", "default-quality", "Provider interrupted", "Tests failed", "Retain context", "yesterday"]) assert.ok(html.includes(value), value);
  assert.match(html, /No current failure is recorded/);
  assert.match(html, /Skill result identity does not match this invocation/);
  assert.match(html, /Raw &lt;script&gt; &amp; evidence/);
  assert.doesNotMatch(html, /<script>|<output>|<details|\[object Object\]/);
});

test("Failed shows unavailable reasons and a distinct empty state without borrowing another Round's status", () => {
  const runtime = goalDetailRuntime();
  assert.match(runtime.failed({ status: "failed" }, {}), /recorded failure reason is unavailable/);
  assert.match(runtime.failed({ status: "done" }, {}), /goal-failed-empty/);
  const oldRound = { logs: [{ severity: "error", message: "Older diagnostic" }] };
  const goal = { status: "failed", rounds: [oldRound, { failure_message: "New failure" }] };
  const oldHtml = runtime.failed(goal, oldRound, false);
  assert.match(oldHtml, /No current failure is recorded/);
  assert.match(oldHtml, /Older diagnostic/);
  assert.doesNotMatch(oldHtml, /Why this Round failed|New failure|recorded failure reason is unavailable/);
  assert.match(runtime.failed(goal, {}, false), /goal-failed-empty/);
  assert.doesNotMatch(runtime.failed(goal, goal.rounds[1]), /Older diagnostic/);
});

test("legacy error logs remain supporting evidence when the recorded cause is unavailable", () => {
  const runtime = goalDetailRuntime();
  const round = { logs: [{ severity: "warn", message: "Legacy provider warning", datetime: "2026-09-15T12:00:00Z", details: { stderr: "<denied>" } }] };
  const html = runtime.failed({ status: "failed" }, round);
  assert.match(html, /recorded failure reason is unavailable/);
  assert.match(html, /supporting evidence only/);
  assert.match(html, /Legacy provider warning/);
  assert.match(html, /&lt;denied&gt;/);
});

test("retry and candidate-refresh evidence remains historical and deduplicates by occurrence and invocation", () => {
  const runtime = goalDetailRuntime();
  const cause = { failure_message: "Earlier contract failure", failure_category: "quality", failure_at: "2026-09-15T12:00:00Z" };
  const occurrence = { generation: 3, round_idx: 0 };
  const log = { id: "log-1", severity: "error", message: "Retained warning", datetime: cause.failure_at, details: { stderr: "Full log" } };
  const event = { state: "error", generation: 3, source: "workflow.quality.enter", results: {
    check: { invocation_id: "old-invocation", binding_id: "check", role: "quality", outcome: "failure", summary: "Old Skill finding" },
  } };
  const prior = { ...cause, workflow_failure_occurrence: occurrence, event_results: { "old-invocation": event }, logs: [log] };
  const round = { failure_history: [{ ...cause, occurrence }], prior_attempts: [prior, prior],
    workflow_candidate_refresh: { previous_gate_evidence: prior },
    logs: [log], latest_error_log: log };
  const result = runtime.collect({ status: "quality" }, round);
  assert.equal(result.failure, null);
  assert.equal(result.diagnostics.length, 0);
  assert.equal(result.history.filter(item => item.failure).length, 1);
  assert.equal(result.history.filter(item => item.title === "Skill diagnostic").length, 1);
  assert.equal(result.history.filter(item => item.title === "Event diagnostic").length, 1);
  assert.equal(result.history.filter(item => item.title === "Warning/error log").length, 1);
  const html = runtime.failed({ status: "done" }, round);
  assert.match(html, /No current failure is recorded/);
  assert.match(html, /Before candidate refresh/);
  assert.match(html, /does not describe the current execution/);
  assert.doesNotMatch(html, /Why this Round failed/);
  // A later invocation with the same message is independent evidence.
  round.event_results = { "new-invocation": { ...event, generation: 4, results: {
    check: { ...event.results.check, invocation_id: "new-invocation" },
  } } };
  assert.equal(runtime.collect({ status: "quality" }, round).diagnostics.filter(item => item.title === "Skill diagnostic").length, 1);
});

test("compact failure history remains visible without prior snapshots and does not decorate current logs", () => {
  const runtime = goalDetailRuntime();
  const round = { failure_history: [{ failure_message: "Historical cause", failure_at: "2026-09-14T12:00:00Z" }],
    logs: [{ severity: "warn", message: "Today's warning", datetime: "2026-09-16T12:00:00Z", details: { unrelated: "latest log detail" } }] };
  const result = runtime.collect({ status: "implement" }, round);
  assert.equal(result.history.length, 1);
  assert.equal(result.diagnostics.length, 1);
  assert.equal(result.history[0].failure.log_details, undefined);
  const html = runtime.failed({ status: "implement" }, round);
  assert.match(html, /Previous execution/);
  assert.match(html, /Historical cause/);
});

test("distinct recorded failure occurrences survive even with the same timestamp and message", () => {
  const runtime = goalDetailRuntime();
  const cause = { failure_message: "Repeated failure", failure_at: "2026-09-15T12:00:00Z" };
  const round = { failure_history: [
    { ...cause, occurrence: { generation: 1, round_idx: 0 } },
    { ...cause, occurrence: { generation: 2, round_idx: 0 } },
  ] };
  const result = runtime.collect({ status: "implement" }, round);
  assert.equal(result.history.length, 2);
  assert.match(runtime.failed({ status: "implement" }, round), /&quot;generation&quot;: 2/);
});

test("partial recorded failure metadata never borrows a later log's reason or timestamp", () => {
  const runtime = goalDetailRuntime();
  for (const recorded of [{ failure_category: "integration" }, { failure_at: "2026-09-15T12:00:00Z" }]) {
    const round = { ...recorded, latest_error_log: { severity: "error", category: "provider",
      datetime: "2026-09-16T12:00:00Z", message: "Unrelated later error", details: { stderr: "later output" } } };
    const summary = runtime.failure({ status: "failed" }, round);
    assert.match(summary, /recorded failure reason is unavailable/);
    assert.doesNotMatch(summary, /Unrelated later error|later output|2026-09-16/);
    assert.match(runtime.failed({ status: "failed" }, round), /Unrelated later error/);
    assert.match(runtime.failed({ status: "failed" }, round), /later output/);
  }
});

test("Round logs without IDs deduplicate across retry snapshots and latest-log projections", () => {
  const runtime = goalDetailRuntime();
  const archived = { datetime: "2026-09-15T12:00:00Z", severity: "error", category: "quality",
    message: "Earlier test failure", details: { exit_code: 1, stderr: "retained output" }, round_idx: 0 };
  const current = { ...archived, datetime: "2026-09-16T12:00:00Z", severity: "warn", message: "Current warning" };
  const round = { prior_attempts: [{ logs: [archived], latest_error_log: structuredClone(archived) }],
    logs: [structuredClone(archived), current], latest_error_log: structuredClone(current), latest_log: structuredClone(current) };
  const result = runtime.collect({ status: "implement" }, round);
  assert.equal(result.failure, null);
  assert.equal(result.history.length, 1);
  assert.equal(result.history[0].message, "Earlier test failure");
  assert.equal(result.history[0].history, "Previous execution");
  assert.equal(result.diagnostics.length, 1);
  assert.equal(result.diagnostics[0].message, "Current warning");
});

test("passing current and retained gates alone leave Failed empty", () => {
  const runtime = goalDetailRuntime();
  const passed = { quality_state: "passed", quality_message: "Tests passed", rule_state: "passed",
    governance_message: "Verified", event_results: { check: { state: "succeeded", generation: 1,
      results: { skill: { outcome: "success", summary: "Accepted" } } } },
    logs: [{ severity: "info", message: "Normal progress" }] };
  const round = { ...passed, prior_attempts: [structuredClone(passed)],
    workflow_candidate_refresh: { previous_gate_evidence: structuredClone(passed) } };
  const html = runtime.failed({ status: "done" }, round);
  assert.match(html, /No failure evidence has been recorded for this Round/);
  assert.doesNotMatch(html, /Why this Round failed|Supporting diagnostics|Historical failures/);
});
