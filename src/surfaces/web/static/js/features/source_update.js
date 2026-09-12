// Source update action in Settings.

let _sourceUpdateNavRequest = null;
let _sourceUpdateNavSnapshot = null;
let _sourceUpdateCheckSnapshot = null;
let _sourceUpdateNavDiscoverable = false;

function shortSourceCommit(commit) {
  return commit ? String(commit).slice(0, 12) : "unknown";
}

function sourcePromotionActiveOperation(source = {}) {
  const operation = source.operation || null;
  return operation && ["queued", "running"].includes(operation.status) ? operation : null;
}

function sourcePromotionBlockers(source = {}) {
  // A dirty checkout is not a blocker: queueing an update stashes and reports
  // uncommitted work automatically.
  const blockers = [];
  if (!source.fast_forward) blockers.push("upstream is not a fast-forward");
  if (!source.update_available) blockers.push("already at the fetched source commit");
  const operation = sourcePromotionActiveOperation(source);
  if (operation) blockers.push(`promotion ${operation.id} is ${operation.status}`);
  return blockers;
}

function sourcePromotionIsReady(source = {}) {
  return !!source.fast_forward && !!source.update_available
    && !sourcePromotionActiveOperation(source);
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
  const updateAction = sourceUpdate.state === "available"
    || (["failed", "interrupted"].includes(sourceUpdate.state) && sourceUpdate.update_available);
  updateSourceUpdateNavAction(button, updateAction
    ? "Update Refine"
    : (["updating", "queued", "running"].includes(sourceUpdate.state)
      ? "Updating Refine"
      : "Check for updates"));
  updateSourceUpdateNavLabel(button, button.title);
  button.setAttribute("aria-label", button.title);
}

function handleSourcePromotionSseEvent(payload = {}) {
  const operation = payload.operation || null;
  if (!operation?.id) return;
  const source = { ...(_sourceUpdateNavSnapshot || {}), operation };
  _sourceUpdateNavSnapshot = source;
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
      ? `Update running Refine to ${shortSourceCommit(_sourceUpdateNavSnapshot.available_commit)}`
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
        { cache: false, recordError: fetchRemote },
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
    updateSourceUpdateNavAction(button, "Updating Refine");
    toast("Refine update Agent queued; this page will reconnect after restart", "info");
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

