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
  referenceQuery: "",
};

const GUIDE_CATEGORIES = [
  {
    id: "get-started",
    title: "Get Started",
    description: "The minimum steps needed to run refine on this app.",
    checklist: true,
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
      guideItem("instance-manage", "Instances", "Educate and configure",
        "Instances separate machine-local ownership, runtime configuration, reporters, and application commands while sharing project-level policy.",
        "Default: keep one active instance unless another machine or environment needs separate ownership.",
        { hash: "#/instance/instances", selector: "#instance-add" }),
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
      guideItem("reporter-merge-into", "Merge into", "Educate and configure",
        "Merge into chooses the destination reporter when consolidating duplicate reporter names.",
        "Default: merge only when you are sure two reporter names represent the same source.",
        { hash: "#/instance/reporters", selector: "[data-rmerge]" },
        { canUseDefault: false }),
      guideItem("instance-copy-settings-source", "Source instance", "Educate and configure",
        "Source instance chooses another instance to copy Application or Runtime settings from into the active instance.",
        "Default: copy only when the source instance is known to have the desired local configuration.",
        { hash: "#/instance/application", selector: "#s-application-copy-instance" },
        { canUseDefault: false }),
      guideItem("application-ai", "Generate with AI button", "Educate and configure",
        "The AI generator analyses the codebase, writes a .refine/manage-app.sh wrapper (start, stop, rebuild, status — with detailed logging), and points the target-app commands at it. Edit the script or the saved commands afterward if you have a more reliable app-specific approach.",
        "Action: generate with AI to scaffold .refine/manage-app.sh, then review it.",
        { hash: "#/instance/application", selector: "#s-target-generate-ai" },
        { canUseDefault: false }),
      guideItem("application-agent-subpath", "Agent subpath", "Educate and optional config",
        "Agent subpath changes the working directory for agent and chat subprocesses inside a monorepo. Leave it blank when work should happen from the repository root.",
        "Default: blank, which uses the repo root.",
        { hash: "#/instance/application", selector: "#s-subpath" }),
      guideItem("application-merge-target", "Merge target branch", "Educate and optional config",
        "Merge target branch chooses the branch Gap worktrees are based on and where merged work lands. Blank follows the host checkout.",
        "Default: blank, following the host's current branch.",
        { hash: "#/instance/application", selector: "#s-merge-target" }),
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
      guideItem("application-working-directory", "Working directory", "Educate and optional config",
        "Working directory is the repo-relative directory where target-app commands run. Use it when app scripts live below the repo root.",
        "Default: blank, which uses the repo root.",
        { hash: "#/instance/application", selector: "#s-target-cwd" }),
      guideItem("application-environment", "Environment overrides", "Educate and optional config",
        "Environment overrides are a JSON object merged into the host environment before target-app commands run.",
        "Default: an empty JSON object.",
        { hash: "#/instance/application", selector: "#s-target-env" }),
      guideItem("application-start-timeout", "Start timeout", "Educate and optional config",
        "Start timeout is how long Refine waits for the start command before treating it as failed.",
        "Default: 120 seconds.",
        { hash: "#/instance/application", selector: "#s-target-start-timeout" }),
      guideItem("application-stop-timeout", "Stop timeout", "Educate and optional config",
        "Stop timeout is how long Refine waits for the stop command before treating it as failed.",
        "Default: 60 seconds.",
        { hash: "#/instance/application", selector: "#s-target-stop-timeout" }),
      guideItem("application-rebuild-timeout", "Rebuild timeout", "Educate and optional config",
        "Rebuild timeout is how long Refine waits for the rebuild command before leaving rebuild work failed or incomplete.",
        "Default: 300 seconds.",
        { hash: "#/instance/application", selector: "#s-target-rebuild-timeout" }),
      guideItem("application-status-timeout", "Status timeout", "Educate and optional config",
        "Status timeout is how long Refine waits for each health or status check before treating it as failed.",
        "Default: 10 seconds.",
        { hash: "#/instance/application", selector: "#s-target-status-timeout" }),
      guideItem("application-log-path", "Log path", "Educate and optional config",
        "Log path points Refine at a local target-app log file that helps explain start, stop, rebuild, or status failures.",
        "Default: blank.",
        { hash: "#/instance/application", selector: "#s-target-log-path" }),
      guideItem("application-http-check-url", "HTTP check URL", "Educate and optional config",
        "HTTP check URL is polled from the host; a 2xx response marks the target app healthy.",
        "Default: blank unless the app exposes a stable health endpoint.",
        { hash: "#/instance/application", selector: "#s-target-http-url" }),
      guideItem("application-tcp-host", "TCP host", "Educate and optional config",
        "TCP host is paired with TCP port when an open socket is the best signal that the app is running.",
        "Default: blank.",
        { hash: "#/instance/application", selector: "#s-target-tcp-host" }),
      guideItem("application-tcp-port", "TCP port", "Educate and optional config",
        "TCP port is paired with TCP host when an open socket is the best signal that the app is running.",
        "Default: blank.",
        { hash: "#/instance/application", selector: "#s-target-tcp-port" }),
      guideItem("application-process-check-command", "Process check command", "Educate and optional config",
        "Process check command is a one-line host command that exits 0 when the expected target-app process exists.",
        "Default: blank unless process matching is more reliable than HTTP, TCP, or a status command.",
        { hash: "#/instance/application", selector: "#s-target-process-command" }),
      guideItem("application-checks", "Optional checks", "Educate and configure",
        "Optional HTTP, TCP, and process checks add confidence, but should stay empty unless they match the app reliably.",
        "Default: all optional checks empty.",
        { hash: "#/instance/application", selector: "#s-target-http-url" }),
      guideItem("runtime-parallel-run-cap", "Parallel-run cap", "Educate and configure",
        "Parallel-run cap limits how many Gap agent runs this instance can launch at once.",
        "Default: 5.",
        { hash: "#/instance/runtime", selector: "#s-cap" }),
      guideItem("runtime-branch-name-pattern", "Branch name pattern", "Educate and configure",
        "Branch name pattern controls worktree branch names. Include the Gap id token so branches stay unique.",
        "Default: refine/{gap_id}.",
        { hash: "#/instance/runtime", selector: "#s-pattern" }),
      guideItem("runtime-agent-idle-timeout", "Agent idle timeout", "Educate and configure",
        "Agent idle timeout cancels an agent subprocess that stops producing activity for too long.",
        "Default: 900 seconds.",
        { hash: "#/instance/runtime", selector: "#s-idle" }),
      guideItem("runtime-agent-hard-cap", "Agent hard cap", "Educate and configure",
        "Agent hard cap is the absolute maximum runtime for an agent subprocess even if it is still active.",
        "Default: 86400 seconds.",
        { hash: "#/instance/runtime", selector: "#s-hard" }),
      guideItem("runtime-worker-memory-limit", "Worker memory limit", "Educate and configure",
        "Worker memory limit caps supervised worker processes when the runtime backend supports resource limits. Zero disables the per-process limit.",
        "Default: 2000 MB.",
        { hash: "#/instance/runtime", selector: "#s-worker-memory" }),
      guideItem("runtime-ui-memory-limit", "UI memory limit", "Educate and configure",
        "UI memory limit caps the supervised UI process when the runtime backend supports resource limits. Zero disables the limit.",
        "Default: 2000 MB.",
        { hash: "#/instance/runtime", selector: "#s-ui-memory" }),
      guideItem("runtime-worker-cpu-priority", "Worker CPU priority", "Educate and configure",
        "Worker CPU priority lowers background worker scheduling priority so Refine is less likely to compete with normal development work.",
        "Default: low.",
        { hash: "#/instance/runtime", selector: "#s-worker-cpu-priority" }),
      guideItem("runtime-resource-isolation", "Resource isolation mode", "Educate and configure",
        "Resource isolation mode controls whether Refine enforces host process limits, tries best effort, or auto-selects based on backend support.",
        "Default: auto.",
        { hash: "#/instance/runtime", selector: "#s-resource-isolation" }),
      guideItem("runtime-agent-limit-pause", "Rate/token limit pause", "Educate and configure",
        "Rate/token limit pause controls how long agents wait before retrying after provider rate-limit or token-limit failures.",
        "Default: 1 minute.",
        { hash: "#/instance/runtime", selector: "#s-agent-limit-pause" }),
      guideItem("runtime-chat-idle-timeout", "Standalone chat idle timeout", "Educate and configure",
        "Standalone chat idle timeout closes inactive standalone chats. Set it to zero to disable automatic close.",
        "Default: 300 seconds.",
        { hash: "#/instance/runtime", selector: "#s-chat-idle" }),
      guideItem("runtime-backlog-promote", "Auto-promote backlog to todo", "Educate and configure",
        "Auto-promote backlog to todo controls how long dispatcher leaves backlog Gaps alone before making them eligible for work.",
        "Default: 1 hour. Use Never to keep backlog manual.",
        { hash: "#/instance/runtime", selector: "#s-backlog-promote" }),
      guideItem("runtime-project-update-pulse", "Target repo update pulse", "Educate and configure",
        "Target repo update pulse checks for local or upstream commits and refreshes this instance's projected state.",
        "Default: 1 minute.",
        { hash: "#/instance/runtime", selector: "#s-project-update-pulse" }),
      guideItem("runtime-file-browser-ignore", "File browser ignore patterns", "Educate and configure",
        "File browser ignore patterns hide noisy files and directories during normal browsing without changing git state.",
        "Default: node_modules, .git, .refine.",
        { hash: "#/instance/runtime", selector: "#s-file-browser-ignore" }),
      guideItem("runtime-ai-provider", "AI provider", "Educate and configure",
        "AI provider chooses which local CLI Refine drives for agents, chat, imports, conflict resolution, and pre-flight checks.",
        "Default: Claude Code.",
        { hash: "#/instance/runtime", selector: "#s-cli" }),
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
      guideItem("project-known-apps", "Known apps", "Educate and configure",
        "Known apps lists the target applications this Refine checkout can attach to. Add an app before switching when the desired repo is missing.",
        "Default: keep the currently attached app.",
        { hash: "#/project/application", selector: "#s-project-select" }),
      guideItem("quality-enabled", "QA enabled", "Educate and configure",
        "QA enabled controls whether the Quality agent runs as part of Gap workflow checks.",
        "Default: keep QA enabled once requirements and instructions are configured.",
        { hash: "#/project/quality", selector: "#s-quality-enabled" }),
      guideItem("quality-gate", "Quality Gate", "Configure",
        "Choose whether QA runs before merge in a Gap worktree or after the shared application rebuild.",
        "Default: pre-merge QA.",
        { hash: "#/project/quality", selector: "#s-quality-timing" }),
      guideItem("quality-regressions-enabled", "Regression checks enabled", "Educate and configure",
        "Regression checks enabled controls whether managed regressions run in workflow QA.",
        "Default: disabled until at least one useful regression exists.",
        { hash: "#/project/quality", selector: "#s-quality-regressions-enabled" }),
      guideItem("quality-regressions", "Regressions", "Educate and optional config",
        "Managed regressions give QA repeatable scenarios to run against the current checkout or workflow environment.",
        "Default: disabled until at least one useful regression exists.",
        { hash: "#/project/quality", selector: "#s-quality-regression-new" }),
      guideItem("quality-regression-title", "Regression title", "Educate and configure",
        "Regression title names a managed QA scenario so it is easy to scan in the regression list.",
        "Default: use a short behavior-oriented name.",
        { hash: "#/project/quality", selector: "#s-quality-regression-new" },
        { canUseDefault: false }),
      guideItem("quality-regression-scenario", "Regression scenario", "Educate and configure",
        "Regression scenario describes the steps and evidence the Quality agent should run against the current checkout or workflow environment.",
        "Default: write concrete navigation, setup, wait, and assertion steps.",
        { hash: "#/project/quality", selector: "#s-quality-regression-new" },
        { canUseDefault: false }),
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
      guideItem("guidance-items", "Guidance", "Educate and optional config",
        "Guidance entries are classified against each Gap before work starts. Matching guidance is prepended to the agent prompt.",
        "Default: no guidance until the project has repeatable instructions that only apply to some work.",
        { hash: "#/project/guidance", selector: "#guidance-add" }),
      guideItem("guidance-name", "Guidance name", "Educate and configure",
        "Guidance name is the short label shown in the guidance list.",
        "Default: use a concise domain or workflow name.",
        { hash: "#/project/guidance", selector: "#guidance-add" },
        { canUseDefault: false }),
      guideItem("guidance-rule", "Guidance rule", "Educate and configure",
        "Guidance rule tells Refine when this guidance should apply to a Gap.",
        "Default: describe the matching condition clearly enough for classification.",
        { hash: "#/project/guidance", selector: "#guidance-add" },
        { canUseDefault: false }),
      guideItem("guidance-instructions", "Guidance instructions", "Educate and configure",
        "Guidance instructions are prepended to the agent prompt when the rule matches a Gap.",
        "Default: include only instructions the agent should follow for matching work.",
        { hash: "#/project/guidance", selector: "#guidance-add" },
        { canUseDefault: false }),
      guideItem("guidance-status", "Guidance status", "Educate and configure",
        "Guidance status enables or disables an entry without deleting its rule and instructions.",
        "Default: enabled for guidance that should currently apply.",
        { hash: "#/project/guidance", selector: "#guidance-add" }),
    ],
  },
  {
    id: "system",
    title: "System",
    description: "Runtime and process management.",
    items: [
      guideItem("process-management", "Process management", "Educate",
        "Process management shows supervised services and controls for background processing, agent scheduling, and the target app.",
        "Default: leave healthy processes running.",
        { hash: "#/system/processes", selector: ".managed-process-table, [data-toggle-background-processes]" },
        { canUseDefault: false }),
      guideItem("process-stop-background", "Stop background processes", "Educate",
        "Stopping background processes keeps the UI running while pausing scheduling, chats, agents, queued rebuilds, and active background jobs.",
        "Default: leave background processes running unless you need a maintenance pause.",
        { hash: "#/system/processes", selector: "[data-toggle-background-processes]" }),
      guideItem("process-pause-agents", "Pause or unpause agents", "Educate",
        "Pausing agents stops new agent subprocesses while leaving the rest of refine available.",
        "Default: agents unpaused.",
        { hash: "#/system/processes", selector: "[data-toggle-agent-processes]" }),
      guideItem("process-agent-processes", "Agent processes", "Educate",
        "Agent processes lists active agent and chat subprocesses so you can inspect runtime state and stop stuck work.",
        "Default: no action unless a process is stale or consuming resources unexpectedly.",
        { hash: "#/system/processes", selector: ".agents-process-table" }),
      guideItem("process-runner-processes", "Runner processes", "Educate",
        "Runner processes shows background work such as rebuilds, merges, cache work, and queued jobs.",
        "Default: let runner work finish unless it is clearly blocked.",
        { hash: "#/system/processes", selector: ".runner-workers-table" }),
      guideItem("performance-overview", "Performance", "Educate",
        "Performance shows local runtime metrics for Refine operations so slow or failing paths are easier to identify.",
        "Default: review only when diagnosing runtime behavior.",
        { hash: "#/system/performance", selector: "#performance-refresh" }),
      guideItem("performance-operation-filter", "Performance operation filter", "Educate",
        "Operation filter narrows recent performance events to one Refine operation name.",
        "Default: all operations.",
        { hash: "#/system/performance", selector: "#performance-operation-filter" }),
      guideItem("performance-outcome-filter", "Performance outcome filter", "Educate",
        "Outcome filter narrows recent performance events to successes or failures.",
        "Default: all outcomes.",
        { hash: "#/system/performance", selector: "#performance-success-filter" }),
      guideItem("performance-limit", "Performance event limit", "Educate",
        "Event limit controls how many performance events are fetched per page.",
        "Default: 50 events.",
        { hash: "#/system/performance", selector: "#performance-limit" }),
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
  guideState.referenceQuery = "";
  clearGuideTargetHighlight();
  try { localStorage.removeItem(GUIDE_CHECKLIST_KEY); } catch {}
  if (redraw) drawGuide();
}

function guideItemsInOrder() {
  return GUIDE_CATEGORIES.flatMap((category) => (
    category.items.map((item) => ({ category, item }))
  ));
}

function guideChecklistItemsInOrder() {
  return guideItemsInOrder().filter(({ category }) => category.checklist);
}

function guideReferenceCategories() {
  return GUIDE_CATEGORIES.filter((category) => !category.checklist);
}

function filteredGuideReferenceCategories() {
  const query = guideState.referenceQuery.trim().toLowerCase();
  const categories = guideReferenceCategories();
  if (!query) return categories;
  return categories.map((category) => {
    const categoryMatches = [category.title, category.description]
      .some((value) => value.toLowerCase().includes(query));
    const items = categoryMatches
      ? category.items
      : category.items.filter((item) => [
          item.title,
          item.kind,
          item.description,
          item.defaultText,
        ].some((value) => value.toLowerCase().includes(query)));
    return { ...category, items };
  }).filter((category) => category.items.length);
}

function guideItemIsChecklist(id) {
  return Boolean(findGuideItem(id)?.category?.checklist);
}

function clearGuideSelection() {
  guideState.activeCategory = "";
  guideState.activeItem = "";
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
  const items = guideChecklistItemsInOrder();
  const done = items.filter(({ item }) => !guideItemIsIncomplete(item.id)).length;
  return { done, total: items.length };
}

function guideChecklistComplete() {
  const progress = guideProgress();
  return progress.total > 0 && progress.done >= progress.total;
}

function firstIncompleteGuideItem({ afterId = "" } = {}) {
  const ordered = guideChecklistItemsInOrder();
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
  const ordered = guideChecklistItemsInOrder();
  const index = ordered.findIndex(({ item }) => item.id === id);
  if (index < 0) return null;
  return ordered[index + offset] || null;
}

function ensureGuideSelection() {
  const active = guideState.activeItem ? findGuideItem(guideState.activeItem) : null;
  if (active && !(active.category.checklist && guideChecklistComplete())) {
    return;
  }
  clearGuideSelection();
  activateGuideItem(firstIncompleteGuideItem());
}

function setGuideItemStatus(id, status, { advance = false } = {}) {
  if (!guideItemIsChecklist(id)) return;
  if (![GUIDE_STATUS_UNCHECKED, GUIDE_STATUS_CHECKED, GUIDE_STATUS_SKIPPED].includes(status)) {
    status = GUIDE_STATUS_UNCHECKED;
  }
  if (status === GUIDE_STATUS_UNCHECKED) {
    delete guideState.statuses[id];
  } else {
    guideState.statuses[id] = status;
  }
  saveGuideChecklist();
  if (guideChecklistComplete()) {
    clearGuideSelection();
    return;
  }
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

async function selectGuideItem(id) {
  const found = findGuideItem(id);
  if (!found) return;
  const previousBodyScrollTop = guideBodyScrollTop();
  activateGuideItem(found);
  drawGuide();
  restoreGuideBodyScrollTop(previousBodyScrollTop);
  await openGuideItemTarget(found.item);
  restoreGuideBodyScrollTop(previousBodyScrollTop);
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
  document.addEventListener("click", (e) => {
    const button = e.target.closest("[data-guide-label-item]");
    if (!button) return;
    e.preventDefault();
    e.stopPropagation();
    const itemId = button.dataset.guideLabelItem || "";
    if (itemId) openGuide({ itemId, openTarget: false });
  });
}

function openGuide(options = {}) {
  guideState.open = true;
  guideState.context = options.context || guideState.context || "";
  const requested = options.itemId ? findGuideItem(options.itemId) : null;
  if (requested) guideState.referenceQuery = "";
  const firstIncomplete = requested || firstIncompleteGuideItem();
  if (firstIncomplete) {
    activateGuideItem(firstIncomplete);
  } else if (!requested && guideChecklistComplete()) {
    clearGuideSelection();
  }
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
  return "Get Started helps you set up for application quickly. Use Reference to understand any configuration.";
}

function drawGuide() {
  const root = document.getElementById("guide-panel");
  if (!root) return;
  const previousBodyScrollTop = guideBodyScrollTop();
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
        Get Started &middot; <strong>${progress.done}</strong> of <strong>${progress.total}</strong> complete
      </div>
      ${GUIDE_CATEGORIES.filter((category) => category.checklist).map(renderGuideCategory).join("")}
      <section class="guide-reference" aria-labelledby="guide-reference-title">
        <div class="guide-reference-header">
          <h3 id="guide-reference-title">Reference</h3>
          <p>Explanations for fields, settings, screens, and daily workflows.</p>
          <div class="guide-reference-search">
            <span class="guide-reference-search-icon">${guideSearchIcon()}</span>
            <input type="search"
                   data-guide-reference-search
                   aria-label="Search reference"
                   placeholder="Search reference"
                   value="${htmlEscape(guideState.referenceQuery)}">
          </div>
        </div>
        ${renderGuideReferenceCategories()}
      </section>
    </div>
  `;
  restoreGuideBodyScrollTop(previousBodyScrollTop);
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
  root.querySelector("[data-guide-reference-search]")?.addEventListener("input", (e) => {
    const selectionStart = e.target.selectionStart;
    const selectionEnd = e.target.selectionEnd;
    guideState.referenceQuery = e.target.value || "";
    drawGuide();
    const nextInput = root.querySelector("[data-guide-reference-search]");
    nextInput?.focus();
    if (nextInput && Number.isInteger(selectionStart) && Number.isInteger(selectionEnd)) {
      nextInput.setSelectionRange(selectionStart, selectionEnd);
    }
  });
  wireGuideResize(root);
}

function guideBodyScrollTop() {
  return document.getElementById("guide-panel")?.querySelector(".guide-body")?.scrollTop ?? 0;
}

function restoreGuideBodyScrollTop(scrollTop) {
  const body = document.getElementById("guide-panel")?.querySelector(".guide-body");
  if (body && Number.isFinite(scrollTop)) body.scrollTop = scrollTop;
}

function renderGuideReferenceCategories() {
  const categories = filteredGuideReferenceCategories();
  if (!categories.length) {
    return `<p class="guide-reference-empty">No reference matches.</p>`;
  }
  return categories.map(renderGuideCategory).join("");
}

function renderGuideCategory(category) {
  const checklistComplete = category.checklist && guideChecklistComplete();
  const searchingReference = !category.checklist && Boolean(guideState.referenceQuery.trim());
  const open = searchingReference
    || (category.checklist && !checklistComplete)
    || guideState.activeCategory === category.id
    || category.items.some((item) => item.id === guideState.activeItem);
  const completeIcon = checklistComplete ? guideCategoryCompleteIcon() : "";
  return `
    <details class="guide-category" data-guide-category="${htmlEscape(category.id)}" ${open ? "open" : ""}>
      <summary>
        <span class="guide-category-summary">
          ${guideChevronIcon()}
          <span>
            <span class="guide-category-title">${htmlEscape(category.title)}</span>
            <span class="guide-category-description">${htmlEscape(category.description)}</span>
          </span>
          ${completeIcon}
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
  const checklist = guideItemIsChecklist(item.id);
  const status = guideItemStatus(item.id);
  const previous = checklist ? guideItemByOffset(item.id, -1) : null;
  const defaultButton = checklist && item.canUseDefault
    ? `<button type="button" class="secondary" data-guide-default="${htmlEscape(item.id)}">Use default</button>`
    : "";
  const actions = checklist
    ? `<div class="guide-item-actions">
          <button type="button" class="secondary" data-guide-prev="${htmlEscape(item.id)}" ${previous ? "" : "disabled"}>Prev</button>
          ${defaultButton}
          <button type="button" class="secondary" data-guide-skip="${htmlEscape(item.id)}">Skip</button>
          <button type="button" data-guide-complete="${htmlEscape(item.id)}">Complete</button>
        </div>`
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
        ${checklist ? guideStatusButton(item, status) : ""}
      </div>
      <div class="guide-item-body" ${open ? "" : "hidden"}>
        <p>${htmlEscape(item.description)}</p>
        <div class="guide-default">${htmlEscape(item.defaultText)}</div>
        ${actions}
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

function guideCategoryCompleteIcon() {
  return `
    <svg class="guide-category-complete" aria-label="Complete" viewBox="0 0 24 24" focusable="false">
      <path d="M20 6 9 17l-5-5"></path>
    </svg>`;
}

function guideSearchIcon() {
  return `
    <svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">
      <circle cx="11" cy="11" r="7"></circle>
      <path d="m20 20-3.5-3.5"></path>
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
