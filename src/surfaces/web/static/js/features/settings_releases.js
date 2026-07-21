// ---- System / Releases ------------------------------------------------------

let _releasePlan = null;
let _releaseRefreshTimer = null;
let _sourcePromotionPollTimer = null;

function renderSettingsReleasesTab(releases = {}) {
  const operations = [...(releases.operations || [])].reverse();
  const prepared = operations.find((operation) => operation.owner === "release:prepare" && operation.preparation)?.preparation || null;
  return `
    <section class="settings-section" data-testid="release-planner">
      <h3>${renderSettingsGuideLabel("Semantic releases", "release-workflow")}</h3>
      <p class="muted small">Prepare a reviewable release candidate first. Publishing is separate and always asks for explicit confirmation.</p>
      <div class="form-row">
        <label for="release-bump">Version change</label>
        <select id="release-bump" data-testid="release-bump">
          <option value="patch">Patch</option><option value="minor">Minor</option><option value="major">Major</option>
        </select>
      </div>
      <div class="actions">
        <button type="button" class="secondary" id="release-preview" data-testid="release-preview">Preview</button>
        <button type="button" id="release-prepare" data-testid="release-prepare" ${_releasePlan ? "" : "disabled"}>Prepare release</button>
      </div>
      <div id="release-plan" data-testid="release-plan">${renderReleasePlan(_releasePlan)}</div>
    </section>
    ${prepared ? renderPreparedRelease(prepared) : ""}
    <section class="settings-section" data-testid="release-operations">
      <h3>Persisted activity</h3>
      ${operations.length ? operations.map(renderReleaseOperation).join("") : '<p class="muted small">No release operations yet.</p>'}
    </section>
    ${renderSourcePromotionSection()}`;
}

function renderSourcePromotionSection() {
  return `
    <section class="settings-section" data-testid="source-promotion-section">
      <h3>Dogfood source</h3>
      <p class="muted small" style="margin-top:0">
        Check and promote the configured upstream source separately from published release updates.
        Promotion requires a clean checkout, fast-forward ancestry, paused automation with no active work, and a successful candidate build.
      </p>
      <div id="source-promotion-status" aria-live="polite" aria-busy="true">
        <p class="muted">Loading source checkout…</p>
      </div>
      <div class="actions settings-section-actions">
        <button class="secondary" type="button" id="source-promotion-check" data-testid="source-promotion-check">
          Check for source updates
        </button>
        <button type="button" id="source-promotion-promote" data-testid="source-promotion-promote" disabled>
          Promote latest source
        </button>
      </div>
    </section>`;
}

function shortSourceCommit(commit) {
  return commit ? String(commit).slice(0, 12) : "unknown";
}

function renderSourcePromotionStatus(source = {}) {
  const operation = source.operation || null;
  const activeOperation = operation && ["queued", "running"].includes(operation.status);
  const blockers = [];
  if (!source.clean) blockers.push("checkout has uncommitted changes");
  if (!source.fast_forward) blockers.push("upstream is not a fast-forward");
  if (!source.update_available) blockers.push("already at the fetched source commit");
  if ((source.active_work || []).length) blockers.push(...source.active_work);
  if (activeOperation) blockers.push(`promotion ${operation.id} is ${operation.status}`);
  const operationClass = operation?.status === "failed" ? "error" : "muted";
  return `
    <dl class="source-promotion-facts">
      <div><dt>Checkout</dt><dd><code title="${htmlEscape(source.checkout_path || "")}">${htmlEscape(source.checkout_path || "unknown")}</code></dd></div>
      <div><dt>Current commit</dt><dd><code title="${htmlEscape(source.current_commit || "")}">${htmlEscape(shortSourceCommit(source.current_commit))}</code></dd></div>
      <div><dt>Upstream</dt><dd><code>${htmlEscape(`${source.remote || "unknown"}/${source.branch || "unknown"}`)}</code></dd></div>
      <div><dt>Available commit</dt><dd><code title="${htmlEscape(source.available_commit || "")}">${htmlEscape(shortSourceCommit(source.available_commit))}</code></dd></div>
    </dl>
    ${operation ? `
      <p class="small ${operationClass}" data-testid="source-promotion-operation">
        ${htmlEscape(operation.message || operation.stage || operation.status)}
        ${operation.error ? ` — ${htmlEscape(operation.error)}` : ""}
      </p>
      ${operation.recovery ? `<p class="muted small" data-testid="source-promotion-recovery">Recovery: ${htmlEscape(operation.recovery)}</p>` : ""}
    ` : ""}
    <p class="muted small" data-testid="source-promotion-readiness">
      ${blockers.length
        ? `Promotion unavailable: ${htmlEscape(blockers.join("; "))}`
        : "Ready to build, promote, and restart from the fetched source commit."}
    </p>`;
}

function applySourcePromotionStatus(source) {
  const root = document.getElementById("source-promotion-status");
  if (!root) return;
  root.setAttribute("aria-busy", "false");
  root.innerHTML = renderSourcePromotionStatus(source);
  const operation = source?.operation || null;
  const activeOperation = operation && ["queued", "running"].includes(operation.status);
  const promotable = source?.clean && source?.fast_forward && source?.update_available
    && !(source?.active_work || []).length && !activeOperation;
  const promote = document.getElementById("source-promotion-promote");
  const check = document.getElementById("source-promotion-check");
  if (promote) promote.disabled = !promotable;
  if (check) check.disabled = !!activeOperation;
  if (activeOperation) startSourcePromotionPolling();
  else stopSourcePromotionPolling();
}

async function refreshSourcePromotionStatus({ fetchRemote = false, quiet = false } = {}) {
  const root = document.getElementById("source-promotion-status");
  if (!root) return;
  try {
    const result = await api(
      fetchRemote ? "POST" : "GET",
      fetchRemote ? "/api/system/source/check" : "/api/system/source",
      fetchRemote ? {} : undefined,
      { cache: false },
    );
    applySourcePromotionStatus(result.source || {});
  } catch (error) {
    if (!document.getElementById("source-promotion-status")) return;
    root.setAttribute("aria-busy", "true");
    root.innerHTML = `<p class="muted small">${quiet
      ? "Refine is restarting; reconnecting to source-promotion state…"
      : htmlEscape(error.message || "Source checkout status is unavailable")}</p>`;
    if (!quiet) stopSourcePromotionPolling();
  }
}

function startSourcePromotionPolling() {
  if (_sourcePromotionPollTimer) return;
  _sourcePromotionPollTimer = window.setInterval(() => {
    refreshSourcePromotionStatus({ quiet: true });
  }, 1000);
}

function stopSourcePromotionPolling() {
  if (!_sourcePromotionPollTimer) return;
  window.clearInterval(_sourcePromotionPollTimer);
  _sourcePromotionPollTimer = null;
}

function bindSourcePromotionControls() {
  stopSourcePromotionPolling();
  const check = document.getElementById("source-promotion-check");
  const promote = document.getElementById("source-promotion-promote");
  check?.addEventListener("click", () => withButtonBusy(check, "Checking…", async () => {
    await refreshSourcePromotionStatus({ fetchRemote: true });
  }));
  promote?.addEventListener("click", () => withButtonBusy(promote, "Queuing…", async () => {
    const confirmed = window.confirm(
      "Build the fetched source, stop this idle Refine daemon, fast-forward the clean checkout, and restart?",
    );
    if (!confirmed) return;
    try {
      const result = await api("POST", "/api/system/source/promote", {});
      const current = await api("GET", "/api/system/source", undefined, { cache: false });
      current.source.operation = result.operation;
      applySourcePromotionStatus(current.source);
      toast("Source promotion queued; Refine will reconnect after restart", "info");
    } catch (error) {
      toast(error.message || "Source promotion could not start", "error");
      await refreshSourcePromotionStatus();
    }
  }));
  refreshSourcePromotionStatus();
}

function renderReleasePlan(plan) {
  if (!plan) return '<p class="muted small">Choose a semantic increment and preview the proposed release.</p>';
  return `<div class="card" style="margin-top:12px">
    <p><strong>${htmlEscape(plan.current_version)}</strong> → <strong>${htmlEscape(plan.proposed_version)}</strong> <code>${htmlEscape(plan.proposed_tag)}</code></p>
    <p class="muted small">${(plan.completed_goals || []).length} completed Goal(s); ${(plan.changes || []).length} commit(s) since ${htmlEscape(plan.previous_tag || "repository start")}; ${(plan.breaking_changes || []).length} breaking change(s) identified.</p>
    <details><summary>Files and deterministic gates</summary>
      <p class="small">${(plan.version_files || []).concat(plan.documentation_files || []).map(htmlEscape).join(", ")}</p>
      <ul class="small">${(plan.gates || []).map((gate) => `<li><code>${htmlEscape(gate)}</code></li>`).join("")}</ul>
    </details></div>`;
}

function renderPreparedRelease(preparation) {
  const plan = preparation.plan || {};
  const candidate = preparation.candidate_commit || "";
  const publishable = preparation.publishable === true;
  return `<section class="settings-section" data-testid="prepared-release">
    <h3>Preparation Goal · ${htmlEscape(plan.proposed_tag || "release")}</h3>
    <p class="small">Status: <strong>${htmlEscape(preparation.status || "queued")}</strong> · <a href="${htmlEscape(preparation.review_url || "#/goals")}"><code>${htmlEscape(preparation.goal_id || "")}</code></a></p>
    ${preparation.branch ? `<p class="small"><code>${htmlEscape(preparation.branch)}</code>${candidate ? ` at <code>${htmlEscape(candidate.slice(0, 12))}</code>` : ""}</p>` : ""}
    <p class="muted small">The configured agent works in the normal Goal worktree. Review and approve that Goal before publishing synchronized main.</p>
    <button type="button" class="danger" id="release-publish" data-testid="release-publish" ${publishable ? "" : "disabled"}>Publish release…</button>
  </section>`;
}

function renderReleaseOperation(operation) {
  const progress = operation.progress || {};
  const goal = operation.preparation || {};
  const retryable = ["failed", "interrupted", "cancelled"].includes(operation.status) || (operation.owner === "release:prepare" && goal.status === "failed");
  const goalLogs = (goal.rounds || []).flatMap((round) => round.logs || []);
  return `<article class="card" style="margin-top:10px" data-release-operation="${htmlEscape(operation.id)}">
    <p><strong>${htmlEscape(operation.owner === "release:publish" ? "Publish" : "Prepare")}</strong> · ${htmlEscape(operation.status)}</p>
    <p class="muted small">${htmlEscape(progress.message || operation.error?.message || operation.id)}</p>
    ${goal.goal_id ? `<p class="small">Linked Goal: <a href="${htmlEscape(goal.review_url)}"><code>${htmlEscape(goal.goal_id)}</code></a> · ${htmlEscape(goal.status || "queued")}</p>` : ""}
    ${(operation.logs || []).length ? `<details><summary>Release stages</summary><ul class="small">${operation.logs.map((log) => `<li>${htmlEscape(log.message)}</li>`).join("")}</ul></details>` : ""}
    ${goalLogs.length ? `<details><summary>Agent activity and outputs</summary><ul class="small">${goalLogs.map((log) => `<li>${htmlEscape(log.message)}</li>`).join("")}</ul></details>` : ""}
    ${retryable ? `<button type="button" class="secondary" data-release-retry="${htmlEscape(operation.id)}" data-release-publish-retry="${operation.owner === "release:publish"}">Retry / resume</button>` : ""}
  </article>`;
}

function bindSettingsReleasesTab(releases = {}) {
  clearTimeout(_releaseRefreshTimer);
  bindSourcePromotionControls();
  $("#release-preview")?.addEventListener("click", previewRelease);
  $("#release-prepare")?.addEventListener("click", prepareRelease);
  const prepared = [...(releases.operations || [])].reverse().find((operation) => operation.owner === "release:prepare" && operation.preparation)?.preparation;
  $("#release-publish")?.addEventListener("click", () => publishRelease(prepared));
  $$('[data-release-retry]').forEach((button) => button.addEventListener("click", async () => {
    const publish = button.dataset.releasePublishRetry === "true";
    if (publish && !confirm("Retry publishing this release? This may create and push a tag and publish externally.")) return;
    await api("POST", `/api/system/releases/${button.dataset.releaseRetry}/retry`, { confirmed: publish });
    await refreshActiveSettingsTab({ force: true });
  }));
  const activeGoalStates = ["backlog", "todo", "in-progress", "ready-merge", "build", "qa", "review"];
  if ((releases.operations || []).some((operation) => ["pending", "running"].includes(operation.status) || activeGoalStates.includes(operation.preparation?.status))) {
    _releaseRefreshTimer = setTimeout(() => refreshActiveSettingsTab({ force: true }), 1500);
  }
}

async function previewRelease() {
  try {
    const response = await api("POST", "/api/system/releases/plan", { bump: $("#release-bump")?.value || "patch" });
    _releasePlan = response.plan;
    if ($("#release-plan")) $("#release-plan").innerHTML = renderReleasePlan(_releasePlan);
    if ($("#release-prepare")) $("#release-prepare").disabled = false;
  } catch (error) { await showActionError(error); }
}

async function prepareRelease() {
  if (!_releasePlan) return;
  try {
    await api("POST", "/api/system/releases/prepare", { bump: _releasePlan.bump });
    _releasePlan = null;
    await refreshActiveSettingsTab({ force: true });
  } catch (error) { await showActionError(error); }
}

async function publishRelease(preparation) {
  if (!preparation?.publishable || !confirm(`Publish ${preparation.plan?.proposed_tag}? This will create and push the semantic tag and publish the GitHub release. This explicit confirmation applies only to this attempt.`)) return;
  try {
    await api("POST", "/api/system/releases/publish", { preparation_id: preparation.preparation_id, confirmed: true });
    await refreshActiveSettingsTab({ force: true });
  } catch (error) { await showActionError(error); }
}
