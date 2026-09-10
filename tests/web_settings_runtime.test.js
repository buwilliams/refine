const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

function runtimeSettings() {
  const controls = new Map();
  const requests = [];
  const alerts = [];
  const root = {};
  const control = (selector) => {
    if (!controls.has(selector)) {
      controls.set(selector, { value: "", dataset: {}, listeners: {} });
    }
    return controls.get(selector);
  };
  const context = vm.createContext({
    htmlEscape: (value) => String(value),
    HTMLInputElement: class {},
    document: { querySelector: (selector) => selector.includes("data-tab-pane") ? root : null },
    $: control,
    $$: (selector) => selector.split(",").map((s) => control(s.trim())),
    bindOnce: (el, event, listener) => { if (el) el.listeners[event] = listener; },
    bindCommand() {},
    api: async (method, url, body) => {
      requests.push({ method, url, body });
      if (context.failSave) throw new Error("Save rejected");
      return { ok: true, settings: body };
    },
    modalAlert: async (message) => alerts.push(message),
  });
  for (const filename of ["settings.js", "settings_runtime.js"]) {
    vm.runInContext(fs.readFileSync(path.join(__dirname,
      "../src/surfaces/web/static/js/features", filename), "utf8"), context);
  }
  // The real shared autosave code handles the commit event and failed saves;
  // only browser rendering and refresh side effects are stubbed here.
  vm.runInContext(`
    bindSettingsEditableFields = () => {};
    refreshActiveSettingsTab = async () => {};
  `, context);
  return { context, control, requests, alerts };
}

test("Runtime omits the retired email approval setting", () => {
  const { context } = runtimeSettings();
  const html = context.renderNodeRuntimeConfigSections({ auto_approve: "true" }, "Default", "claude");
  assert.doesNotMatch(html, /s-auto-approve|email-request Goals/);
});
