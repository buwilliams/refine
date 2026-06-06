// ---- Command registry -------------------------------------------------------

const commandRegistry = new Map();

function registerCommand(def) {
  if (!def || !def.id) throw new Error("command id required");
  commandRegistry.set(def.id, {
    aliases: [],
    keywords: [],
    group: "General",
    danger: false,
    ...def,
  });
}

function commandContext(extra = {}) {
  return {
    route: state.currentRoute || "",
    settingsTab: typeof readSettingsTab === "function" ? readSettingsTab() : "",
    hash: location.hash || "#/",
    project: state.project || null,
    reporter: state.lastReporter || "",
    ...extra,
  };
}

function commandShortcutLabel() {
  const platform = navigator.platform || "";
  const mac = /Mac|iPhone|iPad|iPod/i.test(platform);
  return mac ? "Cmd+K" : "Ctrl+K";
}

function commandIsVisible(command, ctx) {
  return typeof command.visible === "function" ? command.visible(ctx) !== false : true;
}

function commandIsEnabled(command, ctx) {
  return typeof command.enabled === "function" ? command.enabled(ctx) !== false : true;
}

function parseCommandInput(command, input, ctx) {
  if (typeof command.parse !== "function") return {};
  const parsed = command.parse(input || "", ctx);
  return parsed && typeof parsed === "object" ? parsed : {};
}

function commandMatchesExplicitAlias(command, query) {
  const q = (query || "").trim();
  if (!q) return null;
  const names = [command.id, ...(command.aliases || [])].filter(Boolean);
  for (const name of names) {
    const n = String(name).toLowerCase();
    const lower = q.toLowerCase();
    if (lower === n) return { name, tail: "" };
    if (lower.startsWith(n + " ")) {
      return { name, tail: q.slice(String(name).length).trim() };
    }
  }
  return null;
}

function fuzzyScoreCommand(command, query) {
  const q = (query || "").trim().toLowerCase();
  if (!q) return 1;
  const explicit = commandMatchesExplicitAlias(command, q);
  if (explicit) return 1000 - explicit.name.length;
  const haystack = [
    command.title || "",
    command.group || "",
    command.id || "",
    ...(command.aliases || []),
    ...(command.keywords || []),
  ].join(" ").toLowerCase();
  if (haystack.includes(q)) return 600 - haystack.indexOf(q);

  let score = 0;
  let pos = 0;
  let streak = 0;
  for (const ch of q) {
    const found = haystack.indexOf(ch, pos);
    if (found === -1) return -1;
    streak = found === pos ? streak + 1 : 0;
    score += 8 + streak * 3 - Math.min(6, found - pos);
    pos = found + 1;
  }
  return score;
}

function searchCommands(query = "", ctx = commandContext()) {
  const results = [];
  for (const command of commandRegistry.values()) {
    if (!commandIsVisible(command, ctx)) continue;
    const score = fuzzyScoreCommand(command, query);
    if (score < 0) continue;
    let params = {};
    try {
      params = parseCommandInput(command, query, ctx);
    } catch (e) {
      params = { __parseError: e.message || String(e) };
    }
    results.push({
      command,
      score,
      params,
      enabled: commandIsEnabled(command, ctx),
    });
  }
  results.sort((a, b) => {
    if (a.enabled !== b.enabled) return a.enabled ? -1 : 1;
    if (b.score !== a.score) return b.score - a.score;
    return String(a.command.title).localeCompare(String(b.command.title));
  });
  return results;
}

async function runCommand(id, options = {}) {
  const command = commandRegistry.get(id);
  if (!command) {
    toast(`Unknown command: ${id}`, "error");
    return null;
  }
  const ctx = commandContext(options.context || {});
  if (!commandIsVisible(command, ctx)) {
    toast("Command is not available here.", "warn");
    return null;
  }
  if (!commandIsEnabled(command, ctx)) {
    toast(command.disabledMessage || "Command is disabled.", "warn");
    return null;
  }
  let params = { ...(options.params || {}) };
  if (options.input && typeof command.parse === "function") {
    try {
      params = { ...params, ...parseCommandInput(command, options.input, ctx) };
    } catch (e) {
      toast(e.message || String(e), "error");
      return null;
    }
  }
  if (params.__parseError) {
    toast(params.__parseError, "error");
    return null;
  }
  if (!options.skipConfirm) {
    let ok = true;
    if (typeof command.confirm === "function") {
      ok = await command.confirm(params, ctx);
    } else if (command.danger) {
      ok = await modalConfirm(`Run ${command.title}?`, {
        title: command.title,
        okLabel: "Run",
        danger: true,
      });
    }
    if (!ok) return null;
  }
  try {
    const runParams = { ...ctx, ...params };
    return await command.run(runParams, ctx);
  } catch (e) {
    await showActionError(e, command.errorTitle || "Command failed");
    return null;
  }
}

function bindCommand(target, id, options = {}) {
  const el = typeof target === "string" ? document.querySelector(target) : target;
  if (!el) return;
  el.addEventListener("click", async (e) => {
    if (options.preventDefault !== false) e.preventDefault();
    if (typeof options.beforeRun === "function") await options.beforeRun(e);
    await runCommand(id, {
      ...options,
      context: {
        ...(options.context || {}),
        event: e,
        button: el,
      },
    });
  });
}

window.RefineCommands = {
  register: registerCommand,
  run: runCommand,
  bind: bindCommand,
  search: searchCommands,
  context: commandContext,
  all: () => Array.from(commandRegistry.values()),
  shortcutLabel: commandShortcutLabel,
};
