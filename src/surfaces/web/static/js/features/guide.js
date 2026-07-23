// ---- Guide -----------------------------------------------------------------

const GUIDE_WIDTH_KEY = "refine_guide_width";
const GUIDE_CHECKLIST_KEY = "refine_guide_checklist";
const GUIDE_STATE_KEY_PREFIX = "refine_guide_state:";
const GUIDE_DETACHED_STATE_KEY = "__detached__";
const GUIDE_DEFAULT_WIDTH = 360;
const GUIDE_MIN_WIDTH = 280;
const GUIDE_MAX_WIDTH = 560;
const GUIDE_STATUS_UNCHECKED = "unchecked";
const GUIDE_STATUS_CHECKED = "checked";
const GUIDE_STATUS_SKIPPED = "skipped";
const GUIDE_TAB_GET_STARTED = "get-started";
const GUIDE_TAB_REFERENCE = "reference";
let guideHighlightedTarget = null;

const guideState = {
  open: false,
  width: readGuideWidth(),
  statuses: readGuideChecklist(),
  context: "",
  activeCategory: "",
  activeItem: "",
  activeTab: GUIDE_TAB_GET_STARTED,
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
        { hash: "#/node/application", selector: "#s-project-add" },
        { canUseDefault: false }),
      guideItem("quickstart-create-node", "Create node", "Configure",
        "Create a node so this machine, operator, or environment owns its Goals and local runtime settings.",
        "Action: create a node for this machine.",
        { hash: "#/node/application", selector: "#node-add" },
        { canUseDefault: false }),
      guideItem("quickstart-generate-ai", "Generate with AI", "Configure",
        "Let the AI generator draft target-app start, stop, and build instructions from the codebase.",
        "Action: generate the application lifecycle instructions with AI.",
        { hash: "#/node/target-app", selector: "#s-target-generate-ai" },
        { canUseDefault: false }),
      guideItem("quickstart-start", "Start", "Configure",
        "Start the target app from Node process management to confirm the lifecycle instructions work.",
        "Action: start the target app.",
        { hash: "#/node/processes", selector: "#s-target-run-start" },
        { canUseDefault: false }),
    ],
  },
  {
    id: "node",
    title: "Node",
    description: "Settings for this machine and active refine node.",
    items: [
      guideItem("node-educate", "Settings only for this machine", "Educate",
        "Node settings belong to this machine. Use them for local runtime, reporters, and target-app lifecycle instructions that should not apply to every refine node.",
        "Default: each machine keeps its own active node settings.",
        { hash: "#/node/application", selector: "#settings-tabs" }),
      guideItem("node-create", "Create node", "Educate and configure",
        "Create a node when this machine, operator, or environment should own separate Goals and local runtime settings.",
        "Action: create when setting up a new machine.",
        { hash: "#/node/application", selector: "#node-add" },
        { canUseDefault: false }),
      guideItem("node-active", "Activate node", "Educate and configure",
        "The active node controls local ownership and node-scoped settings. Switch it before changing reporters or application lifecycle instructions for another machine.",
        "Default: keep the current active node unless setup is for another machine.",
        { hash: "#/node/application", selector: "[data-node-activate], .filter-pill, .table" },
        { canUseDefault: false }),
      guideItem("node-manage", "Nodes", "Educate and configure",
        "Nodes separate ownership, runtime configuration, reporters, application lifecycle instructions, and optional remote connection details while sharing project-level policy.",
        "Default: keep one active node unless another machine or environment needs separate ownership.",
        { hash: "#/node/application", selector: "#node-add" }),
      guideItem("node-connection", "Node connection", "Educate and configure",
        "Connection fields attach SSH bootstrap and maintenance behavior to an existing Node.",
        "Default: leave connection fields empty unless this Node should be managed over SSH.",
        { hash: "#/node/application", selector: "[data-node-remote-configure]" },
        { canUseDefault: false }),
      guideItem("project-application", "Application", "Educate",
        "Add an existing app path, paste a Git clone URL, or create a new directory. refine will attach the app and initialize .refine state when needed.",
        "Default: keep the current app unless you are setting up a new target app.",
        { hash: "#/node/application", selector: "#s-project-add" }),
      guideItem("project-known-apps", "Known apps", "Educate and configure",
        "Known apps lists the target applications this Refine checkout can attach to. Add an app before switching when the desired repo is missing.",
        "Default: keep the currently attached app.",
        { hash: "#/node/application", selector: "#s-project-select" }),
      guideItem("reporter-add", "Add reporter", "Educate and configure",
        "Reporters identify who submitted or owns feedback. Add the names your team will use when creating Goals.",
        "Action: add the reporter names your team will use before creating Goals.",
        { hash: "#/node/reporters", selector: "#r-add" },
        { canUseDefault: false }),
      guideItem("reporter-manage", "Manage reporters", "Educate and configure",
        "Reporter rename, merge, and remove actions keep the dropdown useful while preserving historical Goal rounds.",
        "Default: leave existing reporters unchanged until duplicates or stale names appear.",
        { hash: "#/node/reporters", selector: "[data-rename], [data-rmerge], [data-rdel]" },
        { canUseDefault: false }),
      guideItem("reporter-merge-into", "Merge into", "Educate and configure",
        "Merge into chooses the destination reporter when consolidating duplicate reporter names.",
        "Default: merge only when you are sure two reporter names represent the same source.",
        { hash: "#/node/reporters", selector: "[data-rmerge]" },
        { canUseDefault: false }),
      guideItem("node-copy-settings-source", "Source node", "Educate and configure",
        "Source node chooses another node to copy Target App Config or Runtime Config settings from into the active node.",
        "Default: copy only when the source node is known to have the desired local configuration.",
        { hash: "#/node/target-app", selector: "#s-application-copy-node" },
        { canUseDefault: false }),
      guideItem("application-ai", "Generate with AI button", "Educate and configure",
        "The AI generator analyses the codebase and writes agent instructions for start, stop, and build plus deterministic test and status checks.",
        "Action: generate with AI, then review the saved lifecycle instructions.",
        { hash: "#/node/target-app", selector: "#s-target-generate-ai" },
        { canUseDefault: false }),
      guideItem("application-agent-subpath", "Agent subpath", "Educate and optional config",
        "Agent subpath changes the working directory for agent and chat subprocesses inside a monorepo. Leave it blank when work should happen from the repository root.",
        "Default: blank, which uses the repo root.",
        { hash: "#/node/target-app", selector: "#s-subpath" }),
      guideItem("application-git-remote", "Git remote", "Educate and optional config",
        "Git remote chooses the shared remote used for both Refine state synchronization and Goal branch publication.",
        "Default: origin.",
        { hash: "#/node/target-app", selector: "#s-git-remote" }),
      guideItem("application-merge-target", "Integration branch", "Educate and optional config",
        "Integration branch chooses the branch Goal worktrees are based on and where approved implementations land. Blank follows the attached project.",
        "Default: blank, following the attached project's current branch.",
        { hash: "#/node/target-app", selector: "#s-merge-target" }),
      guideItem("application-url", "App URL", "Educate and configure",
        "The app URL is opened from the application status indicator when the target app is running.",
        "Default: blank until the local app has a stable URL.",
        { hash: "#/node/target-app", selector: "#s-target-app-url" }),
      guideItem("application-start", "Start", "Educate and configure",
        "Start instructions tell the configured agent how to start the app, repair expected setup issues, and verify it is usable.",
        "Default: blank until the lifecycle generator or operator has described the local start flow.",
        { hash: "#/node/target-app", selector: "#s-target-start-instructions" }),
      guideItem("application-stop", "Stop", "Educate and configure",
        "Stop instructions tell the configured agent how to stop app processes and confirm the app is down.",
        "Default: blank until the lifecycle generator or operator has described the local stop flow.",
        { hash: "#/node/target-app", selector: "#s-target-stop-instructions" }),
      guideItem("application-build", "Build", "Educate and configure",
        "Build instructions tell the configured agent how to rebuild the app, handle setup problems, and report blockers with evidence.",
        "Default: blank unless the integrated target app needs a rebuild before review.",
        { hash: "#/node/target-app", selector: "#s-target-build-instructions" }),
      guideItem("application-auto-build", "Automatic application build", "Educate and configure",
        "Automatic build controls when isolated candidate work is built before review.",
        "Default: in the Goal worktree.",
        { hash: "#/node/target-app", selector: "#s-target-auto-build" }),
      guideItem("application-auto-build-time", "Daily build time", "Educate and optional config",
        "Daily build time chooses the UTC whole-hour build window when Automatic application build is set to Daily.",
        "Default: 00:00 UTC.",
        { hash: "#/node/target-app", selector: "#s-target-auto-build-hour-utc" }),
      guideItem("application-status", "Status command", "Educate and configure",
        "The status command exits 0 only when the app is healthy or running. It is the most deterministic health check when available.",
        "Default: blank until a reliable local status command exists.",
        { hash: "#/node/target-app", selector: "#s-target-status-command" }),
      guideItem("application-working-directory", "Working directory", "Educate and optional config",
        "Working directory is the repo-relative directory where target-app lifecycle agents and checks run. Use it when app scripts live below the repo root.",
        "Default: blank, which uses the repo root.",
        { hash: "#/node/target-app", selector: "#s-target-cwd" }),
      guideItem("application-environment", "Environment overrides", "Educate and optional config",
        "Environment overrides are a JSON object provided as target-app lifecycle context and used by deterministic checks.",
        "Default: an empty JSON object.",
        { hash: "#/node/target-app", selector: "#s-target-env" }),
      guideItem("application-start-timeout", "Start timeout", "Educate and optional config",
        "Start timeout is how long Refine reserves for start lifecycle work before treating it as failed.",
        "Default: 120 seconds.",
        { hash: "#/node/target-app", selector: "#s-target-start-timeout" }),
      guideItem("application-stop-timeout", "Stop timeout", "Educate and optional config",
        "Stop timeout is how long Refine reserves for stop lifecycle work before treating it as failed.",
        "Default: 60 seconds.",
        { hash: "#/node/target-app", selector: "#s-target-stop-timeout" }),
      guideItem("application-build-timeout", "Build timeout", "Educate and optional config",
        "Build timeout is how long Refine reserves for build lifecycle work before leaving build work failed or incomplete.",
        "Default: 300 seconds.",
        { hash: "#/node/target-app", selector: "#s-target-build-timeout" }),
      guideItem("application-test-timeout", "Test timeout", "Educate and optional config",
        "Test timeout is how long Refine waits for the target-app test command before leaving QA failed or incomplete.",
        "Default: 600 seconds.",
        { hash: "#/node/target-app", selector: "#s-target-test-timeout" }),
      guideItem("application-status-timeout", "Status timeout", "Educate and optional config",
        "Status timeout is how long Refine waits for each health or status check before treating it as failed.",
        "Default: 10 seconds.",
        { hash: "#/node/target-app", selector: "#s-target-status-timeout" }),
      guideItem("application-log-path", "Log path", "Educate and optional config",
        "Log path points Refine at a local target-app log file that helps explain start, stop, build, or status failures.",
        "Default: blank.",
        { hash: "#/node/target-app", selector: "#s-target-log-path" }),
      guideItem("application-http-check-url", "HTTP check URL", "Educate and optional config",
        "HTTP check URL is polled from the host; a 2xx response marks the target app healthy.",
        "Default: blank unless the app exposes a stable health endpoint.",
        { hash: "#/node/target-app", selector: "#s-target-http-url" }),
      guideItem("application-tcp-host", "TCP host", "Educate and optional config",
        "TCP host is paired with TCP port when an open socket is the best signal that the app is running.",
        "Default: blank.",
        { hash: "#/node/target-app", selector: "#s-target-tcp-host" }),
      guideItem("application-tcp-port", "TCP port", "Educate and optional config",
        "TCP port is paired with TCP host when an open socket is the best signal that the app is running.",
        "Default: blank.",
        { hash: "#/node/target-app", selector: "#s-target-tcp-port" }),
      guideItem("application-process-check-command", "Process check command", "Educate and optional config",
        "Process check command is a one-line host command that exits 0 when the expected target-app process exists.",
        "Default: blank unless process matching is more reliable than HTTP, TCP, or a status command.",
        { hash: "#/node/target-app", selector: "#s-target-process-command" }),
      guideItem("application-checks", "Optional checks", "Educate and configure",
        "Optional HTTP, TCP, and process checks add confidence, but should stay empty unless they match the app reliably.",
        "Default: all optional checks empty.",
        { hash: "#/node/target-app", selector: "#s-target-http-url" }),
      guideItem("runtime-parallel-run-cap", "Parallel-run cap", "Educate and configure",
        "Parallel-run cap limits how many Goal agent runs this node can launch at once.",
        "Default: 5.",
        { hash: "#/node/runtime", selector: "#s-cap" }),
      guideItem("runtime-branch-name-pattern", "Branch name pattern", "Educate and configure",
        "Branch name pattern controls worktree branch names. Include the Goal id token so branches stay unique.",
        "Default: refine/{goal_id}.",
        { hash: "#/node/runtime", selector: "#s-pattern" }),
      guideItem("runtime-agent-idle-timeout", "Agent idle timeout", "Educate and configure",
        "Agent idle timeout cancels an agent subprocess that stops producing activity for too long.",
        "Default: 900 seconds.",
        { hash: "#/node/runtime", selector: "#s-idle" }),
      guideItem("runtime-agent-hard-cap", "Agent hard cap", "Educate and configure",
        "Agent hard cap is the absolute maximum runtime for an agent subprocess even if it is still active.",
        "Default: 86400 seconds.",
        { hash: "#/node/runtime", selector: "#s-hard" }),
      guideItem("runtime-worker-memory-limit", "Worker memory limit", "Educate and configure",
        "Worker memory limit caps supervised worker processes when the runtime backend supports resource limits. Zero disables the per-process limit.",
        "Default: 2000 MB.",
        { hash: "#/node/runtime", selector: "#s-worker-memory" }),
      guideItem("runtime-ui-memory-limit", "UI memory limit", "Educate and configure",
        "UI memory limit caps the supervised UI process when the runtime backend supports resource limits. Zero disables the limit.",
        "Default: 2000 MB.",
        { hash: "#/node/runtime", selector: "#s-ui-memory" }),
      guideItem("runtime-worker-cpu-priority", "Worker CPU priority", "Educate and configure",
        "Worker CPU priority lowers background worker CPU priority so Refine is less likely to compete with normal development work.",
        "Default: low.",
        { hash: "#/node/runtime", selector: "#s-worker-cpu-priority" }),
      guideItem("runtime-resource-isolation", "Resource isolation mode", "Educate and configure",
        "Resource isolation mode controls whether Refine enforces host process limits, tries best effort, or auto-selects based on backend support.",
        "Default: auto.",
        { hash: "#/node/runtime", selector: "#s-resource-isolation" }),
      guideItem("runtime-agent-limit-pause", "Rate/token limit pause", "Educate and configure",
        "Rate/token limit pause controls how long agents wait before retrying after provider rate-limit or token-limit failures.",
        "Default: 1 minute.",
        { hash: "#/node/runtime", selector: "#s-agent-limit-pause" }),
      guideItem("runtime-chat-idle-timeout", "Standalone chat idle timeout", "Educate and configure",
        "Standalone chat idle timeout closes inactive standalone chats. Set it to zero to disable automatic close.",
        "Default: 300 seconds.",
        { hash: "#/node/runtime", selector: "#s-chat-idle" }),
      guideItem("runtime-backlog-promote", "Auto-promote backlog to todo", "Educate and configure",
        "Auto-promote backlog to todo controls how long the Workflow Engine leaves backlog Goals alone before making them eligible for work.",
        "Default: 1 hour. Use Never to keep backlog manual.",
        { hash: "#/node/runtime", selector: "#s-backlog-promote" }),
      guideItem("runtime-supervisor-stall-threshold", "Supervisor stall threshold", "Educate and configure",
        "Supervisor stall threshold controls how long active Goal work may remain unchanged before the supervisor agent records an actionable stall.",
        "Default: 900 seconds. Detection reports the stall but does not force a merge, reset, or destructive repair.",
        { hash: "#/node/runtime", selector: "#s-supervisor-stall" }),
      guideItem("runtime-project-update-pulse", "Refine state synchronization", "Educate and configure",
        "Refine publishes demand-driven state batches on refine/state without touching application branches. Frequent remote fetches discover both human application commits and Refine state updates without changing the checked-out branch.",
        "Default: 5-second state debounce and 5-minute project update pulse. The pulse can be increased to 1 hour; Sync state now bypasses both.",
        { hash: "#/node/runtime", selector: "#s-state-sync-debounce" }),
      guideItem("runtime-file-browser-ignore", "File browser ignore patterns", "Educate and configure",
        "File browser ignore patterns hide noisy files and directories during normal browsing without changing git state.",
        "Default: node_modules, .git, .refine, run.",
        { hash: "#/node/runtime", selector: "#s-file-browser-ignore" }),
      guideItem("runtime-ai-provider", "AI provider", "Educate and configure",
        "AI provider chooses which local CLI Refine drives for agents, chat, imports, conflict resolution, and pre-flight checks.",
        "Default: Claude Code.",
        { hash: "#/node/runtime", selector: "#s-cli" }),
    ],
  },
  {
    id: "project",
    title: "Governance",
    description: "Project-wide quality, governance, and guidance shared by all refine nodes.",
    items: [
      guideItem("project-educate", "Governance settings", "Educate",
        "Governance settings are stored with the app and shared by all refine nodes. Use them for product intent, quality policy, and guidance.",
        "Action: review these once per target app so shared policy is intentional.",
        { hash: "#/project/governance", selector: "#settings-tabs" },
        { canUseDefault: false }),
      guideItem("quality-gate", "Quality Gate", "Configure",
        "Choose whether QA runs before merge in a Goal worktree or after the shared application build.",
        "Default: pre-merge QA.",
        { hash: "#/project/quality", selector: "#s-quality-timing" }),
      guideItem("application-test", "Target-app tests", "Educate and configure",
        "Target-app test commands are deterministic application lifecycle checks, separate from the Quality agent's plain-text candidate tests.",
        "Default: configure commands for the target app's normal test runners.",
        { hash: "#/node/application", selector: "#s-target-test-commands" }),
      guideItem("quality-requirements", "Business requirements", "Educate and optional config",
        "Business requirements tell the Quality agent what behavior matters for this product.",
        "Default: blank until the project has stable requirements to enforce.",
        { hash: "#/project/quality", selector: "[data-settings-markdown-title='Business requirements']" }),
      guideItem("quality-instructions", "Instructions", "Educate and optional config",
        "Quality instructions tell the Quality agent how to evaluate coverage, risk, and evidence.",
        "Default: blank until the team has QA preferences to enforce.",
        { hash: "#/project/quality", selector: "[data-settings-markdown-title='Instructions']" }),
      guideItem("quality-tests", "Quality tests", "Educate and configure",
        "Quality tests are plain-text outcomes evaluated against every Goal candidate. The configured agent decides how to check each one and records pass or fail with evidence.",
        "Default: no tests, which makes Quality a successful no-op.",
        { hash: "#/project/quality", selector: "[data-settings-markdown-title='Tests']" }),
      guideItem("governance-product", "Product", "Educate and optional config",
        "Product context gives Governance the what and why before implementation work starts.",
        "Default: blank until the product shape is ready to share with agents.",
        { hash: "#/project/governance", selector: "[data-settings-markdown-title='Product']" }),
      guideItem("governance-constitution", "Constitution", "Educate and optional config",
        "The constitution records project principles that should apply across all Goal work.",
        "Default: blank until the team has non-negotiable principles.",
        { hash: "#/project/governance", selector: "[data-settings-markdown-title='Constitution']" }),
      guideItem("governance-rules", "Rules", "Educate and optional config",
        "Rules are short checks Governance applies before implementation. Use Add rule for manual rules or Generate rules when Product and Constitution are filled in.",
        "Default: no rules.",
        { hash: "#/project/governance", selector: "#s-governance-add-rule" }),
      guideItem("guidance-items", "Guidance", "Educate and optional config",
        "Guidance entries are classified against each Goal before work starts. Matching guidance is prepended to the agent prompt.",
        "Default: no guidance until the project has repeatable instructions that only apply to some work.",
        { hash: "#/project/guidance", selector: "#guidance-add" }),
      guideItem("guidance-name", "Guidance name", "Educate and configure",
        "Guidance name is the short label shown in the guidance list.",
        "Default: use a concise domain or workflow name.",
        { hash: "#/project/guidance", selector: "#guidance-add" },
        { canUseDefault: false }),
      guideItem("guidance-rule", "Guidance rule", "Educate and configure",
        "Guidance rule tells Refine when this guidance should apply to a Goal.",
        "Default: describe the matching condition clearly enough for classification.",
        { hash: "#/project/guidance", selector: "#guidance-add" },
        { canUseDefault: false }),
      guideItem("guidance-instructions", "Guidance instructions", "Educate and configure",
        "Guidance instructions are prepended to the agent prompt when the rule matches a Goal.",
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
    title: "Node runtime",
    description: "Process and performance management for this node.",
    items: [
      guideItem("release-workflow", "Semantic releases", "Educate and operate",
        "Releases separates reviewable semantic preparation from explicitly confirmed external publication. Preview the version and gates, prepare and merge the candidate, then publish from synchronized main.",
        "Default: prepare first; publish only after review and merge.",
        { hash: "#/node/releases", selector: "[data-testid='release-planner']" },
        { canUseDefault: false }),
      guideItem("process-management", "Process management", "Educate",
        "Process management shows supervised services and controls for background processing, workflow automation, and the target app.",
        "Default: leave healthy processes running.",
        { hash: "#/node/processes", selector: ".managed-process-table, [data-toggle-workflow]" },
        { canUseDefault: false }),
      guideItem("process-pause-workflow", "Pause or unpause workflow", "Educate",
        "Pausing workflow automation keeps the UI running while agents, queued builds, QA, and active background operations wait.",
        "Default: workflow unpaused.",
        { hash: "#/node/processes", selector: "[data-toggle-workflow]" }),
      guideItem("process-agent-processes", "Agents", "Educate",
        "Agents lists active calls to the configured agent provider, including Goal agents and chat sessions.",
        "Default: no action unless a process is stale or consuming resources unexpectedly.",
        { hash: "#/node/processes", selector: ".agents-process-table" }),
      guideItem("process-runner-processes", "Subprocess history", "Educate",
        "Subprocess history shows supervised work such as builds, repository reconciliation, projection-cache work, and queued operations.",
        "Default: let runner work finish unless it is clearly blocked.",
        { hash: "#/node/processes", selector: ".runner-workers-table" }),
      guideItem("performance-overview", "Performance", "Educate",
        "Performance shows local runtime metrics for Refine operations so slow or failing paths are easier to identify.",
        "Default: review only when diagnosing runtime behavior.",
        { hash: "#/node/performance", selector: "#performance-refresh" }),
      guideItem("performance-operation-filter", "Performance operation filter", "Educate",
        "Operation filter narrows recent performance events to one Refine operation name.",
        "Default: all operations.",
        { hash: "#/node/performance", selector: "#performance-operation-filter" }),
      guideItem("performance-outcome-filter", "Performance outcome filter", "Educate",
        "Outcome filter narrows recent performance events to successes or failures.",
        "Default: all outcomes.",
        { hash: "#/node/performance", selector: "#performance-success-filter" }),
      guideItem("performance-limit", "Performance event limit", "Educate",
        "Event limit controls how many performance events are fetched per page.",
        "Default: 50 events.",
        { hash: "#/node/performance", selector: "#performance-limit" }),
    ],
  },
  {
    id: "main-nav",
    title: "Main nav",
    description: "Common navigation and daily actions.",
    items: [
      guideItem("nav-application-status", "Application status", "Educate",
        "The application status indicator shows target-app state and opens the Node process view for start, stop, build, and checks. Repository reconciliation is automatic.",
        "Action: use this indicator to inspect and control the target app.",
        { selector: "#target-app-indicator", openContextMenu: true },
        { canUseDefault: false }),
      guideItem("nav-agent-status", "Agent status", "Educate",
        "The agent status row in Controls summarizes active or paused agent work and links to Node processes.",
        "Action: use this row to inspect whether agents are running or paused.",
        { selector: "#agent-status-indicator", openContextMenu: true },
        { canUseDefault: false }),
      guideItem("nav-reporter", "Reporter", "Educate and configure",
        "The reporter selector chooses who new Goals are submitted as.",
        "Action: pick or add the reporter before creating Goals.",
        { selector: "#global-reporter", openContextMenu: true },
        { canUseDefault: false }),
      guideItem("nav-create-goal", "Creating Goal", "Educate",
        "Create a Goal when you have an actionable instruction for an agent.",
        "Action: open the Goal form, write the prompt, then save it.",
        { command: "goal.new" },
        { canUseDefault: false }),
      guideItem("nav-import-goals", "Importing Goals", "Educate",
        "Import turns CSV or pasted feedback into editable Goal drafts before saving.",
        "Action: review the drafts before importing them.",
        { command: "goal.import" },
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
  return readGuideStoredState().statuses;
}

function guideStateStorageKey(project = null) {
  const projectState = project || (typeof state !== "undefined" ? state.project : null) || {};
  const path = projectState.target_root || projectState.path || "";
  return `${GUIDE_STATE_KEY_PREFIX}${path ? encodeURIComponent(path) : GUIDE_DETACHED_STATE_KEY}`;
}

function readGuideStoredState(project = null) {
  try {
    const raw = localStorage.getItem(guideStateStorageKey(project)) || "{}";
    const parsed = JSON.parse(raw);
    const statuses = parsed?.statuses && typeof parsed.statuses === "object"
      ? parsed.statuses
      : parsed && typeof parsed === "object"
        ? parsed
        : {};
    const activeItem = typeof parsed?.activeItem === "string" ? parsed.activeItem : "";
    const activeCategory = typeof parsed?.activeCategory === "string" ? parsed.activeCategory : "";
    const activeTab = normalizeGuideTab(parsed?.activeTab)
      || (activeCategory && activeCategory !== GUIDE_TAB_GET_STARTED ? GUIDE_TAB_REFERENCE : GUIDE_TAB_GET_STARTED);
    return {
      statuses,
      activeCategory,
      activeItem,
      activeTab,
      referenceQuery: typeof parsed?.referenceQuery === "string" ? parsed.referenceQuery : "",
    };
  } catch {
    return {
      statuses: {},
      activeCategory: "",
      activeItem: "",
      activeTab: GUIDE_TAB_GET_STARTED,
      referenceQuery: "",
    };
  }
}

function saveGuideChecklist() {
  saveGuideState();
}

function saveGuideState() {
  try {
    localStorage.setItem(guideStateStorageKey(), JSON.stringify({
      statuses: guideState.statuses || {},
      activeCategory: guideState.activeCategory || "",
      activeItem: guideState.activeItem || "",
      activeTab: normalizeGuideTab(guideState.activeTab) || GUIDE_TAB_GET_STARTED,
      referenceQuery: guideState.referenceQuery || "",
    }));
  } catch {}
}

function loadGuideStateForProject(project = null, { redraw = true } = {}) {
  const stored = readGuideStoredState(project);
  guideState.statuses = stored.statuses || {};
  guideState.activeCategory = stored.activeCategory || "";
  guideState.activeItem = stored.activeItem || "";
  guideState.activeTab = stored.activeTab || GUIDE_TAB_GET_STARTED;
  guideState.referenceQuery = stored.referenceQuery || "";
  guideState.context = "";
  clearGuideTargetHighlight();
  if (redraw) drawGuide();
}

function loadGuideStateForCurrentApp({ redraw = true } = {}) {
  loadGuideStateForProject(null, { redraw });
}

function resetGuideState({ redraw = true } = {}) {
  guideState.statuses = {};
  guideState.context = "";
  guideState.activeCategory = "";
  guideState.activeItem = "";
  guideState.activeTab = GUIDE_TAB_GET_STARTED;
  guideState.referenceQuery = "";
  clearGuideTargetHighlight();
  try { localStorage.removeItem(guideStateStorageKey()); } catch {}
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

function normalizeGuideTab(tab) {
  return [GUIDE_TAB_GET_STARTED, GUIDE_TAB_REFERENCE].includes(tab) ? tab : "";
}

function guideTabForCategory(category) {
  return category?.checklist ? GUIDE_TAB_GET_STARTED : GUIDE_TAB_REFERENCE;
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
  guideState.activeTab = guideTabForCategory(found.category);
}

function guideItemByOffset(id, offset) {
  const ordered = guideChecklistItemsInOrder();
  const index = ordered.findIndex(({ item }) => item.id === id);
  if (index < 0) return null;
  return ordered[index + offset] || null;
}

function ensureGuideSelection() {
  if (guideState.activeTab !== GUIDE_TAB_GET_STARTED) return;
  const active = guideState.activeItem ? findGuideItem(guideState.activeItem) : null;
  if (active?.category?.checklist && !guideChecklistComplete()) {
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
  if (guideChecklistComplete()) {
    clearGuideSelection();
    saveGuideChecklist();
    return;
  }
  if (advance) {
    activateGuideItem(firstIncompleteGuideItem({ afterId: id }));
  } else if (!guideItemIsIncomplete(guideState.activeItem)) {
    ensureGuideSelection();
  }
  saveGuideChecklist();
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
  saveGuideState();
  drawGuide();
  openActiveGuideTarget();
}

async function selectGuideItem(id) {
  const found = findGuideItem(id);
  if (!found) return;
  const previousBodyScrollTop = guideBodyScrollTop();
  activateGuideItem(found);
  saveGuideState();
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
  loadGuideStateForCurrentApp({ redraw: false });
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
    if (itemId) openGuide({ itemId });
  });
}

function openGuide(options = {}) {
  guideState.open = true;
  guideState.context = options.context || guideState.context || "";
  const requested = options.itemId ? findGuideItem(options.itemId) : null;
  let shouldOpenTarget = false;
  if (requested) guideState.referenceQuery = "";
  if (requested) {
    activateGuideItem(requested);
    shouldOpenTarget = true;
  } else {
    const requestedTab = normalizeGuideTab(options.tab);
    const firstIncomplete = firstIncompleteGuideItem();
    if (requestedTab) {
      guideState.activeTab = requestedTab;
    } else {
      guideState.activeTab = GUIDE_TAB_GET_STARTED;
    }
    if (guideState.activeTab === GUIDE_TAB_GET_STARTED) {
      ensureGuideSelection();
      if (guideChecklistComplete()) clearGuideSelection();
      shouldOpenTarget = Boolean(firstIncomplete);
    } else {
      const active = guideState.activeItem ? findGuideItem(guideState.activeItem) : null;
      if (active?.category?.checklist) clearGuideSelection();
    }
  }
  if (requested) saveGuideState();
  setGuideWidth(guideState.width, { persist: false });
  drawGuide();
  if (shouldOpenTarget && guideState.activeItem && options.openTarget !== false) openActiveGuideTarget();
}

function selectGuideTab(tab) {
  const normalized = normalizeGuideTab(tab);
  if (!normalized) return;
  guideState.activeTab = normalized;
  if (normalized === GUIDE_TAB_GET_STARTED) {
    ensureGuideSelection();
  } else {
    const active = guideState.activeItem ? findGuideItem(guideState.activeItem) : null;
    if (active?.category?.checklist) clearGuideSelection();
  }
  saveGuideState();
  drawGuide();
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
    return "This app was initialized for refine. Start with Node Application, then create or select the right Node for this machine.";
  }
  if (guideState.context === "app-existing") {
    return "This app already has refine state. Review Governance settings, then select or create the right Node for this machine.";
  }
  if (guideState.context === "no-app") {
    return "No app is attached. Configure Refine from Node Application, then select or create the right Node for this machine.";
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
  guideState.activeTab = normalizeGuideTab(guideState.activeTab) || GUIDE_TAB_GET_STARTED;
  if (guideState.activeTab === GUIDE_TAB_GET_STARTED) ensureGuideSelection();
  root.innerHTML = `
    <div class="guide-resize" id="guide-resize"
         role="separator" aria-orientation="vertical"
         aria-label="Resize Guide"
         data-testid="guide-resize"
         title="Drag to resize"></div>
    <div class="guide-header">
      <h2>Guide</h2>
      <button type="button" class="secondary guide-close" id="guide-close"
              data-testid="guide-close"
              aria-label="Close Guide" title="Close Guide">x</button>
    </div>
    <div class="guide-body">
      ${renderGuideTabStrip()}
      <p class="guide-intro">${htmlEscape(guideContextMessage())}</p>
      ${guideState.activeTab === GUIDE_TAB_GET_STARTED
        ? renderGuideGetStartedPane()
        : renderGuideReferencePane()}
    </div>
  `;
  restoreGuideBodyScrollTop(previousBodyScrollTop);
  root.querySelector("#guide-close")?.addEventListener("click", closeGuide);
  root.querySelectorAll("[data-guide-tab]").forEach((button) => {
    button.addEventListener("click", () => selectGuideTab(button.dataset.guideTab || ""));
  });
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
    saveGuideState();
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

function renderGuideTabStrip() {
  const tabs = [
    { slug: GUIDE_TAB_GET_STARTED, label: "Get Started" },
    { slug: GUIDE_TAB_REFERENCE, label: "Reference" },
  ];
  return `
    <div class="settings-tabs-row guide-tabs-row">
      <nav class="settings-tabs guide-tabs" id="guide-tabs" role="tablist" aria-label="Guide sections">
        ${tabs.map((tab) => `
          <button type="button"
                  class="settings-tab ${guideState.activeTab === tab.slug ? "active" : ""}"
                  data-guide-tab="${tab.slug}"
                  data-testid="guide-tab-${htmlEscape(tab.slug)}"
                  role="tab"
                  aria-selected="${guideState.activeTab === tab.slug ? "true" : "false"}">
            ${htmlEscape(tab.label)}
          </button>`).join("")}
      </nav>
    </div>`;
}

function renderGuideGetStartedPane() {
  return `
    <section class="guide-tab-pane guide-get-started-pane" data-guide-tab-pane="${GUIDE_TAB_GET_STARTED}">
      <div class="guide-get-started-list">
        ${guideChecklistItemsInOrder().map(({ item }) => renderGuideItem(item)).join("")}
      </div>
    </section>`;
}

function renderGuideReferencePane() {
  return `
    <section class="guide-tab-pane guide-reference" data-guide-tab-pane="${GUIDE_TAB_REFERENCE}" aria-labelledby="guide-reference-title">
      <div class="guide-reference-header">
        <h3 id="guide-reference-title">Reference</h3>
        <p>Explanations for fields, settings, screens, and daily workflows.</p>
        <div class="guide-reference-search">
          <span class="guide-reference-search-icon">${guideSearchIcon()}</span>
          <input type="search"
                 data-guide-reference-search
                 data-testid="guide-reference-search"
                 aria-label="Search reference"
                 placeholder="Search reference"
                 value="${htmlEscape(guideState.referenceQuery)}">
        </div>
      </div>
      ${renderGuideReferenceCategories()}
    </section>`;
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
    <details class="guide-category" data-guide-category="${htmlEscape(category.id)}" data-testid="guide-category-${htmlEscape(category.id)}" ${open ? "open" : ""}>
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
    ? `<button type="button" class="secondary" data-guide-default="${htmlEscape(item.id)}" data-testid="guide-default-${htmlEscape(item.id)}">Use default</button>`
    : "";
  const actions = checklist
    ? `<div class="guide-item-actions">
          <button type="button" class="secondary" data-guide-prev="${htmlEscape(item.id)}" data-testid="guide-prev-${htmlEscape(item.id)}" ${previous ? "" : "disabled"}>Prev</button>
          ${defaultButton}
          <button type="button" class="secondary" data-guide-skip="${htmlEscape(item.id)}" data-testid="guide-skip-${htmlEscape(item.id)}">Skip</button>
          <button type="button" data-guide-complete="${htmlEscape(item.id)}" data-testid="guide-complete-${htmlEscape(item.id)}">Complete</button>
        </div>`
    : "";
  return `
    <div class="guide-item ${open ? "active" : ""}" data-guide-item="${htmlEscape(item.id)}" data-testid="guide-item-${htmlEscape(item.id)}">
      <div class="guide-item-summary">
        <button type="button" class="guide-item-open"
                data-guide-open-item="${htmlEscape(item.id)}"
                data-testid="guide-open-item-${htmlEscape(item.id)}"
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
            data-testid="guide-status-${htmlEscape(item.id)}"
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
