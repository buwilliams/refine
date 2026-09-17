// ---- Router -----------------------------------------------------------------

// `goals_detail` and `features_detail` are handled directly in `navigate()`
// because they open modals on top of the current screen rather than replacing
// `#main`.
const routes = {
  dashboard: renderDashboard,
  planning: renderPlanning,
  features: renderFeaturesList,
  features_new: renderFeatureNew,
  goals: renderGoalsList,
  goals_new: renderGoalNew,
  goals_import: renderGoalImport,
  goals_plan: renderGoalPlan,
  changes: renderChanges,
  settings: renderSettings,
  control: renderControl,
};

function parseHash() {
  const raw = location.hash.slice(1) || "/";
  // "/" → dashboard, "/goals" → list, "/goals/<id>" → detail
  // Strip the query string (e.g. "?status=review") before path parsing;
  // views that care about query params read them off location.hash directly.
  const path = raw.split("?", 1)[0];
  const parts = path.split("/").filter(Boolean);
  if (parts[0] === "windows" && parts[1]) {
    try { return { route: "window", id: decodeURIComponent(parts[1]) }; }
    catch { return { route: "dashboard" }; }
  }
  if (parts.length === 0) return { route: "dashboard" };
  if (parts[0] === "planning") return { route: "planning" };
  if (parts[0] === "goals") {
    if (parts.length === 1) return { route: "goals" };
    if (parts[1] === "new") return { route: "goals_new" };
    if (parts[1] === "plan") return { route: "goals_plan" };
    if (parts[1] === "import") return { route: "goals_import" };
    return { route: "goals_detail", id: parts[1] };
  }
  if (parts[0] === "features") {
    if (parts.length === 1) return { route: "features" };
    if (parts[1] === "new") return { route: "features_new" };
    return { route: "features_detail", id: parts[1] };
  }
  if (parts[0] === "chat") return { route: "chat_redirect" };
  if (parts[0] === "logs") return { route: "logs_redirect" };
  if (parts[0] === "control") return { route: "control", tab: "processes" };
  if (parts[0] === "changes") return { route: "changes" };
  if (["system", "settings", "node", "governance", "project"].includes(parts[0])) {
    const retired = ["governance", "quality", "guidance", "events", "releases"];
    let tab = parts[1] || (["governance", "project"].includes(parts[0]) ? "skills" : "application");
    if (["processes", "agents", "system"].includes(tab) || (parts[0] === "system" && !parts[1])) {
      history.replaceState(null, "", "#/control");
      return { route: "control", tab: "processes" };
    }
    if (retired.includes(tab)) tab = "skills";
    if (parts[0] !== "settings" || tab !== parts[1]) history.replaceState(null, "", `#/settings/${tab}`);
    return { route: "settings", tab };
  }
  return { route: "dashboard" };
}

function navigate() {
  const r = parseHash();
  const destinationHash = location.hash || "#/";
  if (
    r.route !== "goals_new" &&
    typeof guardNewGoalNavigation === "function" &&
    guardNewGoalNavigation({ destinationHash, continueNavigation: navigate })
  ) {
    return;
  }
  if (r.route === "logs_redirect") {
    const params = new URLSearchParams(location.hash.split("?")[1] || "");
    const goalId = params.get("goal_id");
    if (goalId) openGoalLogTail({ goalId }); else openSystemLogs();
    return;
  }
  if (r.route === "chat_redirect") {
    // Legacy chat links now resolve to an agent window.
    const hashQs = new URLSearchParams(location.hash.split("?")[1] || "");
    const goalId = hashQs.get("goal") || null;
    openAgentDock(goalId ? { goalId } : {});
    return;
  }
  if (r.route === "window") {
    if (_goalModalRoot) closeGoalDetailModal({ navigateAway: false });
    if (_featureModalRoot) closeFeatureModal({ navigateAway: false });
  }
  if (typeof routeWorkspace === "function") {
    const handled = routeWorkspace(r, destinationHash);
    if (handled) return handled;
  }
  // Leaving the Goals list forgets in-memory bulk-selection exceptions on
  // purpose — a fresh visit starts with all matching Goals selected again.
  const prevRoute = state.currentRoute;
  if (prevRoute === "goals" && r.route !== "goals") {
    resetGoalsSelection();
  }
  if (prevRoute === "features" && r.route !== "features") {
    resetFeaturesSelection();
  }
  if (r.route === "goals_detail") {
    // Goal detail is now a modal layered on top of the current screen, so
    // the user keeps their underlying context (Dashboard, Goals list, etc.)
    // and dismissing returns them to where they were. We don't touch
    // `#main` — whatever's there stays. If `#main` is empty (cold-load
    // deep link), open the dashboard underneath as the natural landing.
    //
    // Refresh the underlay hash from the URL we navigated AWAY from on
    // this hashchange — but only if it wasn't another goal-detail URL
    // (modal-to-modal swaps shouldn't clobber the true underlay).
    try {
      const prevHash = new URL(_prevHashURL).hash || "#/";
      if (!/^#\/goals\/[^/]+/.test(prevHash) || /^#\/goals\/(new|plan|import)/.test(prevHash)) {
        state.underlayHash = prevHash;
      }
    } catch { /* keep prior state.underlayHash */ }
    state.currentRoute = "goals_detail";
    state.currentGoal = r.id;
    syncNodeScopeNavigation(state.underlayHash);
    highlightNav("goals");
    openGoalDetailModal(r.id);
    return;
  }

  if (r.route === "features_detail") {
    try {
      const prevHash = new URL(_prevHashURL).hash || "#/features";
      const fromFeatureDetail = /^#\/features\/[^/]+/.test(prevHash) && !/^#\/features\/new/.test(prevHash);
      const fromGoalDetail = /^#\/goals\/[^/]+/.test(prevHash) && !/^#\/goals\/(new|plan|import)/.test(prevHash);
      if (!fromFeatureDetail && !fromGoalDetail) {
        state.underlayHash = prevHash;
      }
    } catch { /* keep prior state.underlayHash */ }
    state.currentRoute = "features_detail";
    state.currentGoal = null;
    highlightNav("features");
    if (_goalModalRoot) closeGoalDetailModal({ navigateAway: false });
    if (_featureModalRoot) closeFeatureModal({ navigateAway: false });
    openFeatureDetailModal(r.id);
    return;
  }

  // Leaving a Goal detail modal — close it (without rewriting the hash,
  // since we're already moving to a different one).
  if (_goalModalRoot) closeGoalDetailModal({ navigateAway: false });
  if (_featureModalRoot) closeFeatureModal({ navigateAway: false });
  syncNodeScopeNavigation(destinationHash);

  if (
    prevRoute === r.route &&
    (r.route === "control" || r.route === "settings" || r.route === "node" || r.route === "project")
  ) {
    state.currentRoute = r.route;
    state.currentGoal = null;
    state.underlayHash = location.hash || "#/";
    highlightNav(r.route);
    if (
      typeof setSettingsTab === "function" &&
      typeof refreshSettingsTab === "function" &&
      typeof normalizeSettingsTab === "function" &&
      typeof readSettingsTab === "function"
    ) {
      const slug = normalizeSettingsTab(r.tab) || readSettingsTab();
      setSettingsTab(slug);
      refreshSettingsTab(slug).catch(showActionError);
      return;
    }
  }

  state.currentRoute = r.route;
  state.currentGoal = r.id || null;
  state.underlayHash = location.hash || "#/";
  highlightNav(r.route);
  const fn = routes[r.route];
  if (fn) fn(r);
  else $("#main").innerHTML = "<p>Not found</p>";
}

function highlightNav(route) {
  if (typeof syncWorkspaceVisibility === "function") syncWorkspaceVisibility();
  for (const a of $$(".nav a")) {
    const r = a.dataset.route;
    a.classList.toggle("active",
      r === route ||
      (r === "goals" && route.startsWith("goals")) ||
      (r === "features" && route.startsWith("features")));
    if (a.dataset.planningNavBoard) a.classList.toggle("active", route === "planning" && a.dataset.planningNavBoard === (new URLSearchParams(location.hash.split("?")[1] || "").get("board") || planningBoardId));
    if (a.closest("#planning-board-options")) a.classList.remove("active");
    if (a.classList.contains("active")) a.setAttribute("aria-current", "page");
    else a.removeAttribute("aria-current");
  }
}

// Capture the URL we navigated FROM so the goal-detail modal can return
// the user to their actual prior view — including any filter params the
// Goals list applied via `history.replaceState` (which doesn't fire
// `hashchange`). `navigate()` reads this only when transitioning into
// the `goals_detail` route.
let _prevHashURL = location.href;
window.addEventListener("hashchange", (e) => {
  try { _prevHashURL = e.oldURL || location.href; }
  catch { _prevHashURL = location.href; }
  const pending = navigate();
  if (pending?.catch) pending.catch(showActionError);
});
