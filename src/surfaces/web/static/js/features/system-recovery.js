let systemStateRecovery = newSystemStateRecovery("");

function newSystemStateRecovery(contextKey) {
  return {
    contextKey,
    phase: "idle",
    preview: null,
    authority: "",
    // Contested paths the operator settles on the OPPOSITE side of the chosen
    // authority — the same exceptions `refine sync --path` names.
    exceptions: [],
    confirmedFingerprint: "",
    previewRefreshRequired: false,
    completedFailureSince: "",
    completedHealthRevision: -1,
    error: "",
    result: null,
  };
}

function resetSystemStateRecovery() {
  systemStateRecovery = newSystemStateRecovery("");
}

function systemRecoveryContextKey(d) {
  const health = d?.state_sync_health || {};
  return [
    health.target_root || "",
    health.node_id || d?.active_node_id || "",
    "current",
  ].join("\u0000");
}

// A conflict-shaped failure is a decision waiting for an operator: sync failed
// closed with a stable report id. Other failures (network, lock) have nothing
// an authority decision could settle.
function systemRecoveryEligible(d) {
  return d?.state_sync_health?.status === "failed"
    && !!d.state_sync_health.last_conflict_report_id;
}

// The preview is never an apply token; this local fingerprint only invalidates
// the operator's confirmation checkbox when a refreshed preview shows a
// different divergence.
function systemRecoveryFingerprint(preview) {
  if (!preview) return "";
  return [
    preview.local_state_head || "",
    preview.remote_state_head || "",
    preview.merge_base || "",
    preview.ancestry || "",
    ...(preview.conflicts || []).map((conflict) => conflict.path),
  ].join("\u0000");
}

function systemRecoveryHealthRevision(health) {
  const revision = Number(health?.revision);
  return Number.isFinite(revision) ? revision : -1;
}

function systemRecoverySuccessSuperseded(d) {
  if (!systemRecoveryEligible(d)) return false;
  const health = d.state_sync_health;
  const failureSince = health.failure_since || "";
  if (failureSince && systemStateRecovery.completedFailureSince
      && failureSince !== systemStateRecovery.completedFailureSince) return true;
  return systemRecoveryHealthRevision(health)
    > systemStateRecovery.completedHealthRevision;
}

function systemRecoverySetPreview(preview) {
  const previousFingerprint = systemRecoveryFingerprint(systemStateRecovery.preview);
  const fingerprint = systemRecoveryFingerprint(preview);
  if (previousFingerprint && previousFingerprint !== fingerprint) {
    systemStateRecovery.authority = "";
    systemStateRecovery.exceptions = [];
    systemStateRecovery.confirmedFingerprint = "";
  }
  systemStateRecovery.preview = preview;
  systemStateRecovery.previewRefreshRequired = false;
  systemStateRecovery.phase = "ready";
  systemStateRecovery.error = "";
}

async function reconcileSystemStateRecovery(d) {
  const nodeGeneration = captureNodeContextGeneration();
  const contextKey = systemRecoveryContextKey(d);
  if (systemStateRecovery.contextKey !== contextKey) {
    systemStateRecovery = newSystemStateRecovery(contextKey);
  }
  if (systemStateRecovery.phase === "success") {
    if (!systemRecoverySuccessSuperseded(d)) return;
    systemStateRecovery = newSystemStateRecovery(contextKey);
  }
  if (!systemRecoveryEligible(d)) {
    systemStateRecovery = newSystemStateRecovery(contextKey);
    return;
  }
  if (systemStateRecovery.previewRefreshRequired
      || systemStateRecovery.preview) return;
  systemStateRecovery.phase = "loading";
  try {
    const preview = await api(
      "GET",
      "/api/sync/preview",
      undefined,
      { recordError: false },
    );
    if (!isNodeContextGenerationCurrent(nodeGeneration) || systemStateRecovery.contextKey !== contextKey) return;
    systemRecoverySetPreview(preview);
  } catch (error) {
    if (!isNodeContextGenerationCurrent(nodeGeneration) || systemStateRecovery.contextKey !== contextKey) return;
    systemStateRecovery.phase = "preview_error";
    systemStateRecovery.error = error.message;
  }
}

function systemRecoveryApplyReady() {
  const preview = systemStateRecovery.preview;
  return !!preview
    && !!systemStateRecovery.authority
    && systemStateRecovery.confirmedFingerprint === systemRecoveryFingerprint(preview)
    && ["ready", "apply_error"].includes(systemStateRecovery.phase);
}

// Exceptions only ever name paths the current preview still lists as
// contested, so a preview that no longer contests a path cannot smuggle it
// into the decision.
function systemRecoveryExceptions() {
  const contested = (systemStateRecovery.preview?.conflicts || [])
    .map((conflict) => conflict.path);
  return contested.filter((path) => systemStateRecovery.exceptions.includes(path));
}

function systemRecoveryApplyPayload() {
  return {
    authority: systemStateRecovery.authority,
    paths: systemRecoveryExceptions(),
  };
}

function systemRecoverySelectAuthority(authority) {
  if (!systemStateRecovery.preview || !["live", "remote"].includes(authority)) return;
  systemStateRecovery.authority = authority;
  systemStateRecovery.confirmedFingerprint = "";
  if (systemStateRecovery.phase === "apply_error") {
    systemStateRecovery.phase = "ready";
  }
}

// An exception changes which side a path lands on, so it invalidates the
// confirmation exactly the way choosing an authority does.
function systemRecoveryToggleException(path, excepted) {
  const contested = (systemStateRecovery.preview?.conflicts || [])
    .map((conflict) => conflict.path);
  if (!contested.includes(path)) return;
  const kept = systemStateRecovery.exceptions.filter((entry) => entry !== path);
  systemStateRecovery.exceptions = excepted ? [...kept, path] : kept;
  systemStateRecovery.confirmedFingerprint = "";
}

function systemRecoverySetConfirmed(confirmed, fingerprint) {
  const currentFingerprint = systemRecoveryFingerprint(systemStateRecovery.preview);
  systemStateRecovery.confirmedFingerprint =
    confirmed && fingerprint === currentFingerprint ? currentFingerprint : "";
}

function systemRecoveryHandleConflict(error) {
  const reason = error?.error?.reason || "";
  systemStateRecovery.confirmedFingerprint = "";
  systemStateRecovery.error = error?.message || "Recovery was rejected.";
  if (reason === "state_moved") {
    systemStateRecovery.phase = "stale";
    systemStateRecovery.preview = null;
    systemStateRecovery.authority = "";
    systemStateRecovery.previewRefreshRequired = true;
    return;
  }
  systemStateRecovery.phase = "apply_error";
}

function systemRecoveryField(label, value) {
  if (value === null || value === undefined || value === "") return "";
  return `<div><dt>${htmlEscape(label)}</dt><dd>${htmlEscape(value)}</dd></div>`;
}

function renderSystemRecoveryCounts(preview = {}) {
  return `
    <div class="system-recovery-counts" data-testid="state-recovery-counts">
      ${[["Live pending", (preview.live_pending_paths || []).length],
         ["Local only", (preview.local_paths || []).length],
         ["Remote only", (preview.remote_paths || []).length],
         ["Resolvable", (preview.resolvable_paths || []).length],
         ["Contested", (preview.conflicts || []).length]].map(([label, value]) => `
        <div><strong>${htmlEscape(value ?? 0)}</strong><span>${label}</span></div>`).join("")}
    </div>`;
}

function renderSystemRecoverySuccess(d) {
  const result = systemStateRecovery.result;
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
    <section class="system-state-recovery success" data-testid="state-recovery-success">
      <h3>State recovery completed</h3>
      <p class="metric-good"><strong>State-sync error cleared:</strong> ${healthCleared ? "Yes" : "Awaiting authoritative health refresh"}</p>
      <dl class="system-recovery-evidence">
        ${systemRecoveryField("Authority", recovery.authority)}
        ${systemRecoveryField("Attempts", result.attempts)}
        ${systemRecoveryField("Published remote head", recovery.remote_state_head)}
        ${systemRecoveryField("Local state head", recovery.local_state_head)}
        ${systemRecoveryField("Settled paths", (recovery.settled_paths || []).join(", "))}
        ${systemRecoveryField("Retained refs", (recovery.retained_refs || []).join(", "))}
      </dl>
      <p class="small">${htmlEscape(result.detail || "Recovery completed.")}</p>
    </section>`;
}

function renderSystemStateRecovery(d) {
  if (systemStateRecovery.phase === "success") {
    return renderSystemRecoverySuccess(d);
  }
  if (!systemRecoveryEligible(d)) return "";
  if (systemStateRecovery.phase === "loading" || systemStateRecovery.phase === "idle") {
    return `<section class="system-state-recovery" data-testid="state-recovery-loading"><p class="muted">Loading read-only divergence preview…</p></section>`;
  }
  if (systemStateRecovery.phase === "stale") {
    return `
      <section class="system-state-recovery degraded" data-testid="state-recovery-stale">
        <h3>Divergence preview is stale</h3>
        <p>${htmlEscape(systemStateRecovery.error)}</p>
        <p class="muted small">Refresh the preview, review the changed divergence, choose authority again, and reconfirm before applying.</p>
        <button type="button" data-recovery-refresh>Refresh preview</button>
      </section>`;
  }
  if (!systemStateRecovery.preview) {
    return `
      <section class="system-state-recovery degraded" data-testid="state-recovery-preview-error">
        <h3>Divergence preview unavailable</h3>
        <p>${htmlEscape(systemStateRecovery.error || "The preview could not be loaded.")}</p>
        <button type="button" data-recovery-refresh>Retry preview</button>
      </section>`;
  }

  const preview = systemStateRecovery.preview;
  const fingerprint = systemRecoveryFingerprint(preview);
  const selected = systemStateRecovery.authority;
  const confirmed = systemStateRecovery.confirmedFingerprint === fingerprint;
  const conflicts = preview.conflicts || [];
  const exceptions = systemRecoveryExceptions();
  const opposite = selected === "live" ? "the fleet's" : "this node's";
  const exceptionHint = selected
    ? `Ticked paths are exceptions: they settle on ${opposite} version instead, the same as <code>refine sync --path</code>.`
    : "Choose an authority first; individual paths can then be excepted onto the other side.";
  return `
    <section class="system-state-recovery ${systemStateRecovery.phase === "apply_error" ? "degraded" : ""}"
             data-testid="state-recovery-preview">
      <h3>State sync needs a decision</h3>
      ${preview.decision_question
        ? `<p data-testid="state-recovery-question">${htmlEscape(preview.decision_question)}</p>`
        : `<p>This node and another node changed the same records, and no side can be chosen automatically. Review both sides and deliberately choose which state is authoritative; everything uncontested has already converged deterministically.</p>`}
      ${systemStateRecovery.phase === "apply_error" ? `
        <div class="system-recovery-warning">${htmlEscape(systemStateRecovery.error)}</div>` : ""}
      <dl class="system-recovery-evidence">
        ${systemRecoveryField("Configured remote", preview.configured_remote)}
        ${systemRecoveryField("Classification", preview.ancestry)}
        ${systemRecoveryField("Local state head", preview.local_state_head || "not present")}
        ${systemRecoveryField("Remote state head", preview.remote_state_head)}
        ${systemRecoveryField("Merge base", preview.merge_base)}
        ${systemRecoveryField("Detail", preview.detail)}
      </dl>
      ${renderSystemRecoveryCounts(preview)}
      <div class="system-recovery-conflicts">
        <strong>Contested paths (${conflicts.length})</strong>
        ${conflicts.length
          ? `<ul>${conflicts.map((conflict) => `
              <li>
                <label>
                  <input type="checkbox" data-recovery-exception
                         value="${htmlEscape(conflict.path)}"
                         ${exceptions.includes(conflict.path) ? "checked" : ""}
                         ${selected ? "" : "disabled"}>
                  <code>${htmlEscape(conflict.path)}</code> — ${htmlEscape(conflict.summary || "")}
                </label>
              </li>`).join("")}</ul>
             <p class="muted small" data-testid="state-recovery-exceptions">${exceptionHint}</p>`
          : `<p class="muted small">No contested paths.</p>`}
      </div>
      <fieldset class="system-recovery-authority" data-testid="state-recovery-authority">
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
      <label class="system-recovery-confirmation">
        <input type="checkbox" data-recovery-confirm data-recovery-fingerprint="${htmlEscape(encodeURIComponent(fingerprint))}" ${confirmed ? "checked" : ""} ${selected ? "" : "disabled"}>
        I reviewed this exact preview and confirm the selected authority.
      </label>
      <div class="actions">
        <button type="button" data-recovery-apply ${systemRecoveryApplyReady() ? "" : "disabled"}>
          ${systemStateRecovery.phase === "apply_error" ? "Retry recovery" : systemStateRecovery.phase === "applying" ? "Applying…" : "Apply recovery"}
        </button>
        <button type="button" class="secondary" data-recovery-refresh>Refresh preview</button>
      </div>
    </section>`;
}

function redrawSystemRecovery() {
  if (currentToolbarTab()?.mode === "system") drawToolbar();
}

function systemRecoveryApplyContextCurrent(context) {
  return "current" === context.scope
    && isNodeContextGenerationCurrent(context.nodeGeneration)
    && systemStateRecovery.contextKey === context.contextKey
    && systemRecoveryFingerprint(systemStateRecovery.preview) === context.fingerprint;
}

async function refreshSystemRecoveryPreview() {
  const contextKey = systemStateRecovery.contextKey;
  systemStateRecovery = newSystemStateRecovery(contextKey);
  await refreshToolbarSyncHealth(true);
}

async function applySystemStateRecovery() {
  if (!systemRecoveryApplyReady()) return;
  const payload = systemRecoveryApplyPayload();
  const context = {
    contextKey: systemStateRecovery.contextKey,
    fingerprint: systemRecoveryFingerprint(systemStateRecovery.preview),
    failureSince: toolbarSystemDashboard?.state_sync_health?.failure_since || "",
    healthRevision: systemRecoveryHealthRevision(toolbarSystemDashboard?.state_sync_health),
    scope: "current",
    nodeGeneration: captureNodeContextGeneration(),
  };
  systemStateRecovery.phase = "applying";
  redrawSystemRecovery();
  let result;
  try {
    result = await api("POST", "/api/sync", payload);
  } catch (error) {
    if (!systemRecoveryApplyContextCurrent(context)) return;
    systemRecoveryHandleConflict(error);
    redrawSystemRecovery();
    return;
  }
  if (!systemRecoveryApplyContextCurrent(context)) return;
  systemStateRecovery.phase = "success";
  systemStateRecovery.confirmedFingerprint = "";
  systemStateRecovery.completedFailureSince = context.failureSince;
  systemStateRecovery.completedHealthRevision = Math.max(
    context.healthRevision,
    systemRecoveryHealthRevision(result?.state_sync_health),
  );
  systemStateRecovery.result = result;
  systemStateRecovery.error = "";
  redrawSystemRecovery();
  try {
    await refreshToolbarSyncHealth(true);
  } catch (_) {
    if (systemRecoveryApplyContextCurrent(context)) redrawSystemRecovery();
  }
}

function wireSystemStateRecovery() {
  $$('[name="state-recovery-authority"]').forEach((control) => {
    bindOnce(control, "change", () => {
      systemRecoverySelectAuthority(control.value);
      redrawSystemRecovery();
    });
  });
  $$('[data-recovery-exception]').forEach((control) => {
    bindOnce(control, "change", () => {
      systemRecoveryToggleException(control.value, control.checked);
      redrawSystemRecovery();
    });
  });
  const confirmation = document.querySelector("[data-recovery-confirm]");
  bindOnce(confirmation, "change", () => {
    systemRecoverySetConfirmed(
      confirmation.checked,
      decodeURIComponent(confirmation.dataset.recoveryFingerprint || ""),
    );
    redrawSystemRecovery();
  });
  $$('[data-recovery-refresh]').forEach((button) => {
    bindOnce(button, "click", () => refreshSystemRecoveryPreview());
  });
  bindOnce(document.querySelector("[data-recovery-apply]"), "click", () => {
    applySystemStateRecovery();
  });
}
