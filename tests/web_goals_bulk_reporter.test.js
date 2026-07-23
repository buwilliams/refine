const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

function browserRuntime({ refreshError = null } = {}) {
  const events = [];
  let modalHtml = "";
  let actionError = null;
  const context = vm.createContext({
    URLSearchParams,
    api: async () => {
      throw new Error("The modal should be cancelled before an update");
    },
    describeGoalsFilter: () => "all goals",
    goalsExcludedIds: new Set(),
    goalsFilterFromHash: () => ({
      status: "", q: "", reporter: "", assignee: "", feature: "", node: "",
      rounds_gte: "", rounds_lte: "",
    }),
    goalsIncludedIds: new Set(),
    goalsSelectAllMatching: true,
    htmlEscape: (value) => String(value)
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;"),
    refreshGoalsListIfCurrent: async () => {},
    refreshReporters: async () => {
      events.push("refresh");
      if (refreshError) throw refreshError;
      context.state.reporters = [
        { id: 1, name: "Buddy Williams" },
        { id: 2, name: "A & B" },
      ];
    },
    resolveBackgroundOperationResponse: async (value) => value,
    showActionError: async (error, title) => {
      events.push("error");
      actionError = { error, title };
    },
    state: { reporters: [] },
    toast() {},
    _lastGoalsRender: null,
    _openModal: async (body) => {
      events.push("modal");
      modalHtml = body();
      return null;
    },
  });
  const source = fs.readFileSync(
    path.join(__dirname, "../src/surfaces/web/static/js/features/goals-bulk.js"),
    "utf8",
  );
  vm.runInContext(source, context);
  vm.runInContext(`
    globalThis.goalsBulkReporterTest = {
      openReporter: () => openBulkModal("reporter"),
    };
  `, context);
  return {
    actionError: () => actionError,
    events,
    modalHtml: () => modalHtml,
    runtime: context.goalsBulkReporterTest,
  };
}

test("bulk reporter loads the reporter model before rendering its picker", async () => {
  const browser = browserRuntime();

  await browser.runtime.openReporter();

  assert.deepEqual(browser.events, ["refresh", "modal"]);
  assert.match(browser.modalHtml(), /<option value="Buddy Williams">Buddy Williams<\/option>/);
  assert.match(browser.modalHtml(), /<option value="A &amp; B">A &amp; B<\/option>/);
});

test("bulk reporter reports a model-load failure instead of showing an empty picker", async () => {
  const failure = new Error("reporters unavailable");
  const browser = browserRuntime({ refreshError: failure });

  await browser.runtime.openReporter();

  assert.deepEqual(browser.events, ["refresh", "error"]);
  assert.equal(browser.modalHtml(), "");
  assert.equal(browser.actionError().error, failure);
  assert.equal(browser.actionError().title, "Could not load reporters");
});
