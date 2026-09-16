// Failure investigation is a read-only projection of one authored Round.
// Recorded causes are authoritative; logs can only supply supporting context.
function goalFailureEvidence(goal, round) {
  if (!round) return null;
  const recorded = !!(round.failure_message || round.failure_category || round.failure_at);
  if (!recorded && goal?.status !== "failed") return null;
  if (recorded) return {
    category: round.failure_category || "workflow",
    message: round.failure_message || "The recorded failure reason is unavailable.",
    at: round.failure_at || "",
    recorded: true,
    occurrence: round.workflow_failure_occurrence || round.occurrence,
  };
  const log = round.latest_error_log
    || (round.logs || []).filter(log => ["error", "warn", "warning"].includes(log.severity)).at(-1);
  return {
    category: log?.category || "workflow",
    message: log?.message || "The recorded failure reason is unavailable.",
    at: log?.datetime || "",
    log_details: log?.details,
    recorded: false,
  };
}

function renderFailureSummary(goal, round) {
  const failure = goalFailureEvidence(goal, round);
  return failure ? renderFailureEvidence(failure) : "";
}

function renderFailureEvidence(failure, title = "Why this Round failed") {
  // Invocation metadata can bury the explicit contract rejection cause.
  const contractCause = failure.message.startsWith("Skill output contract failed:")
    ? failure.message.match(/;\s*(Skill result[^;]+)$/)?.[1] : null;
  return `<div class="round-diagnostic" data-testid="goal-failure-summary">
    <h3>${htmlEscape(title)}</h3>
    ${!failure.recorded && failure.message !== "The recorded failure reason is unavailable."
      ? '<p>The recorded failure reason is unavailable. Latest warning/error log (supporting evidence only):</p>' : ""}
    <p class="round-failure-reason" data-testid="goal-failure-message">${htmlEscape(contractCause || failure.message)}</p>
    <div class="row" style="gap:8px;flex-wrap:wrap">
      ${failure.category ? `<span class="status-pill failed" data-testid="goal-failure-category">${htmlEscape(failure.category)}</span>` : ""}
      ${failure.at ? `<span class="muted small" data-testid="goal-failure-at">${htmlEscape(fmtTime(failure.at))}</span>` : ""}
    </div>
    ${contractCause ? `<p class="muted small">The Skill result was rejected, so it could not authorize the workflow gate.</p><p class="round-recorded-error">${htmlEscape(failure.message)}</p>` : ""}
    ${failure.occurrence ? `<pre class="round-evidence">${htmlEscape(diagnosticDetailsText({ occurrence: failure.occurrence }))}</pre>` : ""}
    ${failure.log_details ? `<pre class="round-evidence" data-testid="goal-failure-details">${htmlEscape(diagnosticDetailsText(failure.log_details))}</pre>` : ""}
  </div>`;
}

function isFailureDiagnostic(value) {
  return ["failure", "failed", "fail", "error", "rejected", "violation"].includes(String(value || "").toLowerCase());
}

function roundFailureEvidence(goal, round, isLatest) {
  const failure = goalFailureEvidence({ status: isLatest ? goal.status : "" }, round);
  const diagnostics = [];
  const history = [];
  const seen = new Set();
  const sources = roundEvidenceSources(round);
  const failureKey = record => JSON.stringify(["cause", record.failure_category || "workflow",
    record.failure_message || "", record.failure_at || ""]);
  const causeKeys = record => {
    const occurrence = record.workflow_failure_occurrence || record.occurrence;
    return [occurrence ? JSON.stringify(["occurrence", occurrence.generation, occurrence.round_idx]) : failureKey(record)];
  };
  const add = (keys, item, historical = "") => {
    if (keys.some(key => seen.has(key))) return;
    keys.forEach(key => seen.add(key));
    (historical ? history : diagnostics).push({ ...item, history: historical });
  };
  if (failure?.recorded) causeKeys(round).forEach(key => seen.add(key));
  const lastArchivedFailure = sources.flatMap(({ record, history }) => [
    ...(record.failure_history || []).map(entry => entry.failure_at),
    ...(history ? [record.failure_at] : []),
  ]).filter(Boolean).sort().at(-1);

  for (const { record, history: historical } of sources) {
    if (historical && (record.failure_message || record.failure_category || record.failure_at)) {
      add(causeKeys(record), { title: "Recorded failure", failure: goalFailureEvidence({}, record) }, historical);
    }
    for (const [id, event] of Object.entries(record.event_results || {})) {
      const { results = {}, ...context } = event;
      const invocation = event.invocation_id || id;
      if (isFailureDiagnostic(event.state)) {
        add([JSON.stringify(["event", invocation, event.generation])],
          { title: "Event diagnostic", details: { invocation_id: invocation, ...context } }, historical);
      }
      for (const [binding, result] of Object.entries(results)) {
        if (!isFailureDiagnostic(result.outcome)) continue;
        add([JSON.stringify(["skill", result.invocation_id || invocation, result.binding_id || binding])],
          { title: "Skill diagnostic", message: result.summary,
            details: { ...context, invocation_id: invocation, binding_id: binding, ...result } }, historical);
      }
    }
    const plan = record.implementation_plan;
    if (plan?.failure || isFailureDiagnostic(plan?.state)) {
      add([JSON.stringify(["plan", plan.failure, plan.updated_at, plan.active_process])],
        { title: "Implementation plan diagnostic", message: plan.failure?.message,
          details: { phase: plan.phase, state: plan.state, failure: plan.failure,
            updated_at: plan.updated_at, active_process: plan.active_process } }, historical);
    }
    // Passing gates are useful context when a later integration fails. Otherwise
    // collect only gate diagnostics, avoiding an empty Failed tab full of passes.
    for (const step of ["quality", "governance"]) {
      const state = step === "quality" ? record.quality_state : record.rule_state;
      if (!isFailureDiagnostic(state) && !(record.failure_message || (!historical && failure))) continue;
      const details = Object.fromEntries(Object.entries(record).filter(([key]) =>
        key.startsWith(`${step}_`) || (step === "governance" && key === "rule_state")));
      if (!Object.keys(details).length) continue;
      add([JSON.stringify([step, details])], { title: `${step === "quality" ? "Quality" : "Governance"} diagnostics`,
        details }, historical);
    }
  }
  // Retry snapshots and failure_history can describe the same occurrence. Add
  // compact history after snapshots so the richer evidence wins deduplication.
  for (const { record } of sources) {
    for (const entry of (record.failure_history || []).slice().reverse()) {
      if (!(entry.failure_message || entry.failure_category || entry.failure_at)) continue;
      add(causeKeys(entry), { title: "Recorded failure", failure: goalFailureEvidence({}, entry) }, "Previous execution");
    }
  }
  // Read archived logs first so duplicates in the Round's complete log retain
  // their previous-execution label. Latest-log projections may duplicate logs.
  for (const { record, history: historical } of [...sources.filter(source => source.history), sources[0]]) {
    const logs = [...(record.logs || []), record.latest_error_log, record.latest_log].filter(Boolean);
    for (const log of logs) {
      if (!["error", "warn", "warning"].includes(log.severity)) continue;
      const label = historical || (lastArchivedFailure && log.datetime && log.datetime <= lastArchivedFailure ? "Previous execution" : "");
      const key = log.id ? ["log", log.id] : ["log", log.datetime, log.category, log.severity, log.message, log.details];
      add([JSON.stringify(key)], { title: "Warning/error log", message: log.message, details: log }, label);
    }
  }
  return { failure, diagnostics, history };
}

function renderFailureDiagnostic(item) {
  return `<section class="round-failure-diagnostic">
    ${item.history ? `<p class="muted small">${htmlEscape(item.history)}</p>` : ""}
    ${item.failure ? renderFailureEvidence(item.failure, "Recorded failure") : `<h4>${htmlEscape(item.title)}</h4>
      ${item.message ? `<p class="round-recorded-error">${htmlEscape(item.message)}</p>` : ""}`}
    ${item.details ? `<pre class="round-evidence">${htmlEscape(diagnosticDetailsText(item.details))}</pre>` : ""}
  </section>`;
}

function renderRoundFailures(goal, round, isLatest) {
  const { failure, diagnostics, history } = roundFailureEvidence(goal, round, isLatest);
  if (!failure && !diagnostics.length && !history.length) {
    return '<p class="muted" data-testid="goal-failed-empty">No failure evidence has been recorded for this Round.</p>';
  }
  return `<div class="round-failures" data-testid="goal-round-failures">
    ${failure ? renderFailureEvidence(failure) : '<p>No current failure is recorded for this Round.</p>'}
    ${diagnostics.length ? `<section class="round-failure-section"><h3>Supporting diagnostics</h3>
      <p class="muted small">Diagnostic results and warning/error logs are supporting evidence, not proof of the failure cause. Round logs may include previous executions.</p>
      ${diagnostics.map(renderFailureDiagnostic).join("")}</section>` : ""}
    ${history.length ? `<section class="round-failure-section" data-testid="goal-failure-history"><h3>Historical failures and diagnostics</h3>
      <p class="muted small">Retained evidence from previous executions or candidate refreshes does not describe the current execution.</p>
      ${history.map(renderFailureDiagnostic).join("")}</section>` : ""}
  </div>`;
}
