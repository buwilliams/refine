// ---- Router -----------------------------------------------------------------

// `gaps_detail` is handled directly in `navigate()` because it opens a
// modal on top of the current screen rather than replacing `#main`.
const routes = {
  dashboard: renderDashboard,
  gaps: renderGapsList,
  gaps_new: renderGapNew,
  gaps_import: renderGapImport,
  logs: renderLogs,
  changes: renderChanges,
  settings: renderSettings,
};

function parseHash() {
  const raw = location.hash.slice(1) || "/";
  // "/" → dashboard, "/gaps" → list, "/gaps/<id>" → detail
  // Strip the query string (e.g. "?status=review") before path parsing;
  // views that care about query params read them off location.hash directly.
  const path = raw.split("?", 1)[0];
  const parts = path.split("/").filter(Boolean);
  if (parts.length === 0) return { route: "dashboard" };
  if (parts[0] === "gaps") {
    if (parts.length === 1) return { route: "gaps" };
    if (parts[1] === "new") return { route: "gaps_new" };
    if (parts[1] === "import") return { route: "gaps_import" };
    return { route: "gaps_detail", id: parts[1] };
  }
  if (parts[0] === "chat") return { route: "chat_redirect" };
  if (parts[0] === "logs") return { route: "logs" };
  if (parts[0] === "changes") return { route: "changes" };
  if (parts[0] === "system" || parts[0] === "settings") return { route: "settings" };
  return { route: "dashboard" };
}

function navigate() {
  const r = parseHash();
  if (r.route === "chat_redirect") {
    // Legacy `#/chat[?gap=...]` deep links now open the dock and bounce to
    // the dashboard so the URL no longer points at a removed screen.
    const hashQs = new URLSearchParams(location.hash.split("?")[1] || "");
    const gapId = hashQs.get("gap") || null;
    openChatDock(gapId ? { gapId } : {});
    location.hash = "#/";
    return;
  }
  // Leaving the Gaps list forgets per-row bulk deselections on purpose —
  // a fresh visit starts with everything selected again.
  const prevRoute = state.currentRoute;
  if (prevRoute === "gaps" && r.route !== "gaps") {
    gapsExcludedIds.clear();
  }
  if (r.route === "gaps_detail") {
    // Gap detail is now a modal layered on top of the current screen, so
    // the user keeps their underlying context (Dashboard, Gaps list, etc.)
    // and dismissing returns them to where they were. We don't touch
    // `#main` — whatever's there stays. If `#main` is empty (cold-load
    // deep link), open the dashboard underneath as the natural landing.
    //
    // Refresh the underlay hash from the URL we navigated AWAY from on
    // this hashchange — but only if it wasn't another gap-detail URL
    // (modal-to-modal swaps shouldn't clobber the true underlay).
    try {
      const prevHash = new URL(_prevHashURL).hash || "#/";
      if (!/^#\/gaps\/[^/]+/.test(prevHash) || /^#\/gaps\/(new|import)/.test(prevHash)) {
        state.underlayHash = prevHash;
      }
    } catch { /* keep prior state.underlayHash */ }
    state.currentRoute = "gaps_detail";
    state.currentGap = r.id;
    highlightNav("gaps");
    openGapDetailModal(r.id);
    return;
  }

  // Leaving a Gap detail modal — close it (without rewriting the hash,
  // since we're already moving to a different one).
  if (_gapModalRoot) closeGapDetailModal({ navigateAway: false });

  state.currentRoute = r.route;
  state.currentGap = r.id || null;
  state.underlayHash = location.hash || "#/";
  highlightNav(r.route);
  const fn = routes[r.route];
  if (fn) fn(r);
  else $("#main").innerHTML = "<p>Not found</p>";
}

function highlightNav(route) {
  for (const a of $$(".nav a")) {
    const r = a.dataset.route;
    a.classList.toggle("active",
      r === route || (r === "gaps" && route.startsWith("gaps")));
  }
}

// Capture the URL we navigated FROM so the gap-detail modal can return
// the user to their actual prior view — including any filter params the
// Gaps list applied via `history.replaceState` (which doesn't fire
// `hashchange`). `navigate()` reads this only when transitioning into
// the `gaps_detail` route.
let _prevHashURL = location.href;
window.addEventListener("hashchange", (e) => {
  try { _prevHashURL = e.oldURL || location.href; }
  catch { _prevHashURL = location.href; }
  navigate();
});
