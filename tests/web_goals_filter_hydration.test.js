const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

function browserRuntime({ reporterError = null } = {}) {
  const events = [];
  const banners = [];
  let renderedHtml = "";
  const element = () => ({
    addEventListener() {},
    dataset: {},
    open: false,
    value: "",
  });
  const main = element();
  Object.defineProperty(main, "innerHTML", {
    set(value) {
      events.push("render");
      renderedHtml = value;
    },
  });
  const context = vm.createContext({
    URLSearchParams,
    bindCommand() {},
    bindOnce() {},
    debounce: (fn) => fn,
    document: { getElementById: () => null },
    events,
    history: { replaceState() {} },
    htmlEscape: (value) => String(value)
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;"),
    location: { hash: "#/goals" },
    renderBanners: (items) => banners.push(...items),
    renderNoProjectIfDetached: () => false,
    renderWorkflowVisualization: () => "",
    refreshReporters: async () => {
      events.push("reporters");
      if (reporterError) throw reporterError;
      context.state.reporters = [{ name: "A & B" }];
    },
    state: { project: { nodes: [] }, reporters: [] },
    syncGoalsJiraExportOperation() {},
    workflowStatusLabel: (status) => status,
    STATUS_FILTER_OPTIONS: [],
    $: (selector) => selector === "#main" ? main : element(),
  });
  const source = fs.readFileSync(
    path.join(__dirname, "../src/surfaces/web/static/js/features/goals-list.js"),
    "utf8",
  );
  vm.runInContext(source, context);
  vm.runInContext(`
    ensureGoalsNodeOptions = async () => { events.push("nodes"); };
    refreshGoalsTable = async () => { events.push("table"); };
    globalThis.goalsFilterHydrationTest = { render: renderGoalsList };
  `, context);
  return {
    banners,
    events,
    html: () => renderedHtml,
    render: () => context.goalsFilterHydrationTest.render(),
  };
}

test("Goals cold render hydrates Reporter and Assignee filters first", async () => {
  const browser = browserRuntime();

  await browser.render();

  assert.ok(browser.events.indexOf("reporters") < browser.events.indexOf("render"));
  assert.deepEqual(browser.events, ["nodes", "reporters", "render", "table"]);
  assert.match(browser.html(), /<option value="A &amp; B"[^>]*>A &amp; B<\/option>/);
  assert.equal((browser.html().match(/value="A &amp; B"/g) || []).length, 2);
  assert.deepEqual(browser.banners, []);
});

test("Goals reports filter hydration failures instead of failing the screen", async () => {
  const browser = browserRuntime({ reporterError: new Error("reporters unavailable") });

  await browser.render();

  assert.match(browser.html(), /<h2>Goals<\/h2>/);
  assert.equal(browser.banners.length, 1);
  assert.equal(browser.banners[0].severity, "error");
  assert.equal(
    browser.banners[0].message,
    "Could not load Reporter and Assignee filters: reporters unavailable",
  );
});
