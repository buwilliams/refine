// Shared application shell. Tool sessions and retained Main hosts have separate
// lifetimes; switching destinations never restarts a tool.
const workspaceNavigation = {
  mainHash: null,
  mainRoute: null,
  inWindow: false,
};

function initWorkspaceNavigation() {
  const rail = document.getElementById("navigation-rail");
  if (!rail) return;
  try {
    rail.classList.toggle("collapsed", localStorage.getItem("refine_rail_collapsed") === "true");
    for (const id of ["rail-skills-section", "rail-hubs-section"]) {
      const section = document.getElementById(id);
      section.open = localStorage.getItem(id) !== "false";
    }
  } catch { /* Storage can be unavailable in private browser contexts. */ }
  syncRailToggle();
  rail.querySelectorAll(".nav-menu").forEach(menu => menu.addEventListener("toggle", () => {
    if (menu.open) closeTopbarMenus(menu);
    menu.querySelector('summary[aria-haspopup="menu"]')?.setAttribute("aria-expanded", String(menu.open));
    positionRailMenus();
  }));
  initRailNewMenu();
  initRailNewMenu("main-screen-menu");
  initMainScreens();
  document.getElementById("rail-navigation").addEventListener("scroll", positionRailMenus);
  window.addEventListener("resize", positionRailMenus);
  document.getElementById("rail-toggle").addEventListener("click", () => {
    if (window.matchMedia("(max-width: 700px)").matches) return closeMobileNavigation(true);
    rail.classList.toggle("collapsed");
    try { localStorage.setItem("refine_rail_collapsed", rail.classList.contains("collapsed")); } catch {}
    syncRailToggle();
  });
  for (const id of ["rail-skills-section", "rail-hubs-section"]) {
    const section = document.getElementById(id);
    section.addEventListener("toggle", () => {
      try { localStorage.setItem(id, section.open); } catch {}
    });
  }
  document.getElementById("mobile-rail-toggle").addEventListener("click", () => {
    rail.classList.add("mobile-open");
    document.querySelector(".workspace").inert = true;
    document.getElementById("rail-scrim").hidden = false;
    document.getElementById("mobile-rail-toggle").setAttribute("aria-expanded", "true");
    document.getElementById("rail-toggle").focus();
  });
  window.matchMedia("(max-width: 700px)").addEventListener("change", () => closeMobileNavigation());
  document.getElementById("rail-scrim").addEventListener("click", () => closeMobileNavigation(true));
  rail.addEventListener("keydown", event => {
    if (event.key === "Tab" && rail.classList.contains("mobile-open")) {
      const controls = [...rail.querySelectorAll("a, button, summary, [tabindex='0']")]
        .filter(el => !el.disabled && el.getClientRects().length);
      const first = controls[0], last = controls[controls.length - 1];
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    }
    if (event.key === "Escape") {
      const menu = event.target.closest(".nav-menu");
      if (menu?.open) {
        event.preventDefault();
        event.stopPropagation();
        menu.open = false;
        menu.querySelector("summary")?.focus();
      } else closeMobileNavigation(true);
    }
  });
  rail.addEventListener("click", event => {
    const add = event.target.closest("[data-add-toolbar-tab]");
    if (add) {
      add.closest("details").open = false;
      void createToolbarTab(add.dataset.addToolbarTab).catch(showActionError);
    }
    if (event.target.closest("a[data-route], #btn-command-palette")) closeMobileNavigation();
  });
  const windows = document.getElementById("rail-windows");
  windows.addEventListener("click", event => {
    const close = event.target.closest("[data-close-tab]");
    if (close) {
      event.preventDefault();
      void closeChatTab(close.dataset.closeTab);
    }
    if (event.target.closest("[data-tab-id]")) closeMobileNavigation();
  });
}

function initRailNewMenu(id = "rail-new-menu") {
  const menu = document.getElementById(id);
  const summary = menu.querySelector("summary");
  const items = [...menu.querySelectorAll('[role="menuitem"]')];
  menu.addEventListener("keydown", event => {
    const onSummary = event.target === summary;
    const toggle = onSummary && ["Enter", " "].includes(event.key);
    if (toggle || ["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
      event.preventDefault();
      event.stopPropagation();
      if (toggle && menu.open) {
        menu.open = false;
        return;
      }
      closeTopbarMenus(menu);
      menu.open = true;
      positionRailMenus();
      let index = items.indexOf(document.activeElement);
      if (event.key === "End" || (onSummary && event.key === "ArrowUp")) index = items.length - 1;
      else if (event.key === "Home" || onSummary) index = 0;
      else index = (index + (event.key === "ArrowUp" ? -1 : 1) + items.length) % items.length;
      items[index].focus();
    } else if (event.key === "Tab" && menu.open) {
      // Continue the rail's normal tab order outside this menu.
      menu.open = false;
      summary.focus();
    }
  });
}

function positionRailMenus() {
  const rail = document.getElementById("navigation-rail");
  const mobile = window.matchMedia("(max-width: 700px)").matches;
  rail.querySelectorAll(".nav-menu[open]").forEach(menu => {
    const panel = menu.querySelector(".nav-menu-panel, .toolbar-add-options");
    if (!panel) return;
    const anchor = menu.querySelector("summary").getBoundingClientRect();
    panel.style.width = mobile ? `${rail.getBoundingClientRect().width - 16}px` : "";
    const bounds = panel.getBoundingClientRect();
    const left = mobile ? 8 : rail.getBoundingClientRect().right + 8;
    panel.style.left = `${Math.max(8, Math.min(left, innerWidth - bounds.width - 8))}px`;
    panel.style.top = `${Math.max(8, Math.min(anchor.top, innerHeight - bounds.height - 8))}px`;
  });
}

function syncRailToggle() {
  const collapsed = document.getElementById("navigation-rail").classList.contains("collapsed");
  const button = document.getElementById("rail-toggle");
  button.setAttribute("aria-expanded", String(!collapsed));
  button.setAttribute("aria-label", collapsed ? "Expand navigation" : "Collapse navigation");
  button.title = collapsed ? "Expand navigation" : "Collapse navigation";
  positionRailMenus();
  if (typeof scheduleActiveTerminalFit === "function") scheduleActiveTerminalFit();
}

function closeMobileNavigation(restoreFocus = false) {
  const workspace = document.querySelector(".workspace");
  if (workspace) workspace.inert = false;
  document.getElementById("navigation-rail")?.classList.remove("mobile-open");
  const scrim = document.getElementById("rail-scrim");
  if (scrim) scrim.hidden = true;
  const toggle = document.getElementById("mobile-rail-toggle");
  toggle?.setAttribute("aria-expanded", "false");
  if (restoreFocus) toggle?.focus();
}

function windowHash(tabId) { return `#/windows/${encodeURIComponent(tabId)}`; }

function navigateToWindow(tabId) {
  const hash = windowHash(tabId);
  if (location.hash === hash) return false;
  // Navigate synchronously so callers awaiting a window can safely use its
  // renderer/session. pushState adds history without scheduling a second route.
  _prevHashURL = location.href;
  history.pushState(null, "", hash);
  return Promise.resolve(navigate());
}

function syncWorkspaceVisibility() {
  renderMainNavigation();
  const isWindow = !!chatState.open && !!currentToolbarTab();
  document.getElementById("main").hidden = isWindow;
  document.getElementById("toolbar-dock").hidden = !isWindow;
  document.getElementById("workspace-tools").hidden = isWindow || state.currentRoute !== "settings";
  document.querySelectorAll(".rail-main a").forEach(link => {
    if (isWindow) {
      link.classList.remove("active");
      link.removeAttribute("aria-current");
    }
  });
  document.getElementById("rail-main-section").classList.toggle("has-active", !isWindow && state.currentRoute !== "planning");
  document.getElementById("rail-planning-section")?.classList.toggle("has-active", !isWindow && state.currentRoute === "planning");
}

// Same locally bundled SVGs are used by the static rail and window menu.
function railIcon(name) {
  return `<svg class="rail-icon" aria-hidden="true" focusable="false" viewBox="0 0 24 24"><use href="/static/vendor/lucide/navigation.svg#${name}"></use></svg>`;
}

function renderWindowNavigation() {
  const root = document.getElementById("rail-windows");
  if (!root) return;
  const icons = { terminal: "square-terminal", agent: "bot", standalone: "git-branch", plan: "notebook-pen", goal: "target", goal_logs: "scroll-text", files: "folder-open", system: "activity", todo: "list-checks", skill: "wand-sparkles" };
  renderInto(root, Object.entries(chatState.tabs).map(([id, tab]) => {
    const active = chatState.open && id === chatState.activeTabId;
    const closeLabel = toolbarTabUsesTerminal(tab) && !tab.exited ? "Close and stop" : "Close";
    return `<div class="rail-window-row${active ? " active" : ""}" id="window-nav-${htmlEscape(id)}">
      <a class="rail-row toolbar-tab ${toolbarTabActivityClass(tab)}${active ? " active" : ""}" href="${windowHash(id)}" data-tab-id="${htmlEscape(id)}" data-testid="toolbar-tab-${htmlEscape(id)}" title="${htmlEscape(tab.label)} — ${htmlEscape(toolbarTabTitle(tab))}" ${active ? 'aria-current="page"' : ""}>
        ${railIcon(icons[tab.mode] || "panels-top-left")}<span class="rail-copy rail-window-label">${htmlEscape(tab.label)}</span>${toolbarTabSessionDot(tab)}
      </a>
      <button class="rail-window-close" type="button" data-close-tab="${htmlEscape(id)}" data-testid="toolbar-tab-close" aria-label="${closeLabel} ${htmlEscape(tab.label)}" title="${closeLabel} ${htmlEscape(tab.label)}">${railIcon("x")}</button>
    </div>`;
  }).join(""));
  syncWorkspaceVisibility();
}

// Returns true when the shell has handled navigation without replacing #main.
function routeWorkspace(route, hash) {
  closeTopbarMenus();
  if (route.route === "window") {
    if (!chatState.tabs[route.id]) {
      history.replaceState(null, "", workspaceNavigation.mainHash || "#/");
      navigate();
      return true;
    }
    captureMainScreen(true);
    if (mainScreens.active?.host.isConnected) {
      const placeholder = mainScreens.active.host.cloneNode(false);
      mainScreens.active.host.replaceWith(placeholder);
    }
    workspaceNavigation.mainHash = mainScreens.active?.hash || state.underlayHash || "#/";
    workspaceNavigation.mainRoute = mainScreens.active?.route || state.currentRoute;
    mainScreens.epoch++;
    workspaceNavigation.inWindow = true;
    state.currentRoute = "window";
    state.currentGoal = null;
    closeMobileNavigation();
    return activateToolbarTab(route.id).then(() => {
      if (chatState.open && chatState.activeTabId === route.id && document.activeElement?.closest?.("#rail-windows, .toolbar-add-menu")) {
        terminalStateFor(route.id)?.term?.focus();
      }
    });
  }
  workspaceNavigation.inWindow = false;
  chatState.open = false;
  saveChatStateToStorage();
  const handled = activateMainScreen(route, hash);
  renderWindowNavigation();
  return handled;
}

function navigateAfterWindowClose(tabId) {
  if (location.hash !== windowHash(tabId)) return;
  const next = chatState.activeTabId;
  history.replaceState(null, "", next ? windowHash(next) : workspaceNavigation.mainHash || "#/");
  navigate();
  document.querySelector('#rail-windows [aria-current="page"]')?.focus();
  if (!next) document.querySelector('.rail-main a.active')?.focus();
}

function resetWorkspaceNavigation() {
  workspaceNavigation.mainHash = null;
  workspaceNavigation.mainRoute = null;
  workspaceNavigation.inWindow = false;
  if (location.hash.startsWith("#/windows/")) location.hash = "#/";
}
