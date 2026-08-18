let dashboardStateRecovery = newDashboardStateRecovery("");

function newDashboardStateRecovery(contextKey) {
  return {
    contextKey,
    phase: "idle",
    preview: null,
    authority: "",
    confirmedFingerprint: "",
    previewRefreshRequired: false,
    completedFailureSince: "",
    completedHealthRevision: -1,
    error: "",
    result: null,
  };
}

function clearDashboardStateRecoveryForRouteChange() {
  dashboardStateRecovery = newDashboardStateRecovery("");
}

function dashboardRecoveryContextKey(d) {
  const health = d?.state_sync_health || {};
  return [
    health.target_root || "",
    health.node_id || d?.active_node_id || "",
    dashboardScopeParam(d),
  ].join("\u0000");
}

// A conflict-shaped failure is a decision waiting for an operator: sync failed
// closed with a stable report id. Other failures (network, lock) have nothing
// an authority decision could settle.
function dashboardRecoveryEligible(d) {
  return d?.state_sync_health?.status === "failed"
    && !!d.state_sync_health.last_conflict_report_id;
}

// The preview is never an apply token; this local fingerprint only invalidates
// the operator's confirmation checkbox when a refreshed preview shows a
// different divergence.
function dashboardRecoveryFingerprint(preview) {
  if (!preview) return "";
  return [
    preview.local_state_head || "",
    preview.remote_state_head || "",
    preview.merge_base || "",
    preview.ancestry || "",
    ...(preview.conflicts || []).map((conflict) => conflict.path),
  ].join("\u0000");
}

function dashboardRecoveryHealthRevision(health) {
  const revision = Number(health?.revision);
  return Number.isFinite(revision) ? revision : -1;
}

function dashboardRecoverySuccessSuperseded(d) {
  if (!dashboardRecoveryEligible(d)) return false;
  const health = d.state_sync_health;
  const failureSince = health.failure_since || "";
  if (failureSince && dashboardStateRecovery.completedFailureSince
      && failureSince !== dashboardStateRecovery.completedFailureSince) return true;
  return dashboardRecoveryHealthRevision(health)
    > dashboardStateRecovery.completedHealthRevision;
}

function dashboardRecoverySetPreview(preview) {
  const previousFingerprint = dashboardRecoveryFingerprint(dashboardStateRecovery.preview);
  const fingerprint = dashboardRecoveryFingerprint(preview);
  if (previousFingerprint && previousFingerprint !== fingerprint) {
    dashboardStateRecovery.authority = "";
    dashboardStateRecovery.confirmedFingerprint = "";
  }
  dashboardStateRecovery.preview = preview;
  dashboardStateRecovery.previewRefreshRequired = false;
  dashboardStateRecovery.phase = "ready";
  dashboardStateRecovery.error = "";
}

async function reconcileDashboardStateRecovery(d) {
  const contextKey = dashboardRecoveryContextKey(d);
  if (dashboardStateRecovery.contextKey !== contextKey) {
    dashboardStateRecovery = newDashboardStateRecovery(contextKey);
  }
  if (dashboardStateRecovery.phase === "success") {
    if (!dashboardRecoverySuccessSuperseded(d)) return;
    dashboardStateRecovery = newDashboardStateRecovery(contextKey);
  }
  if (!dashboardRecoveryEligible(d)) {
    dashboardStateRecovery = newDashboardStateRecovery(contextKey);
    return;
  }
  if (dashboardStateRecovery.previewRefreshRequired
      || dashboardStateRecovery.preview) return;
  dashboardStateRecovery.phase = "loading";
  try {
    const preview = await dashboardApi(
      "GET",
      "/api/sync/preview",
      undefined,
      { recordError: false },
    );
    if (dashboardStateRecovery.contextKey !== contextKey) return;
    dashboardRecoverySetPreview(preview);
  } catch (error) {
    if (dashboardStateRecovery.contextKey !== contextKey) return;
    dashboardStateRecovery.phase = "preview_error";
    dashboardStateRecovery.error = error.message;
  }
}

function dashboardRecoveryApplyReady() {
  const preview = dashboardStateRecovery.preview;
  return !!preview
    && !!dashboardStateRecovery.authority
    && dashboardStateRecovery.confirmedFingerprint === dashboardRecoveryFingerprint(preview)
    && ["ready", "git_busy", "apply_error"].includes(dashboardStateRecovery.phase);
}

function dashboardRecoveryApplyPayload() {
  return { authority: dashboardStateRecovery.authority, paths: [] };
}

function dashboardRecoverySelectAuthority(authority) {
  if (!dashboardStateRecovery.preview || !["live", "remote"].includes(authority)) return;
  dashboardStateRecovery.authority = authority;
  dashboardStateRecovery.confirmedFingerprint = "";
  if (["git_busy", "apply_error"].includes(dashboardStateRecovery.phase)) {
    dashboardStateRecovery.phase = "ready";
  }
}

function dashboardRecoverySetConfirmed(confirmed, fingerprint) {
  const currentFingerprint = dashboardRecoveryFingerprint(dashboardStateRecovery.preview);
  dashboardStateRecovery.confirmedFingerprint =
    confirmed && fingerprint === currentFingerprint ? currentFingerprint : "";
}

function dashboardRecoveryHandleConflict(error) {
  const reason = error?.error?.reason || "";
  dashboardStateRecovery.confirmedFingerprint = "";
  dashboardStateRecovery.error = error?.message || "Recovery was rejected.";
  if (reason === "git_busy") {
    dashboardStateRecovery.phase = "git_busy";
    return;
  }
  if (reason === "stale_preview") {
    dashboardStateRecovery.phase = "stale";
    dashboardStateRecovery.preview = null;
    dashboardStateRecovery.authority = "";
    dashboardStateRecovery.previewRefreshRequired = true;
    return;
  }
  dashboardStateRecovery.phase = "apply_error";
}

function dashboardRecoveryField(label, value) {
  if (value === null || value === undefined || value === "") return "";
  return `<div><dt>${htmlEscape(label)}</dt><dd>${htmlEscape(value)}</dd></div>`;
}

function renderDashboardRecoveryCounts(preview = {}) {
  return `
    <div class="dashboard-recovery-counts" data-testid="state-recovery-counts">
      ${[["Live pending", (preview.live_pending_paths || []).length],
         ["Local only", (preview.local_paths || []).length],
         ["Remote only", (preview.remote_paths || []).length],
         ["Resolvable", (preview.resolvable_paths || []).length],
         ["Contested", (preview.conflicts || []).length]].map(([label, value]) => `
        <div><strong>${htmlEscape(value ?? 0)}</strong><span>${label}</span></div>`).join("")}
    </div>`;
}

function renderDashboardRecoverySuccess(d) {
  const result = dashboardStateRecovery.result;
  if (!result) return "";
  const dashboardHealth = d?.state_sync_health || {};
  const resultHealth = result.state_sync_health || {};
  const dashboardRevision = Number(dashboardHealth.revision ?? -1);
  const resultRevision = Number(resultHealth.revision ?? -1);
  const currentHealth = resultHealth.status && resultRevision >= dashboardRevision
    ? resultHealth
    : dashboardHealth;
  const healthCleared = !!currentHealth.status && currentHealth.status !== "failed";
  const recovery = result.recovery || {};
  return `
    <section class="dashboard-state-recovery success" data-testid="state-recovery-success">
      <h3>State recovery completed</h3>
      <p class="metric-good"><strong>State-sync error cleared:</strong> ${healthCleared ? "Yes" : "Awaiting authoritative health refresh"}</p>
      <dl class="dashboard-recovery-evidence">
        ${dashboardRecoveryField("Authority", recovery.authority)}
        ${dashboardRecoveryField("Attempts", result.attempts)}
        ${dashboardRecoveryField("Published remote head", recovery.remote_state_head)}
        ${dashboardRecoveryField("Local state head", recovery.local_state_head)}
        ${dashboardRecoveryField("Settled paths", (recovery.settled_paths || []).join(", "))}
        ${dashboardRecoveryField("Retained refs", (recovery.retained_refs || []).join(", "))}
      </dl>
      <p class="small">${htmlEscape(result.detail || "Recovery completed.")}</p>
    </section>`;
}

function renderDashboardStateRecovery(d) {
  if (dashboardStateRecovery.phase === "success") {
    return renderDashboardRecoverySuccess(d);
  }
  if (!dashboardRecoveryEligible(d)) return "";
  if (dashboardStateRecovery.phase === "loading" || dashboardStateRecovery.phase === "idle") {
    return `<section class="dashboard-state-recovery" data-testid="state-recovery-loading"><p class="muted">Loading read-only divergence preview…</p></section>`;
  }
  if (dashboardStateRecovery.phase === "stale") {
    return `
      <section class="dashboard-state-recovery degraded" data-testid="state-recovery-stale">
        <h3>Divergence preview is stale</h3>
        <p>${htmlEscape(dashboardStateRecovery.error)}</p>
        <p class="muted small">Refresh the preview, review the changed divergence, choose authority again, and reconfirm before applying.</p>
        <button type="button" data-recovery-refresh>Refresh preview</button>
      </section>`;
  }
  if (!dashboardStateRecovery.preview) {
    return `
      <section class="dashboard-state-recovery degraded" data-testid="state-recovery-preview-error">
        <h3>Divergence preview unavailable</h3>
        <p>${htmlEscape(dashboardStateRecovery.error || "The preview could not be loaded.")}</p>
        <button type="button" data-recovery-refresh>Retry preview</button>
      </section>`;
  }

  const preview = dashboardStateRecovery.preview;
  const fingerprint = dashboardRecoveryFingerprint(preview);
  const selected = dashboardStateRecovery.authority;
  const confirmed = dashboardStateRecovery.confirmedFingerprint === fingerprint;
  const conflicts = preview.conflicts || [];
  const retrying = dashboardStateRecovery.phase === "git_busy";
  return `
    <section class="dashboard-state-recovery ${retrying || dashboardStateRecovery.phase === "apply_error" ? "degraded" : ""}"
             data-testid="state-recovery-preview">
      <h3>State sync needs a decision</h3>
      ${preview.decision_question
        ? `<p data-testid="state-recovery-question">${htmlEscape(preview.decision_question)}</p>`
        : `<p>This node and another node changed the same records, and no side can be chosen automatically. Review both sides and deliberately choose which state is authoritative; everything uncontested has already converged deterministically.</p>`}
      ${retrying ? `
        <div class="dashboard-recovery-warning" data-testid="state-recovery-git-busy">
          <strong>Git is busy.</strong> ${htmlEscape(dashboardStateRecovery.error)}
          The same preview is retained, but confirmation is required again before retry.
        </div>` : ""}
      ${dashboardStateRecovery.phase === "apply_error" ? `
        <div class="dashboard-recovery-warning">${htmlEscape(dashboardStateRecovery.error)}</div>` : ""}
      <dl class="dashboard-recovery-evidence">
        ${dashboardRecoveryField("Configured remote", preview.configured_remote)}
        ${dashboardRecoveryField("Classification", preview.ancestry)}
        ${dashboardRecoveryField("Local state head", preview.local_state_head || "not present")}
        ${dashboardRecoveryField("Remote state head", preview.remote_state_head)}
        ${dashboardRecoveryField("Merge base", preview.merge_base)}
        ${dashboardRecoveryField("Detail", preview.detail)}
      </dl>
      ${renderDashboardRecoveryCounts(preview)}
      <div class="dashboard-recovery-conflicts">
        <strong>Contested paths (${conflicts.length})</strong>
        ${conflicts.length
          ? `<ul>${conflicts.map((conflict) => `<li><code>${htmlEscape(conflict.path)}</code> — ${htmlEscape(conflict.summary || "")}</li>`).join("")}</ul>`
          : `<p class="muted small">No contested paths.</p>`}
      </div>
      <fieldset class="dashboard-recovery-authority" data-testid="state-recovery-authority">
        <legend>Choose authority</legend>
        <label>
          <input type="radio" name="state-recovery-authority" value="live" ${selected === "live" ? "checked" : ""}>
          <span><strong>Live authority</strong> settles every contested path on this node's version and republishes it to the fleet.</span>
        </label>
        <label>
          <input type="radio" name="state-recovery-authority" value="remote" ${selected === "remote" ? "checked" : ""}>
          <span><strong>Remote authority</strong> settles every contested path on the fleet's version. Displaced local state stays reachable as a merge parent or retained ref.</span>
        </label>
      </fieldset>
      <label class="dashboard-recovery-confirmation">
        <input type="checkbox" data-recovery-confirm data-recovery-fingerprint="${htmlEscape(fingerprint)}" ${confirmed ? "checked" : ""} ${selected ? "" : "disabled"}>
        I reviewed this exact preview and confirm the selected authority.
      </label>
      <div class="actions">
        <button type="button" data-recovery-apply ${dashboardRecoveryApplyReady() ? "" : "disabled"}>
          ${retrying ? "Retry recovery" : dashboardStateRecovery.phase === "applying" ? "Applying…" : "Apply recovery"}
        </button>
        <button type="button" class="secondary" data-recovery-refresh>Refresh preview</button>
      </div>
    </section>`;
}

function redrawDashboardRecovery() {
  if (state.currentRoute !== "dashboard" || !state.dashboard) return;
  drawDashboard(state.dashboard, state.dashboardReviewSnapshot || {});
}

function dashboardRecoveryApplyContextCurrent(context) {
  return state.currentRoute === "dashboard"
    && dashboardScopeFromHash() === context.scope
    && isNodeContextGenerationCurrent(context.nodeGeneration)
    && dashboardStateRecovery.contextKey === context.contextKey
    && dashboardRecoveryFingerprint(dashboardStateRecovery.preview) === context.fingerprint;
}

async function refreshDashboardRecoveryPreview() {
  const contextKey = dashboardStateRecovery.contextKey;
  dashboardStateRecovery = newDashboardStateRecovery(contextKey);
  await refreshDashboard();
}

async function applyDashboardStateRecovery() {
  if (!dashboardRecoveryApplyReady()) return;
  const payload = dashboardRecoveryApplyPayload();
  const context = {
    contextKey: dashboardStateRecovery.contextKey,
    fingerprint: dashboardRecoveryFingerprint(dashboardStateRecovery.preview),
    failureSince: state.dashboard?.state_sync_health?.failure_since || "",
    healthRevision: dashboardRecoveryHealthRevision(state.dashboard?.state_sync_health),
    scope: dashboardScopeFromHash(),
    nodeGeneration: captureNodeContextGeneration(),
  };
  dashboardStateRecovery.phase = "applying";
  redrawDashboardRecovery();
  let result;
  try {
    result = await dashboardApi("POST", "/api/sync", payload);
  } catch (error) {
    if (!dashboardRecoveryApplyContextCurrent(context)) return;
    dashboardRecoveryHandleConflict(error);
    redrawDashboardRecovery();
    return;
  }
  if (!dashboardRecoveryApplyContextCurrent(context)) return;
  dashboardStateRecovery.phase = "success";
  dashboardStateRecovery.confirmedFingerprint = "";
  dashboardStateRecovery.completedFailureSince = context.failureSince;
  dashboardStateRecovery.completedHealthRevision = Math.max(
    context.healthRevision,
    dashboardRecoveryHealthRevision(result?.state_sync_health),
  );
  dashboardStateRecovery.result = result;
  dashboardStateRecovery.error = "";
  redrawDashboardRecovery();
  try {
    await refreshDashboard();
  } catch (_) {
    if (dashboardRecoveryApplyContextCurrent(context)) redrawDashboardRecovery();
  }
}

function wireDashboardStateRecovery() {
  $$('[name="state-recovery-authority"]').forEach((control) => {
    bindOnce(control, "change", () => {
      dashboardRecoverySelectAuthority(control.value);
      redrawDashboardRecovery();
    });
  });
  const confirmation = document.querySelector("[data-recovery-confirm]");
  bindOnce(confirmation, "change", () => {
    dashboardRecoverySetConfirmed(
      confirmation.checked,
      confirmation.dataset.recoveryFingerprint || "",
    );
    redrawDashboardRecovery();
  });
  $$('[data-recovery-refresh]').forEach((button) => {
    bindOnce(button, "click", () => refreshDashboardRecoveryPreview());
  });
  bindOnce(document.querySelector("[data-recovery-apply]"), "click", () => {
    applyDashboardStateRecovery();
  });
}
