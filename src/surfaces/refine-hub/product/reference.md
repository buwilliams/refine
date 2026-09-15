# Product reference

Settings and everyday tasks in Refine.

## Get Started

The minimum steps needed to run refine on this app.

<h3 id="quickstart-add-app">Add app</h3>

Register the target app so refine can attach to it. Add an existing path, paste a Git clone URL, or create a new directory.

Action: add the app you want refine to work on.

<h3 id="quickstart-create-node">Create node</h3>

Create a node so this machine, operator, or environment owns its Goals and local runtime settings.

Action: create a node for this machine.

<h3 id="quickstart-generate-ai">Generate with AI</h3>

Let the AI generator draft target-app start, stop, and build instructions from the codebase.

Action: generate the application lifecycle instructions with AI.

<h3 id="quickstart-start">Start</h3>

Start the target app from Node process management to confirm the lifecycle instructions work.

Action: start the target app.

## Node

Settings for this machine and active refine node.

<h3 id="node-educate">Settings only for this machine</h3>

Node settings belong to this machine. Use them for local runtime, reporters, and target-app lifecycle instructions that should not apply to every refine node.

Default: each machine keeps its own active node settings.

<h3 id="node-create">Create node</h3>

Create a node when this machine, operator, or environment should own separate Goals and local runtime settings.

Action: create when setting up a new machine.

<h3 id="node-active">Activate node</h3>

The active node controls local ownership and node-scoped settings. Switch it before changing reporters or application lifecycle instructions for another machine.

Default: keep the current active node unless setup is for another machine.

<h3 id="node-manage">Nodes</h3>

Nodes separate ownership, runtime configuration, reporters, application lifecycle instructions, and optional remote connection details while sharing project-level policy.

Default: keep one active node unless another machine or environment needs separate ownership.

<h3 id="node-connection">Node connection</h3>

Connection fields attach SSH bootstrap and maintenance behavior to an existing Node.

Default: leave connection fields empty unless this Node should be managed over SSH.

<h3 id="project-application">Application</h3>

Add an existing app path, paste a Git clone URL, or create a new directory. refine will attach the app and initialize .refine state when needed.

Default: keep the current app unless you are setting up a new target app.

<h3 id="project-known-apps">Known apps</h3>

Known apps lists the target applications this Refine checkout can attach to. Add an app before switching when the desired repo is missing.

Default: keep the currently attached app.

<h3 id="reporter-add">Add reporter</h3>

Reporters identify who submitted or owns feedback. Add the names your team will use when creating Goals.

Action: add the reporter names your team will use before creating Goals.

<h3 id="reporter-manage">Manage reporters</h3>

Reporter rename, merge, and remove actions keep the dropdown useful while preserving historical Goal rounds.

Default: leave existing reporters unchanged until duplicates or stale names appear.

<h3 id="reporter-merge-into">Merge into</h3>

Merge into chooses the destination reporter when consolidating duplicate reporter names.

Default: merge only when you are sure two reporter names represent the same source.

<h3 id="node-copy-settings-source">Source node</h3>

Source node chooses another node to copy Target App or Runtime settings from into the active node.

Default: copy only when the source node is known to have the desired local configuration.

<h3 id="application-ai">Generate with AI button</h3>

The AI generator analyses the codebase and writes agent instructions for start, stop, and build plus deterministic test and status checks.

Action: generate with AI, then review the saved lifecycle instructions.

<h3 id="application-agent-subpath">Agent subpath</h3>

Agent subpath changes the working directory for agent and chat subprocesses inside a monorepo. Leave it blank when work should happen from the repository root.

Default: blank, which uses the repo root.

<h3 id="application-git-remote">Git remote</h3>

Git remote chooses the shared remote used for both Refine state synchronization and Goal branch publication.

Default: origin.

<h3 id="application-merge-target">Integration branch</h3>

Integration branch chooses the branch Goal worktrees are based on and where approved implementations land. Blank follows the attached project.

Default: blank, following the attached project's current branch.

<h3 id="application-url">App URL</h3>

The app URL is opened from the application status indicator when the target app is running.

Default: blank until the local app has a stable URL.

<h3 id="application-start">Start</h3>

Start instructions tell the configured agent how to start the app, repair expected setup issues, and verify it is usable.

Default: blank until the lifecycle generator or operator has described the local start flow.

<h3 id="application-stop">Stop</h3>

Stop instructions tell the configured agent how to stop app processes and confirm the app is down.

Default: blank until the lifecycle generator or operator has described the local stop flow.

<h3 id="application-build">Build</h3>

Build instructions tell the configured agent how to build the app manually, handle setup problems, and report blockers with evidence.

Default: blank unless the target app has a manual build flow.

<h3 id="application-status">Status command</h3>

The status command exits 0 only when the app is healthy or running. It is the most deterministic health check when available.

Default: blank until a reliable local status command exists.

<h3 id="application-working-directory">Working directory</h3>

Working directory is the repo-relative directory where target-app lifecycle agents and checks run. Use it when app scripts live below the repo root.

Default: blank, which uses the repo root.

<h3 id="application-environment">Environment overrides</h3>

Environment overrides are a JSON object provided as target-app lifecycle context and used by deterministic checks.

Default: an empty JSON object.

<h3 id="application-start-timeout">Start timeout</h3>

Start timeout is how long Refine reserves for start lifecycle work before treating it as failed.

Default: 120 seconds.

<h3 id="application-stop-timeout">Stop timeout</h3>

Stop timeout is how long Refine reserves for stop lifecycle work before treating it as failed.

Default: 60 seconds.

<h3 id="application-build-timeout">Build timeout</h3>

Build timeout is how long Refine reserves for build lifecycle work before leaving build work failed or incomplete.

Default: 300 seconds.

<h3 id="application-test-timeout">Test timeout</h3>

Test timeout is how long Refine waits for the target-app test command before leaving Quality failed or incomplete.

Default: 600 seconds.

<h3 id="application-status-timeout">Status timeout</h3>

Status timeout is how long Refine waits for each health or status check before treating it as failed.

Default: 10 seconds.

<h3 id="application-log-path">Log path</h3>

Log path points Refine at a local target-app log file that helps explain start, stop, build, or status failures.

Default: blank.

<h3 id="application-http-check-url">HTTP check URL</h3>

HTTP check URL is polled from the host; a 2xx response marks the target app healthy.

Default: blank unless the app exposes a stable health endpoint.

<h3 id="application-tcp-host">TCP host</h3>

TCP host is paired with TCP port when an open socket is the best signal that the app is running.

Default: blank.

<h3 id="application-tcp-port">TCP port</h3>

TCP port is paired with TCP host when an open socket is the best signal that the app is running.

Default: blank.

<h3 id="application-process-check-command">Process check command</h3>

Process check command is a one-line host command that exits 0 when the expected target-app process exists.

Default: blank unless process matching is more reliable than HTTP, TCP, or a status command.

<h3 id="application-checks">Optional checks</h3>

Optional HTTP, TCP, and process checks add confidence, but should stay empty unless they match the app reliably.

Default: all optional checks empty.

<h3 id="runtime-parallel-run-cap">Parallel-run cap</h3>

Leave Parallel-run cap blank for automatic shared-host admission. Entering a positive number is an explicit node-level absolute override.

Default: Automatic (blank).

<h3 id="runtime-automatic-resource-budget-percent">Automatic resource budget percent</h3>

Automatic mode applies this percentage to detected logical CPU cores and currently available memory. The 70 percent default leaves 30 percent for Docker, target applications, and other shared-host work.

Default: 70 percent.

<h3 id="runtime-branch-name-pattern">Branch name pattern</h3>

Branch name pattern controls worktree branch names. Include the Goal id token so branches stay unique.

Default: refine/{goal_id}.

<h3 id="runtime-agent-idle-timeout">Agent idle timeout</h3>

Agent idle timeout cancels an agent subprocess that stops producing activity for too long.

Default: 900 seconds.

<h3 id="runtime-agent-hard-cap">Agent hard cap</h3>

Agent hard cap is the absolute maximum runtime for an agent subprocess even if it is still active.

Default: 86400 seconds.

<h3 id="runtime-worker-memory-limit">Worker memory limit</h3>

Worker memory limit caps supervised worker processes when the runtime backend supports resource limits. Zero disables the per-process limit.

Default: 2000 MB.

<h3 id="runtime-ui-memory-limit">UI memory limit</h3>

UI memory limit caps the supervised UI process when the runtime backend supports resource limits. Zero disables the limit.

Default: 2000 MB.

<h3 id="runtime-worker-cpu-priority">Worker CPU priority</h3>

Worker CPU priority lowers background worker CPU priority so Refine is less likely to compete with normal development work.

Default: low.

<h3 id="runtime-resource-isolation">Resource isolation mode</h3>

Resource isolation mode controls whether Refine enforces host process limits, tries best effort, or auto-selects based on backend support.

Default: auto.

<h3 id="runtime-agent-limit-pause">Rate/token limit pause</h3>

Rate/token limit pause controls how long agents wait before retrying after provider rate-limit or token-limit failures.

Default: 1 minute.

<h3 id="runtime-chat-idle-timeout">Standalone chat idle timeout</h3>

Standalone chat idle timeout closes inactive standalone chats. Set it to zero to disable automatic close.

Default: 300 seconds.

<h3 id="runtime-backlog-promote">Auto-promote backlog to todo</h3>

Auto-promote backlog to todo controls how long the Workflow Engine leaves backlog Goals alone before making them eligible for work.

Default: 1 hour. Use Never to keep backlog manual.

<h3 id="runtime-worktree-cleanup">Inactive worktree hibernation</h3>

Todo Goals are state-only. Once admitted to Plan, each Goal round gets an isolated worktree used through Implement, Quality, and Governance. A clean Goal worktree in any status is hibernated after the retention delay when no live process or operation uses it; its recoverable branch recreates the checkout on demand, and ignored build and cache content is discarded with the checkout. Dirty, standalone, and state worktrees remain protected. Exact Refine candidate refs already present in the target branch may be retired locally and upstream.

Default: immediately. The cleanup worker scans every registered app at least once per minute.

<h3 id="runtime-project-update-pulse">Refine state synchronization</h3>

Refine publishes demand-driven state batches on refine/state without touching application branches. Frequent remote fetches discover both human application commits and Refine state updates without changing the checked-out branch.

Default: 5-second state debounce and 5-minute project update pulse. The pulse can be increased to 1 hour; Sync state now bypasses both.

<h3 id="runtime-file-browser-ignore">File browser ignore patterns</h3>

File browser ignore patterns hide noisy files and directories during normal browsing without changing git state.

Default: node_modules, .git, .refine, run.

<h3 id="runtime-ai-provider">AI provider</h3>

AI provider chooses which local CLI Refine drives for agents, chat, imports, conflict resolution, and pre-flight checks.

Default: Claude Code.

## Skills

Reusable agent instructions and the activity that triggers them.

<h3 id="skills-configure">Skills</h3>

Skills contain reusable instructions, scope, and optional inputs. Assign an existing Skill to Goal steps, system events, or custom actions in Settings → Prompts. Each assignment has its own run settings; editing a Skill updates its instructions everywhere it is used. Refine determines the expected result from the trigger.

Default: Plan, Implement, Quality, and Governance have required Skills at their Enter triggers.

<h3 id="skills-run">Run a Skill</h3>

Enabled Custom Skills appear in the rail’s Skills section and the command palette. Fill in any requested inputs to open the Skill in its own agent tab, independently of Goals. Use the tab to follow the work or stop the agent.

Action: expand Skills in the rail and choose Add skill… to create a Skill. Manage Skills opens the shared resource editor.

<h3 id="application-test">Target-app tests</h3>

Target-app test commands remain application lifecycle checks. Candidate Quality is configured through Skills.

Default: configure the target app's normal test runners.

## Node runtime

Process management for this node.

<h3 id="release-workflow">Semantic releases</h3>

Use a Release Skill from the rail’s Skills section to preview a semantic version, prepare a reviewable Goal, and publish after approval through the existing release commands.

Default: prepare first; publish only after review and merge.

<h3 id="process-management">Process management</h3>

Control contains the selected node's centralized list of Refine processes. It shows the target app, daemon, background workers, and current agents with live resource use and direct controls.

Default: leave healthy processes running.

<h3 id="process-pause-workflow">Pause or unpause workflow</h3>

Pausing blocks new Goal admission and quiesces automatic Git sync and inactive-worktree cleanup at safe boundaries. Already active Goal executions continue unless stopped separately. Unpausing makes admission and those repository workers eligible again.

Default: workflow unpaused.

## Main nav

Common navigation and daily actions.

<h3 id="nav-application-status">Application status</h3>

Control shows target application status and icon actions for Start/Stop, Build, and Check status. Hover labels and accessible names identify each action. Configure its instructions in Settings → Target App. Repository reconciliation is automatic.

Action: open Control to inspect and control the target app.

<h3 id="nav-agent-status">Running agents</h3>

Open Control to inspect running agents and available worker actions. Workflow status is also available in Settings → Workspace controls & support.

Action: inspect individual agent processes in Control.

<h3 id="nav-reporter">Reporter</h3>

The reporter selector chooses who new Goals are submitted as.

Action: pick or add the reporter before creating Goals.

<h3 id="nav-create-goal">Creating Goal</h3>

Create a Goal when you have an actionable instruction for an agent.

Action: open the Goal form, write the prompt, then save it.

<h3 id="nav-import-goals">Importing Goals</h3>

Import turns CSV or pasted feedback into editable Goal drafts before saving.

Action: review the drafts before importing them.

<h3 id="nav-report-bug">Report refine bug</h3>

Use the refine issue action for product feedback, bugs, and feature requests about refine itself.

Action: open the issue form when refine itself needs feedback or a fix.
