// ---- Shared Dashboard / Goals node-scope navigation ------------------------

// Dashboard and Goals both expose current/all node scope, but their defaults
// differ: Dashboard defaults to current while Goals treats an absent node query
// as all. Keep the URL as the source of truth and translate only those two
// shared concepts when navigating between the surfaces. A named Goals node is
// deliberately not mapped onto Dashboard.
function sharedNodeScopeFromHash(hash = location.hash) {
  const raw = String(hash || "#/");
  const [path, query = ""] = raw.split("?", 2);
  const params = new URLSearchParams(query);
  if (path === "#/" || path === "#/dashboard") {
    return params.get("node") === "all" ? "all" : "current";
  }
  if (path === "#/goals") {
    const node = params.get("node") || "all";
    return node === "current" || node === "all" ? node : null;
  }
  return null;
}

function nodeScopeSurfaceHash(surface, scope) {
  if (surface === "dashboard") return scope === "all" ? "#/?node=all" : "#/";
  if (surface === "goals") return `#/goals?node=${scope === "current" ? "current" : "all"}`;
  return "#/";
}

function nodeScopeNavigationHash(destinationHash, sourceHash = location.hash) {
  const scope = sharedNodeScopeFromHash(sourceHash);
  if (!scope) return destinationHash;
  if (destinationHash === "#/" || destinationHash === "#/dashboard") {
    return nodeScopeSurfaceHash("dashboard", scope);
  }
  if (destinationHash === "#/goals") {
    return nodeScopeSurfaceHash("goals", scope);
  }
  return destinationHash;
}

function syncNodeScopeNavigation(hash = location.hash) {
  $$('[data-node-scope-destination]').forEach((link) => {
    const surface = link.dataset.nodeScopeDestination;
    const baseHash = surface === "goals" ? "#/goals" : "#/";
    link.setAttribute("href", nodeScopeNavigationHash(baseHash, hash));
  });
}
