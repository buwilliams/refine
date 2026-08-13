// Browser Node context coordinator. Node IDs from the runtime-local project
// status endpoint are authoritative; registry display names are labels only.

let nodeContextGeneration = 0;
let nodeContextActiveId = "";
let nodeContextAttached = null;
let nodeContextTargetRoot = null;
let nodeContextSwitchPromise = null;
let nodeContextPendingId = "";
let nodeContextReconcileTimer = null;
let nodeContextReconcileSequence = 0;

function captureNodeContextGeneration() {
  return nodeContextGeneration;
}

function isNodeContextGenerationCurrent(generation) {
  return generation === nodeContextGeneration;
}

function nodeContextActiveNodeId() {
  return nodeContextActiveId || state.project?.active_node_id || "";
}

function nodeContextProjectLabel(project, nodes) {
  const id = project?.active_node_id || "";
  const registryNode = (nodes || []).find((node) => node.id === id);
  return registryNode?.display_name
    || (typeof project?.active_node === "string" ? project.active_node : project?.active_node?.display_name)
    || id;
}

function hydrateNodeSelector(project, registry) {
  const selector = document.getElementById("global-node");
  if (!selector) return;
  const attached = project?.attached === true;
  const nodes = (registry?.nodes || project?.nodes || [])
    .filter((node) => !node.archived);
  const activeId = attached ? (project?.active_node_id || registry?.active_node_id || "") : "";
  selector.innerHTML = "";
  if (!attached) {
    const option = document.createElement("option");
    option.value = "";
    option.textContent = "No node";
    selector.appendChild(option);
    selector.value = "";
    selector.disabled = true;
    return;
  }
  for (const node of nodes) {
    const option = document.createElement("option");
    option.value = node.id;
    option.textContent = node.display_name || node.id;
    selector.appendChild(option);
  }
  const activeExists = nodes.some((node) => node.id === activeId);
  if (!activeExists) {
    const option = document.createElement("option");
    option.value = activeId;
    option.textContent = nodeContextProjectLabel(project, nodes) || "No node";
    option.disabled = true;
    selector.appendChild(option);
  }
  selector.value = activeId;
  selector.disabled = nodeContextSwitchPromise !== null || !activeId || !nodes.length;
}

function nodeContextDirtySurfaces() {
  const dirty = [];
  const newGoal = document.querySelector("[data-testid='new-goal-modal']");
  const newGoalPrompt = newGoal?.querySelector("[data-testid='new-goal-prompt']");
  const newGoalPriority = newGoal?.querySelector("[data-testid='new-goal-priority']");
  if (newGoal && ((newGoalPrompt?.value || "").trim() || (newGoalPriority?.value || "low") !== "low")) {
    dirty.push({ label: "New Goal", root: newGoal.closest(".modal-backdrop") || newGoal });
  }
  if (typeof importSessionIsDirty === "function" && importSessionIsDirty()) {
    const root = document.querySelector("[data-testid='import-modal']")?.closest(".modal-backdrop");
    dirty.push({ label: "Import", root });
  }
  if (typeof _targetAppDraftDirty !== "undefined" && _targetAppDraftDirty) {
    dirty.push({ label: "Target App settings", root: document.querySelector("[data-tab-pane='target-app']") });
  }
  if (typeof captureRoundFormDraft === "function" && state.currentGoal
      && captureRoundFormDraft(state.currentGoal)) {
    dirty.push({ label: "Goal Round", root: document.querySelector(".goal-detail-modal") });
  }
  const feature = typeof _featureModalRoot !== "undefined" ? _featureModalRoot : null;
  const featureDirty = feature && (
    feature.dataset.nodeContextDirty === "true"
    || feature._featureComposerHasDraft?.()
    || feature._featureCreateHasDraft?.()
  );
  if (featureDirty) dirty.push({ label: "Feature", root: feature });
  return dirty;
}

async function confirmLocalNodeContextDiscard() {
  const dirty = nodeContextDirtySurfaces();
  if (!dirty.length) return true;
  return modalConfirm(
    `Switching Nodes will discard unsaved ${dirty.map((item) => item.label).join(", ")} work.`,
    {
      title: "Discard Node-scoped work?",
      okLabel: "Discard and switch",
      cancelLabel: "Keep editing",
      danger: true,
      focusCancel: true,
    },
  );
}

async function discardLocalNodeContextSurfaces() {
  if (typeof _discardNewGoalForNodeSwitch === "function") _discardNewGoalForNodeSwitch();
  if (typeof _discardImportForNodeSwitch === "function") await _discardImportForNodeSwitch();
  if (typeof _targetAppDraftDirty !== "undefined") _targetAppDraftDirty = false;
  if (typeof closeGoalDetailModal === "function") closeGoalDetailModal({ navigateAway: true });
  if (typeof closeFeatureModal === "function") closeFeatureModal({ navigateAway: true });
}

function preserveExternalDirtySurfaces(dirty) {
  for (const { root, label } of dirty) {
    if (!root || root.dataset.nodeContextStale === "true") continue;
    root.dataset.nodeContextStale = "true";
    const panel = root.matches?.(".modal") ? root : root.querySelector?.(".modal") || root;
    const warning = document.createElement("div");
    warning.className = "banner warn node-context-stale-warning";
    warning.dataset.testid = "node-context-stale-warning";
    warning.textContent = `${label} belongs to the previous Node. Discard and reopen it before submitting.`;
    panel.prepend(warning);
    root.querySelectorAll("input, select, textarea, button:not(.modal-close)").forEach((control) => {
      control.disabled = true;
    });
  }
}

function closeCleanNodeContextModals() {
  const dirtyRoots = new Set(nodeContextDirtySurfaces().map((item) => item.root).filter(Boolean));
  if (typeof _newGoalModalOpen !== "undefined" && _newGoalModalOpen
      && !dirtyRoots.has(document.querySelector("[data-testid='new-goal-modal']")?.closest(".modal-backdrop"))) {
    if (typeof _discardNewGoalForNodeSwitch === "function") _discardNewGoalForNodeSwitch();
  }
  if (typeof _importModalOpen !== "undefined" && _importModalOpen && !importSessionIsDirty()) {
    if (typeof _discardImportForNodeSwitch === "function") void _discardImportForNodeSwitch();
  }
  if (typeof _goalModalRoot !== "undefined" && _goalModalRoot && !nodeContextDirtySurfaces().some((item) => item.label === "Goal Round")) {
    closeGoalDetailModal({ navigateAway: true });
  }
  if (typeof _featureModalRoot !== "undefined" && _featureModalRoot
      && !dirtyRoots.has(_featureModalRoot)) {
    closeFeatureModal({ navigateAway: true });
  }
}

async function refreshNodeContextRoute({ preservedDirty = [] } = {}) {
  // A modal draft can remain stale above a freshly reconciled underlay. An
  // inline settings draft cannot: refreshing that route would overwrite it.
  if (preservedDirty.some((item) => item.label === "Target App settings")) return;
  const route = state.currentRoute;
  if (route === "dashboard" && typeof refreshDashboard === "function") return refreshDashboard();
  if (route === "goals" && typeof refreshGoalsTable === "function") return refreshGoalsTable();
  if (route === "features" && typeof refreshFeaturesTable === "function") return refreshFeaturesTable();
  if (route === "logs" && typeof loadLogs === "function") return loadLogs();
  if (route === "changes" && typeof loadChanges === "function") return loadChanges();
  if (["settings", "node", "project"].includes(route || "")) {
    return refreshCurrentSettingsSurface({ force: true });
  }
  if (typeof navigate === "function") return navigate();
}

async function applyAuthoritativeNodeContext(project, registry, {
  external = false,
  changed = false,
  surfacesPrepared = false,
} = {}) {
  const allNodes = registry?.nodes || project?.nodes || [];
  const nodes = allNodes.filter((node) => !node.archived);
  const activeId = project?.attached === true ? (project.active_node_id || "") : "";
  const activeLabel = nodeContextProjectLabel(project, nodes);
  const preservedDirty = external && changed ? nodeContextDirtySurfaces() : [];
  nodeContextActiveId = activeId;
  nodeContextAttached = project?.attached === true;
  nodeContextTargetRoot = project?.target_root || "";
  state.project = {
    ...project,
    nodes: allNodes,
    active_node_id: activeId,
    active_node: activeLabel,
  };
  if (changed) {
    nodeContextGeneration += 1;
    invalidateScreenDataCache();
    if (external) preserveExternalDirtySurfaces(preservedDirty);
    else if (!surfacesPrepared) await discardLocalNodeContextSurfaces();
    closeCleanNodeContextModals();
    await refreshNodeScopedState();
    if (typeof refreshTargetAppToggle === "function") await refreshTargetAppToggle();
  }
  hydrateNodeSelector(state.project, { ...registry, nodes: allNodes });
  updateActiveNodeLabel();
  if (changed) await refreshNodeContextRoute({ preservedDirty });
  return state.project;
}

async function reconcileNodeContext({ external = false, duringSwitch = false } = {}) {
  // Activation performs its own authoritative reread. Do not let an unrelated
  // reconciliation race that transition or add another generation change.
  if (nodeContextSwitchPromise && !duringSwitch) return state.project;
  const reconcileSequence = ++nodeContextReconcileSequence;
  const priorId = nodeContextActiveNodeId();
  const priorAttached = nodeContextAttached;
  const priorTargetRoot = nodeContextTargetRoot;
  const [project, registry] = await Promise.all([
    api("GET", "/api/project/status", undefined, { cache: false }),
    api("GET", "/api/nodes", undefined, { cache: false }),
  ]);
  if (reconcileSequence !== nodeContextReconcileSequence) return state.project;
  const nextId = project?.attached === true ? (project.active_node_id || "") : "";
  const nextAttached = project?.attached === true;
  const nextTargetRoot = project?.target_root || "";
  return applyAuthoritativeNodeContext(project, registry, {
    external,
    changed: priorAttached !== null
      && (priorAttached !== nextAttached
        || priorId !== nextId
        || priorTargetRoot !== nextTargetRoot),
  });
}

async function performNodeActivation(nodeId) {
  const currentId = nodeContextActiveNodeId();
  if (!nodeId || nodeId === currentId) {
    hydrateNodeSelector(state.project, { nodes: state.project?.nodes || [] });
    return false;
  }
  if (!await confirmLocalNodeContextDiscard()) {
    hydrateNodeSelector(state.project, { nodes: state.project?.nodes || [] });
    return false;
  }
  // Fence any authority read that began before this user-confirmed transition.
  nodeContextReconcileSequence += 1;
  await discardLocalNodeContextSurfaces();
  await api("POST", "/api/nodes/activate", { node_id: nodeId });
  invalidateScreenDataCache();
  const [project, registry] = await Promise.all([
    api("GET", "/api/project/status", undefined, { cache: false }),
    api("GET", "/api/nodes", undefined, { cache: false }),
  ]);
  if (project?.active_node_id !== nodeId || registry?.active_node_id !== nodeId) {
    throw new Error(`Node activation did not become authoritative for ${nodeId}`);
  }
  await applyAuthoritativeNodeContext(project, registry, { changed: true, surfacesPrepared: true });
  toast(`Node switched to ${nodeContextProjectLabel(project, registry.nodes || [])}`, "info");
  return true;
}

async function activateNodeContext(nodeId) {
  nodeContextPendingId = String(nodeId || "");
  if (nodeContextSwitchPromise) return nodeContextSwitchPromise;
  const selector = document.getElementById("global-node");
  if (selector) selector.disabled = true;
  nodeContextSwitchPromise = (async () => {
    let changed = false;
    while (nodeContextPendingId) {
      const pending = nodeContextPendingId;
      nodeContextPendingId = "";
      changed = await performNodeActivation(pending) || changed;
    }
    return changed;
  })();
  try {
    return await nodeContextSwitchPromise;
  } catch (error) {
    try { await reconcileNodeContext({ duringSwitch: true }); } catch {}
    toast(`Could not switch Node: ${error.message || error}`, "error");
    return false;
  } finally {
    nodeContextSwitchPromise = null;
    hydrateNodeSelector(state.project, { nodes: state.project?.nodes || [] });
  }
}

function nodeContextMutationPath(path) {
  const normalized = String(path || "").split("?", 1)[0];
  return /^\/api\/nodes(?:\/|$)/.test(normalized)
    || /^\/api\/fleet\/nodes(?:\/|$)/.test(normalized)
    || normalized === "/api/apps"
    || /^\/api\/(?:project|apps)\/(?:attach|detach|switch)(?:\/|$)/.test(normalized);
}

function scheduleNodeContextReconciliation(options = {}) {
  if (nodeContextReconcileTimer) clearTimeout(nodeContextReconcileTimer);
  nodeContextReconcileTimer = setTimeout(() => {
    nodeContextReconcileTimer = null;
    reconcileNodeContext(options).catch((error) => {
      toast(`Could not reconcile Node context: ${error.message || error}`, "error");
    });
  }, 75);
}

function handleNodeContextMutationEvent(event) {
  let mutation;
  try { mutation = JSON.parse(event?.data || "{}"); } catch { return; }
  const status = Number(mutation.status || 0);
  if (status < 200 || status >= 300) return;
  if (!nodeContextMutationPath(mutation.path)) return;
  if (nodeContextSwitchPromise) return;
  scheduleNodeContextReconciliation({ external: true });
}

document.addEventListener("change", (event) => {
  if (event.target?.id === "global-node") void activateNodeContext(event.target.value);
});
