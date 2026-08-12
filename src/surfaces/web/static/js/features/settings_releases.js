// ---- System / Releases ------------------------------------------------------

let _releasePlan = null;
let _sourceUpdateNavRequest = null;
let _sourceUpdateNavSnapshot = null;
let _sourceUpdateCheckSnapshot = null;
let _sourceUpdateNavDiscoverable = false;

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
    <section class="settings-section" data-testid="source-upgrade-section">
      <h3>Upgrade</h3>
      <p class="muted small" style="margin-top:0">
        Check and promote the configured upstream source separately from published release updates.
        The upgrade Agent preserves workflow admission intent, drains managed work, builds before shutdown, and uses the shared restart-safe promotion and rollback authority.
      </p>
      <div id="source-promotion-status" aria-live="polite" aria-busy="true">
        <p class="muted">Loading source checkout…</p>
      </div>
      <div class="actions settings-section-actions">
        <button class="secondary" type="button" id="source-promotion-check" data-testid="source-promotion-check">
          Check for source updates
        </button>
        <button type="button" id="source-promotion-promote" data-testid="source-promotion-promote" disabled>
          Upgrade Refine
        </button>
      </div>
    </section>`;
}

function shortSourceCommit(commit) {
  return commit ? String(commit).slice(0, 12) : "unknown";
}

function sourcePromotionActiveOperation(source = {}) {
  const operation = source.operation || null;
  return operation && ["queued", "running"].includes(operation.status) ? operation : null;
}

function sourcePromotionBlockers(source = {}) {
  const blockers = [];
  if (!source.clean) blockers.push("checkout has uncommitted changes");
  if (!source.fast_forward) blockers.push("upstream is not a fast-forward");
  if (!source.update_available) blockers.push("already at the fetched source commit");
  const operation = sourcePromotionActiveOperation(source);
  if (operation) blockers.push(`promotion ${operation.id} is ${operation.status}`);
  return blockers;
}

function sourcePromotionIsReady(source = {}) {
  return !!source.clean && !!source.fast_forward && !!source.update_available
    && !sourcePromotionActiveOperation(source);
}

function renderSourcePromotionStatus(source = {}, check = _sourceUpdateCheckSnapshot || {}) {
  const operation = source.operation || null;
  const blockers = sourcePromotionBlockers(source);
  const operationClass = operation?.status === "failed" ? "error" : "muted";
  return `
    <dl class="source-promotion-facts">
      <div><dt>Checkout</dt><dd><code title="${htmlEscape(source.checkout_path || "")}">${htmlEscape(source.checkout_path || "unknown")}</code></dd></div>
      <div><dt>Current commit</dt><dd><code title="${htmlEscape(source.current_commit || "")}">${htmlEscape(shortSourceCommit(source.current_commit))}</code></dd></div>
      <div><dt>Upstream</dt><dd><code>${htmlEscape(`${source.remote || "unknown"}/${source.branch || "unknown"}`)}</code></dd></div>
      <div><dt>Available commit</dt><dd><code title="${htmlEscape(source.available_commit || "")}">${htmlEscape(shortSourceCommit(source.available_commit))}</code></dd></div>
      <div><dt>Check freshness</dt><dd>${htmlEscape(check.in_flight ? "checking" : (check.freshness || "unknown"))}</dd></div>
      <div><dt>Last successful check</dt><dd>${htmlEscape(check.last_successful_check_at || "never")}</dd></div>
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
        : (source.update_available
          ? `Ready for the upgrade Agent. ${htmlEscape((source.active_work || []).length
            ? "It will pause admission and wait for preserved managed work to settle."
            : "The runtime is already settled.")}`
          : "Refine is current at the last fetched source identity.")}
    </p>`;
}

function applySourcePromotionStatus(source, checkState = _sourceUpdateCheckSnapshot || {}) {
  const root = document.getElementById("source-promotion-status");
  if (!root) return;
  root.setAttribute("aria-busy", "false");
  _sourceUpdateCheckSnapshot = checkState;
  renderInto(root, renderSourcePromotionStatus(source, checkState));
  const activeOperation = sourcePromotionActiveOperation(source);
  const promotable = sourcePromotionIsReady(source);
  const promote = document.getElementById("source-promotion-promote");
  const checkButton = document.getElementById("source-promotion-check");
  if (promote) promote.disabled = !promotable;
  if (checkButton) checkButton.disabled = !!activeOperation || !!checkState.in_flight;
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
    applySourcePromotionStatus(result.source || {}, result.source_check || {});
    applySourceUpdateNavStatus(result);
  } catch (error) {
    if (!document.getElementById("source-promotion-status")) return;
    root.setAttribute("aria-busy", "true");
    renderInto(root, `<p class="muted small">${quiet
      ? "Refine is restarting; reconnecting to source-promotion state…"
      : htmlEscape(error.message || "Source checkout status is unavailable")}</p>`);
  }
}

function updateSourceUpdateNavLabel(button, label) {
  const status = button?.querySelector(".nav-source-update-status");
  if (status) status.textContent = label;
}

function updateSourceUpdateNavAction(button, label) {
  const action = button?.querySelector(".nav-source-update-action");
  if (action) action.textContent = label;
}

function applySourceUpdateNavStatus(result = {}) {
  const button = document.getElementById("btn-source-update");
  if (!button) return;
  const sourceUpdate = result.source_update || {};
  _sourceUpdateNavDiscoverable = sourceUpdate.visible === true;
  _sourceUpdateNavSnapshot = result.source || null;
  _sourceUpdateCheckSnapshot = result.source_check || _sourceUpdateCheckSnapshot;
  button.hidden = !_sourceUpdateNavDiscoverable;
  if (button.hidden) {
    button.disabled = true;
    button.dataset.state = "hidden";
    return;
  }

  button.disabled = sourceUpdate.enabled !== true;
  button.dataset.updateAvailable = sourceUpdate.update_available ? "true" : "false";
  button.dataset.state = sourceUpdate.state || "unavailable";
  button.title = sourceUpdate.title || "Refine source update status is unavailable";
  const upgradeAction = sourceUpdate.state === "available"
    || (["failed", "interrupted"].includes(sourceUpdate.state) && sourceUpdate.update_available);
  updateSourceUpdateNavAction(button, upgradeAction
    ? "Upgrade Refine"
    : (["updating", "queued", "running"].includes(sourceUpdate.state)
      ? "Upgrading Refine"
      : "Check for updates"));
  updateSourceUpdateNavLabel(button, button.title);
  button.setAttribute("aria-label", button.title);
}

function handleSourcePromotionSseEvent(payload = {}) {
  const operation = payload.operation || null;
  if (!operation?.id) return;
  const source = { ...(_sourceUpdateNavSnapshot || {}), operation };
  _sourceUpdateNavSnapshot = source;
  if (document.getElementById("source-promotion-status")) {
    applySourcePromotionStatus(source);
  }
  const active = ["queued", "running"].includes(operation.status);
  applySourceUpdateNavStatus({
    source,
    source_update: {
      visible: _sourceUpdateNavDiscoverable,
      enabled: !active,
      state: active ? "updating" : (operation.status === "failed" ? "error" : operation.status),
      update_available: !!source.update_available,
      title: operation.error
        ? `${operation.message}: ${operation.error}`
        : operation.message,
    },
  });
  if (!active) refreshSourceUpdateNav({ quiet: true });
}

function handleSourceUpdateCheckSseEvent(payload = {}) {
  _sourceUpdateCheckSnapshot = payload.source_check || _sourceUpdateCheckSnapshot;
  if (!_sourceUpdateNavSnapshot) return;
  if (document.getElementById("source-promotion-status")) {
    applySourcePromotionStatus(_sourceUpdateNavSnapshot, _sourceUpdateCheckSnapshot || {});
  }
  const check = _sourceUpdateCheckSnapshot || {};
  const active = sourcePromotionActiveOperation(_sourceUpdateNavSnapshot);
  const sourceReady = sourcePromotionIsReady(_sourceUpdateNavSnapshot);
  const sourceBlocked = !!_sourceUpdateNavSnapshot.update_available && !sourceReady;
  const state = active ? "updating"
    : (check.in_flight ? "checking"
      : (check.failure ? "error"
        : (check.freshness !== "fresh" ? "stale"
          : (sourceReady ? "available" : (sourceBlocked ? "blocked" : "current")))));
  const title = active?.message || check.failure || (check.in_flight
    ? "Checking the configured Refine upstream"
    : (sourceReady
      ? `Upgrade running Refine to ${shortSourceCommit(_sourceUpdateNavSnapshot.available_commit)}`
      : (sourceBlocked
        ? `Refine source update unavailable: ${sourcePromotionBlockers(_sourceUpdateNavSnapshot).join("; ")}`
        : `Refine is current; last successful check: ${check.last_successful_check_at || "never"}`)));
  applySourceUpdateNavStatus({
    source: _sourceUpdateNavSnapshot,
    source_check: check,
    source_update: {
      visible: true,
      enabled: !active && !check.in_flight && !sourceBlocked,
      state: state || "stale",
      update_available: !!_sourceUpdateNavSnapshot.update_available,
      title,
    },
  });
}

function handleSourceUpdateSseEvent(payload = {}) {
  if (!payload.source || !payload.source_update || !payload.source_check) return;
  applySourceUpdateNavStatus(payload);
  if (document.getElementById("source-promotion-status")) {
    applySourcePromotionStatus(payload.source, payload.source_check);
  }
}

function markSourceUpdateNavUnavailable(error) {
  const button = document.getElementById("btn-source-update");
  if (!button || !_sourceUpdateNavDiscoverable) return;
  button.hidden = false;
  button.disabled = true;
  button.dataset.state = "unavailable";
  button.title = error?.message || "Refine source update status is unavailable";
  updateSourceUpdateNavLabel(button, button.title);
  button.setAttribute("aria-label", button.title);
}

async function refreshSourceUpdateNav({ fetchRemote = false, quiet = false } = {}) {
  const button = document.getElementById("btn-source-update");
  if (!button) {
    resetSourceUpdateNav();
    return null;
  }
  if (_sourceUpdateNavRequest) return _sourceUpdateNavRequest;
  if (!quiet && _sourceUpdateNavDiscoverable) {
    button.hidden = false;
    button.disabled = true;
    button.dataset.state = "checking";
    button.title = "Checking for Refine source updates";
    updateSourceUpdateNavLabel(button, button.title);
  }
  _sourceUpdateNavRequest = (async () => {
    try {
      const result = await api(
        fetchRemote ? "POST" : "GET",
        fetchRemote ? "/api/system/source/check" : "/api/system/source",
        fetchRemote ? {} : undefined,
        { cache: false },
      );
      applySourceUpdateNavStatus(result);
      return result;
    } catch (error) {
      markSourceUpdateNavUnavailable(error);
      return null;
    } finally {
      _sourceUpdateNavRequest = null;
    }
  })();
  return _sourceUpdateNavRequest;
}

function resetSourceUpdateNav() {
  const button = document.getElementById("btn-source-update");
  _sourceUpdateNavDiscoverable = false;
  _sourceUpdateNavSnapshot = null;
  _sourceUpdateCheckSnapshot = null;
  if (!button) return;
  button.hidden = true;
  button.disabled = true;
  button.dataset.state = "hidden";
  updateSourceUpdateNavLabel(button, "Unavailable");
}

async function queueSourcePromotionFromUi() {
  return api("POST", "/api/system/source/promote", {});
}

async function promoteSourceFromNav() {
  const button = document.getElementById("btn-source-update");
  if (!button || button.disabled) return;
  const retryableFailure = ["failed", "interrupted"].includes(button.dataset.state)
    && button.dataset.updateAvailable === "true";
  if (button.dataset.state !== "available" && !retryableFailure) {
    await refreshSourceUpdateNav({ fetchRemote: true });
    return;
  }
  try {
    const result = await queueSourcePromotionFromUi();
    if (!result) return;
    button.disabled = true;
    button.dataset.state = "updating";
    button.title = result.operation?.message || "Source promotion queued";
    updateSourceUpdateNavLabel(button, button.title);
    button.setAttribute("aria-label", button.title);
    updateSourceUpdateNavAction(button, "Upgrading Refine");
    toast("Refine upgrade Agent queued; this page will reconnect after restart", "info");
  } catch (error) {
    toast(error.message || "Source promotion could not start", "error");
    await refreshSourceUpdateNav();
  } finally {
    if (button) button.blur();
  }
}

function initSourceUpdateNav() {
  const button = document.getElementById("btn-source-update");
  if (!button) return;
  if (button.dataset.bound !== "true") {
    button.dataset.bound = "true";
    bindOnce(button, "click", promoteSourceFromNav);
  }
  refreshSourceUpdateNav();
}

function bindSourcePromotionControls() {
  const check = document.getElementById("source-promotion-check");
  const promote = document.getElementById("source-promotion-promote");
  bindOnce(check, "click", () => withButtonBusy(check, "Checking…", async () => {
    await refreshSourcePromotionStatus({ fetchRemote: true });
  }));
  bindOnce(promote, "click", () => withButtonBusy(promote, "Queuing…", async () => {
    try {
      const result = await queueSourcePromotionFromUi();
      if (!result) return;
      const current = await api("GET", "/api/system/source", undefined, { cache: false });
      current.source.operation = result.operation;
      applySourcePromotionStatus(current.source);
      applySourceUpdateNavStatus(current);
      toast("Refine upgrade Agent queued; this page will reconnect after restart", "info");
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

// Latest prepared release, refreshed on every bind. The publish handler is bound
// once and outlives the render that bound it, so it has to read this rather than
// close over the `releases` it was first called with.
let _releasePreparation = null;

function bindSettingsReleasesTab(releases = {}) {
  bindSourcePromotionControls();
  bindOnce($("#release-preview"), "click", previewRelease);
  bindOnce($("#release-prepare"), "click", prepareRelease);
  _releasePreparation = [...(releases.operations || [])].reverse().find((operation) => operation.owner === "release:prepare" && operation.preparation)?.preparation;
  bindOnce($("#release-publish"), "click", () => publishRelease(_releasePreparation));
  $$('[data-release-retry]').forEach((button) => bindOnce(button, "click", async () => {
    const publish = button.dataset.releasePublishRetry === "true";
    if (publish && !confirm("Retry publishing this release? This may create and push a tag and publish externally.")) return;
    await api("POST", `/api/system/releases/${button.dataset.releaseRetry}/retry`, { confirmed: publish });
    await refreshActiveSettingsTab({ force: true });
  }));
}

async function previewRelease() {
  try {
    const response = await api("POST", "/api/system/releases/plan", { bump: $("#release-bump")?.value || "patch" });
    _releasePlan = response.plan;
    renderInto($("#release-plan"), renderReleasePlan(_releasePlan));
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
