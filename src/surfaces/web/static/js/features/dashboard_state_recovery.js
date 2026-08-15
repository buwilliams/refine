const DASHBOARD_MISSING_BASELINE_RECOVERY = "missing_baseline";

let dashboardStateRecovery = newDashboardStateRecovery("");

function newDashboardStateRecovery(contextKey) {
  return {
    contextKey,
    phase: "idle",
    preview: null,
    authority: "",
    confirmedEvidenceId: "",
    reviewedEvidenceId: "",
    previewRefreshRequired: false,
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

function dashboardRecoveryEligible(d) {
  return d?.state_sync_health?.status === "failed"
    && d.state_sync_health.recovery_kind === DASHBOARD_MISSING_BASELINE_RECOVERY;
}

function dashboardRecoverySetPreview(preview) {
  const previousEvidence = dashboardStateRecovery.preview?.evidence_id || "";
  if (previousEvidence && previousEvidence !== preview.evidence_id) {
    dashboardStateRecovery.authority = "";
    dashboardStateRecovery.confirmedEvidenceId = "";
  }
  dashboardStateRecovery.preview = preview;
  dashboardStateRecovery.reviewedEvidenceId = preview.evidence_id;
  dashboardStateRecovery.previewRefreshRequired = false;
  dashboardStateRecovery.phase = "ready";
  dashboardStateRecovery.error = "";
}

async function reconcileDashboardStateRecovery(d) {
  const contextKey = dashboardRecoveryContextKey(d);
  if (dashboardStateRecovery.contextKey !== contextKey) {
    dashboardStateRecovery = newDashboardStateRecovery(contextKey);
  }
  if (!dashboardRecoveryEligible(d)) {
    if (dashboardStateRecovery.phase !== "success") {
      dashboardStateRecovery = newDashboardStateRecovery(contextKey);
    }
    return;
  }
  if (dashboardStateRecovery.phase === "success"
      || dashboardStateRecovery.previewRefreshRequired
      || dashboardStateRecovery.preview) return;
  dashboardStateRecovery.phase = "loading";
  try {
    const preview = await dashboardApi(
      "GET",
      "/api/project/state-recovery/preview",
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
    && dashboardStateRecovery.confirmedEvidenceId === preview.evidence_id
    && ["ready", "git_busy", "apply_error"].includes(dashboardStateRecovery.phase);
}

function dashboardRecoveryApplyPayload() {
  return {
    authority: dashboardStateRecovery.authority,
    preview: dashboardStateRecovery.preview,
  };
}

function dashboardRecoverySelectAuthority(authority) {
  if (!dashboardStateRecovery.preview || !["live", "remote"].includes(authority)) return;
  dashboardStateRecovery.authority = authority;
  dashboardStateRecovery.confirmedEvidenceId = "";
  if (["git_busy", "apply_error"].includes(dashboardStateRecovery.phase)) {
    dashboardStateRecovery.phase = "ready";
  }
}

function dashboardRecoverySetConfirmed(confirmed, evidenceId) {
  const currentEvidence = dashboardStateRecovery.preview?.evidence_id || "";
  dashboardStateRecovery.confirmedEvidenceId = confirmed && evidenceId === currentEvidence
    ? currentEvidence
    : "";
}

function dashboardRecoveryHandleConflict(error) {
  const reason = error?.error?.reason || "";
  dashboardStateRecovery.confirmedEvidenceId = "";
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

function renderDashboardRecoveryCounts(counts = {}) {
  return `
    <div class="dashboard-recovery-counts" data-testid="state-recovery-counts">
      ${[["Live only", counts.live_only], ["Remote only", counts.remote_only],
         ["Differing", counts.differing], ["Equal", counts.equal]].map(([label, value]) => `
        <div><strong>${htmlEscape(value ?? 0)}</strong><span>${label}</span></div>`).join("")}
    </div>`;
}

function renderDashboardRecoverySuccess(d) {
  const result = dashboardStateRecovery.result;
  const preview = dashboardStateRecovery.preview;
  if (!result) return "";
  const dashboardHealth = d?.state_sync_health || {};
  const resultHealth = result.state_sync_health || {};
  const dashboardRevision = Number(dashboardHealth.revision ?? -1);
  const resultRevision = Number(resultHealth.revision ?? -1);
  const currentHealth = resultHealth.status && resultRevision >= dashboardRevision
    ? resultHealth
    : dashboardHealth;
  const healthCleared = !!currentHealth.status && currentHealth.status !== "failed"
    && currentHealth.recovery_kind !== DASHBOARD_MISSING_BASELINE_RECOVERY;
  return `
    <section class="dashboard-state-recovery success" data-testid="state-recovery-success">
      <h3>State recovery completed</h3>
      <p class="metric-good"><strong>State-sync error cleared:</strong> ${healthCleared ? "Yes" : "Awaiting authoritative health refresh"}</p>
      <dl class="dashboard-recovery-evidence">
        ${dashboardRecoveryField("Authority", result.authority)}
        ${dashboardRecoveryField("Baseline created", result.baseline_created ? "yes" : "no")}
        ${dashboardRecoveryField("Published remote head", result.remote_state_head)}
        ${dashboardRecoveryField("Local state head", result.local_state_head)}
        ${dashboardRecoveryField("Recovery audit ref", result.recovery_location)}
        ${dashboardRecoveryField("Recovery manifest", result.manifest_path)}
        ${dashboardRecoveryField("Evidence identity", preview?.evidence_id || dashboardStateRecovery.reviewedEvidenceId)}
      </dl>
      ${renderDashboardRecoveryCounts(result.path_counts)}
      <p class="small">${htmlEscape(result.detail || "Recovery completed.")}</p>
    </section>`;
}

function renderDashboardStateRecovery(d) {
  if (dashboardStateRecovery.phase === "success") {
    return renderDashboardRecoverySuccess(d);
  }
  if (!dashboardRecoveryEligible(d)) return "";
  if (dashboardStateRecovery.phase === "loading" || dashboardStateRecovery.phase === "idle") {
    return `<section class="dashboard-state-recovery" data-testid="state-recovery-loading"><p class="muted">Loading read-only recovery preview…</p></section>`;
  }
  if (dashboardStateRecovery.phase === "stale") {
    return `
      <section class="dashboard-state-recovery degraded" data-testid="state-recovery-stale">
        <h3>Recovery preview is stale</h3>
        <p>${htmlEscape(dashboardStateRecovery.error)}</p>
        <p class="muted small">Refresh the preview, review the changed evidence, choose authority again, and reconfirm before applying.</p>
        <button type="button" data-recovery-refresh>Refresh preview</button>
      </section>`;
  }
  if (!dashboardStateRecovery.preview) {
    return `
      <section class="dashboard-state-recovery degraded" data-testid="state-recovery-preview-error">
        <h3>Recovery preview unavailable</h3>
        <p>${htmlEscape(dashboardStateRecovery.error || "The preview could not be loaded.")}</p>
        <button type="button" data-recovery-refresh>Retry preview</button>
      </section>`;
  }

  const preview = dashboardStateRecovery.preview;
  const selected = dashboardStateRecovery.authority;
  const confirmed = dashboardStateRecovery.confirmedEvidenceId === preview.evidence_id;
  const conflicts = preview.conflicting_paths || [];
  const retrying = dashboardStateRecovery.phase === "git_busy";
  return `
    <section class="dashboard-state-recovery ${retrying || dashboardStateRecovery.phase === "apply_error" ? "degraded" : ""}"
             data-testid="state-recovery-preview" data-recovery-evidence="${htmlEscape(preview.evidence_id)}">
      <h3>Missing state-sync baseline recovery</h3>
      <p>No authority has been inferred. Review both sides and deliberately choose which state is authoritative.</p>
      ${retrying ? `
        <div class="dashboard-recovery-warning" data-testid="state-recovery-git-busy">
          <strong>Git is busy.</strong> ${htmlEscape(dashboardStateRecovery.error)}
          The same preview is retained, but confirmation is required again before retry.
        </div>` : ""}
      ${dashboardStateRecovery.phase === "apply_error" ? `
        <div class="dashboard-recovery-warning">${htmlEscape(dashboardStateRecovery.error)}</div>` : ""}
      <dl class="dashboard-recovery-evidence">
        ${dashboardRecoveryField("Target", preview.target_identity)}
        ${dashboardRecoveryField("Repository identity", preview.repository_identity)}
        ${dashboardRecoveryField("Configured remote", preview.configured_remote)}
        ${dashboardRecoveryField("Local state head", preview.local_state_head || "not present")}
        ${dashboardRecoveryField("Remote state head", preview.remote_state_head)}
        ${dashboardRecoveryField("Baseline", preview.baseline_status)}
        ${dashboardRecoveryField("Live snapshot", preview.live_snapshot)}
        ${dashboardRecoveryField("Remote snapshot", preview.remote_snapshot)}
        ${dashboardRecoveryField("Evidence identity", preview.evidence_id)}
      </dl>
      ${renderDashboardRecoveryCounts(preview.path_counts)}
      <div class="dashboard-recovery-conflicts">
        <strong>Conflicting paths (${conflicts.length} shown${preview.conflicting_paths_truncated ? `, ${htmlEscape(preview.conflicting_paths_truncated)} more` : ""})</strong>
        ${conflicts.length
          ? `<ul>${conflicts.map((path) => `<li><code>${htmlEscape(path)}</code></li>`).join("")}</ul>`
          : `<p class="muted small">No unequal paths.</p>`}
      </div>
      <fieldset class="dashboard-recovery-authority" data-testid="state-recovery-authority">
        <legend>Choose authority</legend>
        <label>
          <input type="radio" name="state-recovery-authority" value="live" ${selected === "live" ? "checked" : ""}>
          <span><strong>Live authority</strong> publishes the reviewed live durable state. Remote-only paths are deleted.</span>
        </label>
        <label>
          <input type="radio" name="state-recovery-authority" value="remote" ${selected === "remote" ? "checked" : ""}>
          <span><strong>Remote authority</strong> replaces live durable state with the reviewed remote snapshot. The pre-recovery live state is preserved at a recovery audit ref.</span>
        </label>
      </fieldset>
      <label class="dashboard-recovery-confirmation">
        <input type="checkbox" data-recovery-confirm data-recovery-evidence="${htmlEscape(preview.evidence_id)}" ${confirmed ? "checked" : ""} ${selected ? "" : "disabled"}>
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
    && dashboardStateRecovery.preview?.evidence_id === context.evidenceId;
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
    evidenceId: dashboardStateRecovery.preview.evidence_id,
    scope: dashboardScopeFromHash(),
    nodeGeneration: captureNodeContextGeneration(),
  };
  dashboardStateRecovery.phase = "applying";
  redrawDashboardRecovery();
  let result;
  try {
    result = await dashboardApi(
      "POST",
      "/api/project/state-recovery/apply",
      payload,
    );
  } catch (error) {
    if (!dashboardRecoveryApplyContextCurrent(context)) return;
    dashboardRecoveryHandleConflict(error);
    redrawDashboardRecovery();
    return;
  }
  if (!dashboardRecoveryApplyContextCurrent(context)) return;
  dashboardStateRecovery.phase = "success";
  dashboardStateRecovery.confirmedEvidenceId = "";
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
      confirmation.dataset.recoveryEvidence || "",
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
