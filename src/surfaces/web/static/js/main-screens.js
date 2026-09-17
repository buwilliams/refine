// One live host per screen. Inactive hosts are detached, so legacy renderers and
// document-wide selectors see only the active #main, including in Settings.
// DOM, handlers and drafts belong to this page session, never browser storage.
const MAIN_DESTINATIONS = {
  dashboard: { label: "Dashboard", hash: "#/", icon: "layout-dashboard" },
  features: { label: "Features", hash: "#/features", icon: "layers" },
  goals: { label: "Goals", hash: "#/goals", icon: "target" },
  changes: { label: "Changes", hash: "#/changes", icon: "git-pull-request" },
  control: { label: "Control", hash: "#/control", icon: "sliders-horizontal" },
  settings: { label: "Settings", hash: "#/settings/application", icon: "settings" },
};
const mainScreens = { open: new Map(), active: null, epoch: 0 };

function mainScreenForHash(hash) {
  return Object.keys(MAIN_DESTINATIONS).find(key => MAIN_DESTINATIONS[key].hash === hash);
}

function initMainScreens() {
  document.getElementById("rail-main-section").addEventListener("click", event => {
    const close = event.target.closest("[data-close-main]");
    if (close) {
      event.preventDefault();
      void closeMainScreen(close.dataset.closeMain);
      return;
    }
    const link = event.target.closest("[data-main-open]");
    if (!link || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    openMainScreen(link.dataset.mainOpen);
  });
}

function openMainScreen(key) {
  const destination = MAIN_DESTINATIONS[key];
  if (!destination) return;
  captureMainScreen();
  const source = mainScreens.active?.hash || state.underlayHash || location.hash;
  const hash = mainScreens.open.get(key)?.hash || nodeScopeNavigationHash(destination.hash, source);
  mainScreens.focusKey = key;
  closeTopbarMenus();
  closeMobileNavigation();
  if (location.hash === hash) {
    mainScreens.focusKey = null;
    document.querySelector(`#rail-main-screens [data-main-open="${key}"]`)?.focus();
    return;
  }
  location.hash = hash;
}

function captureMainScreen(leaving = false) {
  const screen = mainScreens.active;
  if (!screen || state.currentRoute === "window") return;
  // The old URL includes filters changed with replaceState; underlayHash owns
  // modal returns. Never save a modal or tool URL as a screen's address.
  let hash = state.underlayHash || screen.hash;
  if (state.currentRoute === screen.route) {
    try {
      const previous = new URL(_prevHashURL).hash || "#/";
      const current = location.hash || "#/";
      hash = leaving ? previous : current;
    } catch {}
  }
  if (!/^#\/(windows|goals\/[^?]+|features\/[^?]+)/.test(hash)) screen.hash = hash;
  screen.scroll = screen.host.scrollTop;
}

function renderMainNavigation() {
  const root = document.getElementById("rail-main-screens");
  if (!root) return;
  renderInto(root, [...mainScreens.open].filter(([key]) => MAIN_DESTINATIONS[key]).map(([key, screen]) => {
    const destination = MAIN_DESTINATIONS[key];
    const active = state.currentRoute !== "window" && mainScreens.active === screen;
    return `<div class="rail-window-row${active ? " active" : ""}">
      <a class="rail-row${active ? " active" : ""}" href="${htmlEscape(screen.hash)}" data-route="${key}" data-main-open="${key}" data-testid="nav-${key}" title="${destination.label}" ${active ? 'aria-current="page"' : ""}>
        ${railIcon(destination.icon)}<span class="rail-copy rail-window-label">${destination.label}</span>
      </a>
      <button class="rail-window-close" type="button" data-close-main="${key}" aria-label="Close ${destination.label}" title="Close ${destination.label}">${railIcon("x")}</button>
    </div>`;
  }).join(""));
}

function captureMainScreenRequest() {
  const epoch = mainScreens.epoch;
  const hash = location.hash;
  const screen = mainScreens.active;
  return () => epoch === mainScreens.epoch && hash === location.hash && screen === mainScreens.active
    && !screen?.stale && state.currentRoute !== "window";
}

function refreshRetainedMainScreen(screen) {
  if (screen.stale || screen.dirty) return;
  const refresh = {
    dashboard: refreshDashboard, goals: refreshGoalsTable, features: refreshFeaturesTable,
    changes: loadChanges, settings: refreshSettings, control: refreshSettings, planning: refreshPlanning,
  }[screen.route];
  if (refresh) Promise.resolve(refresh()).catch(showActionError);
}

function activateMainScreen(route, hash) {
  // Detail modals keep the existing owner, including when opened from a tool.
  if (["goals_detail", "features_detail"].includes(route.route)) {
    if (!mainScreens.active) {
      activateMainScreen({ route: "dashboard" }, "#/");
      state.currentRoute = "dashboard";
      state.underlayHash = "#/";
      void renderDashboard();
    }
    if (!mainScreens.active.host.isConnected) document.getElementById("main").replaceWith(mainScreens.active.host);
    state.underlayHash = mainScreens.active.hash;
    return false;
  }
  captureMainScreen(true);
  const key = route.route;
  const previous = mainScreens.active;
  let screen = mainScreens.open.get(key);
  const sameAddress = screen?.hash === hash;
  if (previous !== screen || state.currentRoute === "window" || !sameAddress) mainScreens.epoch++;
  if (!screen) {
    const host = document.getElementById("main").cloneNode(false);
    host.hidden = false;
    screen = {
      route: key, hash, host, scroll: 0, stale: false, edits: new Map(),
      get dirty() {
        return mainScreenHasDraft(this);
      },
    };
    if (key === "settings") {
      const track = event => {
        const control = event.target;
        if (!control.matches("input, select, textarea")) return;
        if (!screen.edits.has(control)) screen.edits.set(control,
          control.dataset.settingsSavedValue ?? control.defaultValue ?? settingsControlValue(control));
      };
      host.addEventListener("focusin", track);
      host.addEventListener("input", track);
      host.addEventListener("change", track);
    }
    mainScreens.open.set(key, screen);
  }
  if (document.getElementById("main") !== screen.host) {
    document.getElementById("main").replaceWith(screen.host);
  }
  mainScreens.active = screen;
  screen.hash = hash;
  // Empty hosts require the renderer's scaffold, even on the same route after
  // a context change. The router's Settings-tab shortcut assumes it exists.
  if (!screen.host.children.length && state.currentRoute === key) state.currentRoute = null;
  // Existing Settings tabs share a host and retain their existing routing.
  if (key === "settings" && screen.host.children.length && !sameAddress) state.currentRoute = key;
  const retained = (MAIN_DESTINATIONS[key] || key === "planning") && sameAddress && screen.host.children.length > 0;
  if (retained) {
    state.currentRoute = key;
    state.currentGoal = null;
    state.underlayHash = hash;
    screen.host.hidden = false;
    screen.host.scrollTop = screen.scroll;
    syncNodeScopeNavigation(hash);
    highlightNav(key);
    refreshRetainedMainScreen(screen);
  }
  renderMainNavigation();
  if (mainScreens.focusKey === key) {
    document.querySelector(`#rail-main-screens [data-main-open="${key}"]`)?.focus();
    mainScreens.focusKey = null;
  }
  return retained;
}

async function closeMainScreen(key) {
  const screen = mainScreens.open.get(key);
  if (!screen) return;
  if ((screen.dirty || (key === "settings" && _targetAppDraftDirty)) && !await modalConfirm(
    `Close ${MAIN_DESTINATIONS[key].label} and discard unsaved work?`,
    { title: "Discard screen draft?", okLabel: "Discard and close", cancelLabel: "Keep editing", danger: true, focusCancel: true },
  )) return;
  mainScreens.open.delete(key);
  if (key === "settings") _targetAppDraftDirty = false;
  if (key === "goals") resetGoalsSelection();
  if (key === "features") resetFeaturesSelection();
  if (screen === mainScreens.active) {
    mainScreens.epoch++;
    mainScreens.active = null;
    const next = [...mainScreens.open.keys()].find(key => MAIN_DESTINATIONS[key]);
    const hash = next ? mainScreens.open.get(next).hash : "#/";
    workspaceNavigation.mainHash = hash;
    if (state.currentRoute === "window") {
      renderMainNavigation();
      document.querySelector('#rail-windows [aria-current="page"]')?.focus();
      return;
    }
    history.replaceState(null, "", hash);
    navigate();
    document.querySelector('#rail-main-screens [aria-current="page"]')?.focus();
  }
  renderMainNavigation();
}

function mainScreenHasDraft(screen, root = screen?.host) {
  return !!screen && [...screen.edits].some(([control, original]) => root?.contains(control)
    && settingsControlValue(control) !== (control.dataset.settingsSavedValue ?? original));
}

function mainScreenDirtySurfaces() {
  return [...mainScreens.open.values()].filter(screen => screen.dirty).map(screen => ({
    label: MAIN_DESTINATIONS[screen.route]?.label || screen.route, root: screen.host,
  }));
}

function invalidateMainScreenContext({ external = false } = {}) {
  mainScreens.epoch++;
  resetGoalsSelection();
  resetFeaturesSelection();
  dashboardReviewSelectedIds.clear();
  for (const [key, screen] of mainScreens.open) {
    if (external && screen.dirty) {
      screen.stale = true;
      preserveExternalDirtySurfaces([{ label: MAIN_DESTINATIONS[key].label, root: screen.host }]);
    } else if (screen !== mainScreens.active) mainScreens.open.delete(key);
    else {
      // Keep the active scaffold for context reconciliation's existing refresh.
      screen.edits.clear();
      screen.stale = false;
      screen.host.replaceChildren();
    }
  }
  renderMainNavigation();
}
