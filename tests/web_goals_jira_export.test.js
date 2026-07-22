const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

function browserRuntime() {
  const storage = new Map();
  const root = {
    hidden: true,
    innerHTML: "",
    querySelector() { return null; },
  };
  const exportButton = { disabled: false, textContent: "Export for Jira" };
  const apiCalls = [];
  let apiHandler = async () => ({ logs: [] });
  const context = vm.createContext({
    Blob,
    URL,
    clearTimeout,
    console,
    document: {
      body: { appendChild() {} },
      createElement() { return { click() {}, remove() {} }; },
    },
    fmtTime: (value) => value,
    htmlEscape: (value) => String(value)
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;"),
    localStorage: {
      getItem(key) { return storage.get(key) ?? null; },
      removeItem(key) { storage.delete(key); },
      setItem(key, value) { storage.set(key, String(value)); },
    },
    setTimeout,
    showActionError: async () => {},
    toast() {},
    withButtonBusy: async (_button, _label, action) => action(),
    $: (selector) => {
      if (selector === "#goals-jira-export-operation") return root;
      if (selector === "#bulk-export-jira") return exportButton;
      return null;
    },
    api: async (...args) => {
      apiCalls.push(args);
      return apiHandler(...args);
    },
  });
  const source = fs.readFileSync(
    path.join(__dirname, "../src/surfaces/web/static/js/features/goals-bulk.js"),
    "utf8",
  );
  vm.runInContext(source, context);
  vm.runInContext(`
    globalThis.goalsJiraExportTest = {
      cancel: cancelGoalsJiraExportOperation,
      hide: hideGoalsJiraExportOperation,
      html: goalsJiraExportOperationHtml,
      render: renderGoalsJiraExportOperation,
      store: writeGoalsJiraExportOperation,
    };
  `, context);
  return {
    apiCalls,
    button: exportButton,
    root,
    runtime: context.goalsJiraExportTest,
    setApi(handler) { apiHandler = handler; },
    storage,
  };
}

function operation(status, overrides = {}) {
  return {
    id: "op-123",
    owner: "goals:jira-export",
    status,
    progress: { message: "Looking up commit evidence", completed: 2, total: 5 },
    result: {},
    ...overrides,
  };
}

test("running Jira export renders bounded logs, progress, and Cancel only", () => {
  const browser = browserRuntime();
  const logs = Array.from({ length: 10 }, (_, index) => ({
    severity: "info",
    message: `stage ${index}`,
    datetime: `2026-07-22T12:00:${String(index).padStart(2, "0")}Z`,
  }));

  browser.runtime.render(operation("running"), logs);

  assert.equal(browser.root.hidden, false);
  assert.match(browser.root.innerHTML, /data-testid="goals-jira-export-status">Running/);
  assert.match(browser.root.innerHTML, /2 of 5/);
  assert.match(browser.root.innerHTML, /data-testid="goals-jira-export-cancel"/);
  assert.doesNotMatch(browser.root.innerHTML, /data-testid="goals-jira-export-hide"/);
  assert.doesNotMatch(browser.root.innerHTML, /data-testid="goals-jira-export-download"/);
  assert.equal(
    (browser.root.innerHTML.match(/data-testid="goals-jira-export-log-entry"/g) || []).length,
    8,
  );
  assert.doesNotMatch(browser.root.innerHTML, /stage 0/);
  assert.match(browser.root.innerHTML, /stage 9/);
});

test("successful Jira export exposes Download and Hide only after completion", () => {
  const browser = browserRuntime();
  browser.runtime.render(operation("complete", {
    progress: { message: "Jira CSV ready", completed: 5, total: 5 },
    result: { export: { csv: "Summary\nDone", filename: "jira.csv" } },
  }), []);

  assert.match(browser.root.innerHTML, /data-testid="goals-jira-export-status">Complete/);
  assert.match(browser.root.innerHTML, /data-testid="goals-jira-export-download"/);
  assert.match(browser.root.innerHTML, /data-testid="goals-jira-export-hide">Hide/);
  assert.doesNotMatch(browser.root.innerHTML, /data-testid="goals-jira-export-cancel"/);
});

test("failed Jira export keeps error evidence and terminal Dismiss without Download", () => {
  const browser = browserRuntime();
  browser.runtime.render(operation("failed", {
    error: { code: "jira_export_failed", message: "git evidence lookup failed" },
  }), [{ severity: "error", message: "Jira CSV export failed" }]);

  assert.match(browser.root.innerHTML, /data-testid="goals-jira-export-status">Failed/);
  assert.match(browser.root.innerHTML, /data-testid="goals-jira-export-error">git evidence lookup failed/);
  assert.match(browser.root.innerHTML, /data-testid="goals-jira-export-logs" open/);
  assert.match(browser.root.innerHTML, /Jira CSV export failed/);
  assert.match(browser.root.innerHTML, /data-testid="goals-jira-export-hide">Dismiss/);
  assert.doesNotMatch(browser.root.innerHTML, /data-testid="goals-jira-export-download"/);
});

test("Cancel uses the public operation route and Hide refuses non-terminal state", async () => {
  const browser = browserRuntime();
  browser.runtime.store("op-123");
  const running = operation("running");
  browser.runtime.render(running, []);
  assert.equal(browser.runtime.hide(running), false);
  assert.equal(browser.storage.has("refine_goals_jira_export_operation"), true);

  browser.setApi(async (method, requestPath) => {
    if (method === "POST") return { operation: operation("cancelled") };
    if (requestPath.includes("/logs")) return { logs: [{ message: "Operation cancelled" }] };
    return {};
  });
  await browser.runtime.cancel("op-123");

  assert.equal(browser.apiCalls[0][0], "POST");
  assert.equal(browser.apiCalls[0][1], "/api/operations/op-123/cancel");
  assert.equal(JSON.stringify(browser.apiCalls[0][2]), "{}");
  assert.match(browser.root.innerHTML, /data-testid="goals-jira-export-status">Cancelled/);
  assert.match(browser.root.innerHTML, /data-testid="goals-jira-export-hide">Dismiss/);
  assert.equal(browser.runtime.hide(operation("cancelled")), true);
  assert.equal(browser.storage.has("refine_goals_jira_export_operation"), false);
  assert.equal(browser.root.hidden, true);
});
