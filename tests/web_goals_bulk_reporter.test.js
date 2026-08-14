const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

function browserRuntime({
  apiResult = null,
  modalValue = null,
  refreshError = null,
} = {}) {
  const events = [];
  const toasts = [];
  let modalHtml = "";
  let actionError = null;
  const context = vm.createContext({
    URLSearchParams,
    api: async () => {
      if (apiResult) return apiResult;
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
    toast: (message, kind) => toasts.push({ message, kind }),
    _lastGoalsRender: null,
    _openModal: async (body) => {
      events.push("modal");
      modalHtml = body();
      return modalValue;
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
      openStatus: () => openBulkModal("status"),
    };
  `, context);
  return {
    actionError: () => actionError,
    events,
    modalHtml: () => modalHtml,
    runtime: context.goalsBulkReporterTest,
    toasts,
  };
}

test("bulk status offers review and done without automated workflow states", async () => {
  const browser = browserRuntime();

  await browser.runtime.openStatus();

  assert.deepEqual(browser.events, ["modal"]);
  assert.match(browser.modalHtml(), /<option value="review">review<\/option>/);
  assert.match(browser.modalHtml(), /<option value="done">done<\/option>/);
  assert.match(browser.modalHtml(), /<option value="cancelled">cancelled<\/option>/);
  assert.match(browser.modalHtml(), /Cancelled intentionally stops selected active/);
  assert.match(browser.modalHtml(), /Done remains protected/);
  assert.doesNotMatch(browser.modalHtml(), /<option value="in-progress">/);
  assert.doesNotMatch(browser.modalHtml(), /<option value="qa">/);
  assert.doesNotMatch(browser.modalHtml(), /<option value="ready-merge">/);
  assert.doesNotMatch(browser.modalHtml(), /<option value="build">/);
});

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

test("bulk cancellation surfaces per-Goal partial failure", async () => {
  const browser = browserRuntime({
    modalValue: "cancelled",
    apiResult: { updated: 2, failed: 1, failures: [{ id: "GOAL3" }] },
  });

  await browser.runtime.openStatus();

  assert.deepEqual(browser.toasts, [
    {
      message: "Refine is working on it asynchronously.",
      kind: "info",
    },
    {
      message: "Updated 2 goals; 1 failed or need attention.",
      kind: "warn",
    },
  ]);
});
