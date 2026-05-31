// ---- Guide -----------------------------------------------------------------

const GUIDE_WIDTH_KEY = "refine_guide_width";
const GUIDE_CHECKLIST_KEY = "refine_guide_checklist";
const GUIDE_DEFAULT_WIDTH = 360;
const GUIDE_MIN_WIDTH = 280;
const GUIDE_MAX_WIDTH = 560;
const GUIDE_STATUS_UNCHECKED = "unchecked";
const GUIDE_STATUS_CHECKED = "checked";
const GUIDE_STATUS_SKIPPED = "skipped";
let guideHighlightedTarget = null;

const guideState = {
  open: false,
  width: readGuideWidth(),
  statuses: readGuideChecklist(),
  context: "",
  activeCategory: "",
  activeItem: "",
};

const GUIDE_CATEGORIES = [
  {
    id: "quick-start",
    title: "Quick Start",
    description: "The few steps needed to get refine running on a new app.",
    items: [
      guideItem("quickstart-add-app", "Add app", "Configure",
        "Register the target app so refine can attach to it. Add an existing path, paste a Git clone URL, or create a new directory.",
        "Action: add the app you want refine to work on.",
        { hash: "#/project/application", selector: "#s-project-add" },
        { canUseDefault: false }),
      guideItem("quickstart-create-instance", "Create instance", "Configure",
        "Create an instance so this machine owns its Gaps and local runtime settings.",
        "Action: create an instance for this machine.",
        { hash: "#/instance/instances", selector: "#instance-add" },
        { canUseDefault: false }),
      guideItem("quickstart-generate-ai", "Generate with AI", "Configure",
        "Let the AI generator draft the target-app start, stop, and rebuild commands from the codebase.",
        "Action: generate the application commands with AI.",
        { hash: "#/instance/application", selector: "#s-target-generate-ai" },
        { canUseDefault: false }),
      guideItem("quickstart-start", "Start", "Configure",
        "Start the target app from System process management to confirm the commands work.",
        "Action: start the target app.",
        { hash: "#/system/processes", selector: "#s-target-run-start" },
        { canUseDefault: false }),
    ],
  },
  {
    id: "instance",
    title: "Instance",
    description: "Settings for this machine and active refine instance.",
    items: [
      guideItem("instance-educate", "Settings only for this machine", "Educate",
        "Instance settings belong to this machine. Use them for local runtime, reporters, and target-app commands that should not apply to every refine instance.",
        "Default: each machine keeps its own active instance settings.",
        { hash: "#/instance/instances", selector: "#settings-tabs" }),
      guideItem("instance-create", "Create an instance", "Educate and configure",
        "Create an instance when this machine, operator, or environment should own separate Gaps and local runtime settings.",
        "Action: create when setting up a new machine.",
        { hash: "#/instance/instances", selector: "#instance-add" },
        { canUseDefault: false }),
      guideItem("instance-active", "Activate instance", "Educate and configure",
        "The active instance controls local ownership and instance-scoped settings. Switch it before changing reporters or application commands for another machine.",
        "Default: keep the current active instance unless setup is for another machine.",
        { hash: "#/instance/instances", selector: "[data-instance-activate], .filter-pill, .table" },
        { canUseDefault: false }),
      guideItem("reporter-add", "Add reporter", "Educate and configure",
        "Reporters identify who submitted or owns feedback. Add the names your team will use when creating Gaps.",
        "Action: add the reporter names your team will use before creating Gaps.",
        { hash: "#/instance/reporters", selector: "#r-add" },
        { canUseDefault: false }),
      guideItem("reporter-manage", "Manage reporters", "Educate and configure",
        "Reporter rename, merge, and remove actions keep the dropdown useful while preserving historical Gap rounds.",
        "Default: leave existing reporters unchanged until duplicates or stale names appear.",
        { hash: "#/instance/reporters", selector: "[data-rename], [data-rmerge], [data-rdel]" },
        { canUseDefault: false }),
      guideItem("application-ai", "General with AI button", "Educate and configure",
        "The AI generator can draft target-app commands from the codebase. Dedicated refine start, stop, and rebuild scripts are usually more reliable when the app already has them.",
        "Action: generate with AI only when app-specific scripts are not already known.",
        { hash: "#/instance/application", selector: "#s-target-generate-ai" },
        { canUseDefault: false }),
      guideItem("application-url", "App URL", "Educate and configure",
        "The app URL is opened from the application status indicator when the target app is running.",
        "Default: blank until the local app has a stable URL.",
        { hash: "#/instance/application", selector: "#s-target-app-url" }),
      guideItem("application-start", "Start", "Educate and configure",
        "The start command should start the target app and return promptly. Prefer a project script that is safe to run repeatedly.",
        "Default: blank until you know the local start command.",
        { hash: "#/instance/application", selector: "#s-target-start-command" }),
      guideItem("application-stop", "Stop", "Educate and configure",
        "The stop command should be idempotent so refine can stop or rebuild the target app without manual cleanup.",
        "Default: blank until you know the local stop command.",
        { hash: "#/instance/application", selector: "#s-target-stop-command" }),
      guideItem("application-rebuild", "Rebuild", "Educate and configure",
        "The rebuild command prepares generated artifacts after merged work. Use a specific refine rebuild script when the app needs setup beyond the normal build command.",
        "Default: blank unless the app has a reliable build or rebuild command.",
        { hash: "#/instance/application", selector: "#s-target-rebuild-command" }),
      guideItem("application-auto-rebuild", "Automatic application rebuild", "Educate and configure",
        "Automatic rebuild controls when merged work is rebuilt before review.",
        "Default: on worktree merge.",
        { hash: "#/instance/application", selector: "#s-target-auto-rebuild" }),
      guideItem("application-status", "Status command", "Educate and configure",
        "The status command exits 0 only when the app is healthy or running. It is the most deterministic health check when available.",
        "Default: blank until a reliable local status command exists.",
        { hash: "#/instance/application", selector: "#s-target-status-command" }),
      guideItem("application-checks", "Optional checks", "Educate and configure",
        "Optional HTTP, TCP, and process checks add confidence, but should stay empty unless they match the app reliably.",
        "Default: all optional checks empty.",
        { hash: "#/instance/application", selector: "#s-target-http-url" }),
    ],
  },
  {
    id: "project",
    title: "Project",
    description: "Project-wide settings shared by all refine instances.",
    items: [
      guideItem("project-educate", "Project-wide settings", "Educate",
        "Project settings are stored with the app and shared by all refine instances. Use them for product intent, quality policy, and governance.",
        "Action: review these once per target app so shared policy is intentional.",
        { hash: "#/project/application", selector: "#settings-tabs" },
        { canUseDefault: false }),
      guideItem("project-application", "App selection and creation", "Educate",
        "Add an existing app path, paste a Git clone URL, or create a new directory. refine will attach the app and initialize .refine state when needed.",
        "Default: keep the current app unless you are setting up a new target app.",
        { hash: "#/project/application", selector: "#s-project-add" }),
      guideItem("quality-gate", "Quality Gate", "Configure",
        "Choose whether QA runs before merge in a Gap worktree or after the shared application rebuild.",
        "Default: pre-merge QA.",
        { hash: "#/project/quality", selector: "#s-quality-timing" }),
      guideItem("quality-regressions", "Regressions", "Educate and optional config",
        "Managed regressions give QA repeatable scenarios to run against the current checkout or workflow environment.",
        "Default: disabled until at least one useful regression exists.",
        { hash: "#/project/quality", selector: "#s-quality-regression-new" }),
      guideItem("quality-requirements", "Business requirements", "Educate and optional config",
        "Business requirements tell the Quality agent what behavior matters for this product.",
        "Default: blank until the project has stable requirements to enforce.",
        { hash: "#/project/quality", selector: "[data-settings-markdown-title='Business requirements']" }),
      guideItem("quality-instructions", "Instructions", "Educate and optional config",
        "Quality instructions tell the Quality agent how to evaluate coverage, risk, and evidence.",
        "Default: blank until the team has QA preferences to enforce.",
        { hash: "#/project/quality", selector: "[data-settings-markdown-title='Instructions']" }),
      guideItem("governance-product", "Product", "Educate and optional config",
        "Product context gives Governance the what and why before implementation work starts.",
        "Default: blank until the product shape is ready to share with agents.",
        { hash: "#/project/governance", selector: "[data-settings-markdown-title='Product']" }),
      guideItem("governance-constitution", "Constitution", "Educate and optional config",
        "The constitution records project principles that should apply across all Gap work.",
        "Default: blank until the team has non-negotiable principles.",
        { hash: "#/project/governance", selector: "[data-settings-markdown-title='Constitution']" }),
      guideItem("governance-rules", "Rules", "Educate and optional config",
        "Rules are short checks Governance applies before implementation. Use Add rule for manual rules or Generate rules when Product and Constitution are filled in.",
        "Default: no rules.",
        { hash: "#/project/governance", selector: "#s-governance-add-rule" }),
    ],
  },
  {
    id: "system",
    title: "System",
    description: "Runtime and process management.",
    items: [
      guideItem("process-stop-background", "Stop background processes", "Educate",
        "Stopping background processes keeps the UI running while pausing scheduling, chats, agents, queued rebuilds, and active background jobs.",
        "Default: leave background processes running unless you need a maintenance pause.",
        { hash: "#/system/processes", selector: "[data-toggle-background-processes]" }),
      guideItem("process-pause-agents", "Pause or unpause agents", "Educate",
        "Pausing agents stops new agent subprocesses while leaving the rest of refine available.",
        "Default: agents unpaused.",
        { hash: "#/system/processes", selector: "[data-toggle-agent-processes]" }),
    ],
  },
  {
    id: "main-nav",
    title: "Main nav",
    description: "Common navigation and daily actions.",
    items: [
      guideItem("nav-application-status", "Application status", "Educate",
        "The application status indicator shows target-app state and opens the System process view for start, stop, rebuild, sync, and checks.",
        "Action: use this indicator to inspect and control the target app.",
        { selector: "#target-app-indicator", openContextMenu: true },
        { canUseDefault: false }),
      guideItem("nav-agent-status", "Agent status", "Educate",
        "The agent status pill summarizes active or paused agent work and links to System processes.",
        "Action: use this pill to inspect whether agents are running or paused.",
        { selector: "#agent-status-indicator" },
        { canUseDefault: false }),
      guideItem("nav-reporter", "Reporter", "Educate and configure",
        "The reporter selector chooses who new Gaps are submitted as.",
        "Action: pick or add the reporter before creating Gaps.",
        { selector: "#global-reporter", openContextMenu: true },
        { canUseDefault: false }),
      guideItem("nav-create-gap", "Creating Gap", "Educate",
        "Create a Gap when actual behavior differs from target behavior.",
        "Action: open the Gap form, write actual vs target behavior, then save it.",
        { command: "gap.new" },
        { canUseDefault: false }),
      guideItem("nav-import-gaps", "Importing Gaps", "Educate",
        "Import turns CSV or pasted feedback into editable Gap drafts before saving.",
        "Action: review the drafts before importing them.",
        { command: "gap.import" },
        { canUseDefault: false }),
      guideItem("nav-report-bug", "Report refine bug", "Educate",
        "Use the refine issue action for product feedback, bugs, and feature requests about refine itself.",
        "Action: open the issue form when refine itself needs feedback or a fix.",
        { command: "refine.issue.request" },
        { canUseDefault: false }),
    ],
  },
];

function guideItem(id, title, kind, description, defaultText, target, options = {}) {
  return {
    id,
    title,
    kind,
    description,
    defaultText,
    target,
    canUseDefault: options.canUseDefault !== false,
  };
}

function readGuideChecklist() {
  try {
    const parsed = JSON.parse(localStorage.getItem(GUIDE_CHECKLIST_KEY) || "{}");
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
}

function saveGuideChecklist() {
  try {
    localStorage.setItem(GUIDE_CHECKLIST_KEY, JSON.stringify(guideState.statuses || {}));
  } catch {}
}

function resetGuideState({ redraw = true } = {}) {
  guideState.statuses = {};
  guideState.context = "";
  guideState.activeCategory = "";
  guideState.activeItem = "";
  clearGuideTargetHighlight();
  try { localStorage.removeItem(GUIDE_CHECKLIST_KEY); } catch {}
  if (redraw) drawGuide();
}

function guideItemsInOrder() {
  return GUIDE_CATEGORIES.flatMap((category) => (
    category.items.map((item) => ({ category, item }))
  ));
}

function guideItemStatus(id) {
  const status = guideState.statuses?.[id] || GUIDE_STATUS_UNCHECKED;
  return [GUIDE_STATUS_CHECKED, GUIDE_STATUS_SKIPPED].includes(status)
    ? status
    : GUIDE_STATUS_UNCHECKED;
}

function guideItemIsIncomplete(id) {
  return guideItemStatus(id) === GUIDE_STATUS_UNCHECKED;
}

function guideProgress() {
  const items = guideItemsInOrder();
  const done = items.filter(({ item }) => !guideItemIsIncomplete(item.id)).length;
  return { done, total: items.length };
}

function firstIncompleteGuideItem({ afterId = "" } = {}) {
  const ordered = guideItemsInOrder();
  const startIndex = afterId
    ? Math.max(0, ordered.findIndex(({ item }) => item.id === afterId) + 1)
    : 0;
  return ordered.slice(startIndex).find(({ item }) => guideItemIsIncomplete(item.id))
    || ordered.find(({ item }) => guideItemIsIncomplete(item.id))
    || null;
}

function activateGuideItem(found) {
  if (!found) return;
  guideState.activeCategory = found.category.id;
  guideState.activeItem = found.item.id;
}

function guideItemByOffset(id, offset) {
  const ordered = guideItemsInOrder();
  const index = ordered.findIndex(({ item }) => item.id === id);
  if (index < 0) return null;
  return ordered[index + offset] || null;
}

function ensureGuideSelection() {
  if (!guideState.activeItem || !findGuideItem(guideState.activeItem)) {
    activateGuideItem(firstIncompleteGuideItem());
  }
}

function setGuideItemStatus(id, status, { advance = false } = {}) {
  if (![GUIDE_STATUS_UNCHECKED, GUIDE_STATUS_CHECKED, GUIDE_STATUS_SKIPPED].includes(status)) {
    status = GUIDE_STATUS_UNCHECKED;
  }
  if (status === GUIDE_STATUS_UNCHECKED) {
    delete guideState.statuses[id];
  } else {
    guideState.statuses[id] = status;
  }
  saveGuideChecklist();
  if (advance) {
    activateGuideItem(firstIncompleteGuideItem({ afterId: id }));
  } else if (!guideItemIsIncomplete(guideState.activeItem)) {
    ensureGuideSelection();
  }
}

function cycleGuideItemStatus(id) {
  const current = guideItemStatus(id);
  const next = current === GUIDE_STATUS_UNCHECKED
    ? GUIDE_STATUS_CHECKED
    : current === GUIDE_STATUS_CHECKED
      ? GUIDE_STATUS_SKIPPED
      : GUIDE_STATUS_UNCHECKED;
  const before = guideState.activeItem;
  setGuideItemStatus(id, next, { advance: next !== GUIDE_STATUS_UNCHECKED });
  drawGuide();
  if (guideState.activeItem !== before) openActiveGuideTarget();
}

function completeGuideItem(id, status) {
  const before = guideState.activeItem;
  setGuideItemStatus(id, status, { advance: true });
  drawGuide();
  if (guideState.activeItem !== before) openActiveGuideTarget();
}

function openPreviousGuideItem(id) {
  const previous = guideItemByOffset(id, -1);
  if (!previous) return;
  activateGuideItem(previous);
  drawGuide();
  openActiveGuideTarget();
}

function selectGuideItem(id) {
  const found = findGuideItem(id);
  if (!found) return;
  activateGuideItem(found);
  drawGuide();
  openGuideItemTarget(found.item);
}

function readGuideWidth() {
  try {
    const raw = parseInt(localStorage.getItem(GUIDE_WIDTH_KEY) || "", 10);
    if (Number.isFinite(raw)) return clampGuideWidth(raw);
  } catch {}
  return GUIDE_DEFAULT_WIDTH;
}

function clampGuideWidth(width) {
  const viewportMax = Math.max(GUIDE_MIN_WIDTH, window.innerWidth - 48);
  return Math.max(GUIDE_MIN_WIDTH, Math.min(GUIDE_MAX_WIDTH, viewportMax, Math.round(width)));
}

function initGuide() {
  setGuideWidth(guideState.width, { persist: false });
  drawGuide();
  window.addEventListener("resize", () => {
    setGuideWidth(guideState.width, { persist: false });
  });
  document.getElementById("nav-guide-open")?.addEventListener("click", (e) => {
    e.preventDefault();
    closeTopbarMenus();
    openGuide();
  });
}

function openGuide(options = {}) {
  guideState.open = true;
  guideState.context = options.context || guideState.context || "";
  const requested = options.itemId ? findGuideItem(options.itemId) : null;
  const firstIncomplete = requested || firstIncompleteGuideItem();
  if (firstIncomplete) activateGuideItem(firstIncomplete);
  setGuideWidth(guideState.width, { persist: false });
  drawGuide();
  if (firstIncomplete && options.openTarget !== false) openActiveGuideTarget();
}

function closeGuide() {
  guideState.open = false;
  guideState.context = "";
  guideState.activeCategory = "";
  guideState.activeItem = "";
  clearGuideTargetHighlight();
  drawGuide();
}

function toggleGuide() {
  if (guideState.open) closeGuide();
  else openGuide();
}

function setGuideWidth(width, { persist = true } = {}) {
  guideState.width = clampGuideWidth(width);
  document.documentElement.style.setProperty("--guide-panel-width", `${guideState.width}px`);
  if (persist) {
    try { localStorage.setItem(GUIDE_WIDTH_KEY, String(guideState.width)); } catch {}
  }
}

function guideContextMessage() {
  if (guideState.context === "app-created") {
    return "This app was initialized for refine. Start with Project Application, then create or select the right Instance for this machine.";
  }
  if (guideState.context === "app-existing") {
    return "This app already has refine state. Review Project settings, then select or create the right Instance for this machine.";
  }
  if (guideState.context === "no-app") {
    return "No app is attached. Configure Refine from Project Application, then select or create the right Instance for this machine.";
  }
  return "Use the Guide as a checklist. Opening an item takes you to the related refine control, and the action bar records progress.";
}

function drawGuide() {
  const root = document.getElementById("guide-panel");
  if (!root) return;
  root.classList.toggle("open", guideState.open);
  root.setAttribute("aria-hidden", guideState.open ? "false" : "true");
  document.body.classList.toggle("guide-open", guideState.open);
  if (!guideState.open) {
    root.innerHTML = "";
    return;
  }
  ensureGuideSelection();
  const progress = guideProgress();
  root.innerHTML = `
    <div class="guide-resize" id="guide-resize"
         role="separator" aria-orientation="vertical"
         aria-label="Resize Guide"
         title="Drag to resize"></div>
    <div class="guide-header">
      <h2>Guide</h2>
      <button type="button" class="secondary guide-close" id="guide-close"
              aria-label="Close Guide" title="Close Guide">x</button>
    </div>
    <div class="guide-body">
      <p class="guide-intro">${htmlEscape(guideContextMessage())}</p>
      <div class="guide-progress" aria-live="polite">
        <strong>${progress.done}</strong> completed/skipped vs <strong>${progress.total}</strong> total
      </div>
      ${GUIDE_CATEGORIES.map(renderGuideCategory).join("")}
    </div>
  `;
  root.querySelector("#guide-close")?.addEventListener("click", closeGuide);
  root.querySelectorAll("[data-guide-open-item]").forEach((button) => {
    button.addEventListener("click", () => selectGuideItem(button.dataset.guideOpenItem || ""));
  });
  root.querySelectorAll("[data-guide-status]").forEach((button) => {
    button.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
      cycleGuideItemStatus(button.dataset.guideStatus || "");
    });
  });
  root.querySelectorAll("[data-guide-prev]").forEach((button) => {
    button.addEventListener("click", () => openPreviousGuideItem(button.dataset.guidePrev || ""));
  });
  root.querySelectorAll("[data-guide-default]").forEach((button) => {
    button.addEventListener("click", () => completeGuideItem(button.dataset.guideDefault || "", GUIDE_STATUS_CHECKED));
  });
  root.querySelectorAll("[data-guide-skip]").forEach((button) => {
    button.addEventListener("click", () => completeGuideItem(button.dataset.guideSkip || "", GUIDE_STATUS_SKIPPED));
  });
  root.querySelectorAll("[data-guide-complete]").forEach((button) => {
    button.addEventListener("click", () => completeGuideItem(button.dataset.guideComplete || "", GUIDE_STATUS_CHECKED));
  });
  wireGuideResize(root);
}

function renderGuideCategory(category) {
  const open = guideState.activeCategory === category.id
    || category.items.some((item) => item.id === guideState.activeItem);
  return `
    <details class="guide-category" data-guide-category="${htmlEscape(category.id)}" ${open ? "open" : ""}>
      <summary>
        <span class="guide-category-summary">
          ${guideChevronIcon()}
          <span>
            <span class="guide-category-title">${htmlEscape(category.title)}</span>
            <span class="guide-category-description">${htmlEscape(category.description)}</span>
          </span>
        </span>
      </summary>
      <div class="guide-item-list">
        ${category.items.map(renderGuideItem).join("")}
      </div>
    </details>
  `;
}

function renderGuideItem(item) {
  const open = guideState.activeItem === item.id;
  const status = guideItemStatus(item.id);
  const previous = guideItemByOffset(item.id, -1);
  const defaultButton = item.canUseDefault
    ? `<button type="button" class="secondary" data-guide-default="${htmlEscape(item.id)}">Use default</button>`
    : "";
  return `
    <div class="guide-item ${open ? "active" : ""}" data-guide-item="${htmlEscape(item.id)}">
      <div class="guide-item-summary">
        <button type="button" class="guide-item-open"
                data-guide-open-item="${htmlEscape(item.id)}"
                aria-expanded="${open ? "true" : "false"}">
          ${guideChevronIcon()}
          <span class="guide-item-title">${htmlEscape(item.title)}</span>
        </button>
        ${guideStatusButton(item, status)}
      </div>
      <div class="guide-item-body" ${open ? "" : "hidden"}>
        <p>${htmlEscape(item.description)}</p>
        <div class="guide-default">${htmlEscape(item.defaultText)}</div>
        <div class="guide-item-actions">
          <button type="button" class="secondary" data-guide-prev="${htmlEscape(item.id)}" ${previous ? "" : "disabled"}>Prev</button>
          ${defaultButton}
          <button type="button" class="secondary" data-guide-skip="${htmlEscape(item.id)}">Skip</button>
          <button type="button" data-guide-complete="${htmlEscape(item.id)}">Complete</button>
        </div>
      </div>
    </div>
  `;
}

function guideStatusButton(item, status) {
  const label = {
    [GUIDE_STATUS_CHECKED]: "Checked",
    [GUIDE_STATUS_SKIPPED]: "Skipped",
    [GUIDE_STATUS_UNCHECKED]: "Unchecked",
  }[status] || "Unchecked";
  return `
    <button type="button"
            class="guide-status guide-status-${htmlEscape(status)}"
            data-guide-status="${htmlEscape(item.id)}"
            aria-label="${label}: ${htmlEscape(item.title)}"
            title="${label}. Click to change checklist state.">
      ${guideStatusIcon(status)}
    </button>`;
}

function guideStatusIcon(status) {
  if (status === GUIDE_STATUS_CHECKED) {
    return `
      <svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">
        <circle cx="12" cy="12" r="9"></circle>
        <path d="m8 12 2.6 2.6L16.5 9"></path>
      </svg>`;
  }
  if (status === GUIDE_STATUS_SKIPPED) {
    return `
      <svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">
        <circle cx="12" cy="12" r="9"></circle>
        <path d="M8 12h8"></path>
      </svg>`;
  }
  return `
    <svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">
      <circle cx="12" cy="12" r="9"></circle>
    </svg>`;
}

function guideChevronIcon() {
  return `
    <svg class="guide-chevron" aria-hidden="true" viewBox="0 0 24 24" focusable="false">
      <path d="m9 18 6-6-6-6"></path>
    </svg>`;
}

function findGuideItem(id) {
  for (const category of GUIDE_CATEGORIES) {
    const item = category.items.find((candidate) => candidate.id === id);
    if (item) return { category, item };
  }
  return null;
}

function openActiveGuideTarget() {
  const found = findGuideItem(guideState.activeItem || "");
  if (found) openGuideItemTarget(found.item);
}

async function openGuideItemTarget(item) {
  const target = item?.target || {};
  if (target.command) {
    clearGuideTargetHighlight();
    closeTopbarMenus();
    await runCommand(target.command);
    return;
  }
  if (target.openContextMenu) {
    const menu = document.getElementById("nav-context-menu");
    if (menu) menu.open = true;
  } else {
    closeTopbarMenus();
  }
  if (target.hash && location.hash !== target.hash) {
    location.hash = target.hash;
  } else if (target.hash && typeof navigate === "function") {
    navigate();
  }
  if (target.selector) {
    const el = await waitForGuideTarget(target.selector);
    if (!el) {
      clearGuideTargetHighlight();
      toast("Guide target is not available on this screen.", "warn");
      return;
    }
    focusAndHighlightGuideTarget(el);
  } else {
    clearGuideTargetHighlight();
  }
}

function waitForGuideTarget(selector) {
  const started = Date.now();
  return new Promise((resolve) => {
    function check() {
      let el = null;
      try { el = document.querySelector(selector); } catch {}
      if (el && !el.hidden && el.offsetParent !== null) {
        resolve(el);
        return;
      }
      if (Date.now() - started > 2500) {
        resolve(el);
        return;
      }
      setTimeout(check, 50);
    }
    check();
  });
}

function focusAndHighlightGuideTarget(el) {
  el.scrollIntoView({ behavior: "smooth", block: "center", inline: "nearest" });
  if (typeof el.focus === "function") {
    try { el.focus({ preventScroll: true }); } catch { el.focus(); }
  }
  clearGuideTargetHighlight(el);
  el.classList.add("guide-target-highlight");
  guideHighlightedTarget = el;
}

function clearGuideTargetHighlight(except = null) {
  if (guideHighlightedTarget && guideHighlightedTarget !== except) {
    guideHighlightedTarget.classList.remove("guide-target-highlight");
  }
  if (guideHighlightedTarget !== except) {
    guideHighlightedTarget = null;
  }
}

function wireGuideResize(root) {
  const handle = root.querySelector("#guide-resize");
  if (!handle) return;
  handle.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = guideState.width;
    handle.setPointerCapture(e.pointerId);
    root.classList.add("resizing");
    function onMove(ev) {
      setGuideWidth(startWidth + (startX - ev.clientX), { persist: false });
    }
    function onUp(ev) {
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onUp);
      handle.removeEventListener("pointercancel", onUp);
      try { handle.releasePointerCapture(ev.pointerId); } catch {}
      root.classList.remove("resizing");
      setGuideWidth(guideState.width, { persist: true });
    }
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
    handle.addEventListener("pointercancel", onUp);
  });
}

registerCommand({
  id: "guide.toggle",
  title: "Toggle Guide",
  group: "Navigate",
  aliases: ["guide", "open-guide"],
  run: () => toggleGuide(),
});

registerCommand({
  id: "guide.open",
  title: "Open Guide",
  group: "Navigate",
  aliases: ["show-guide"],
  run: () => openGuide(),
});

registerCommand({
  id: "guide.close",
  title: "Close Guide",
  group: "Navigate",
  aliases: ["hide-guide"],
  visible: () => guideState.open,
  run: () => closeGuide(),
});
