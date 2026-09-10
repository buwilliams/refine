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

test("Runtime Config renders Auto-approve off by default and reflects saved booleans", () => {
  const { context } = runtimeSettings();
  for (const [settings, enabled] of [
    [{}, false], [{ auto_approve: "false" }, false], [{ auto_approve: false }, false],
    [{ auto_approve: "true" }, true], [{ auto_approve: true }, true],
  ]) {
    const html = context.renderNodeRuntimeConfigSections(settings, "Email worker", "claude");
    assert.match(html, /Node: Email worker/);
    assert.match(html, /data-testid="s-auto-approve-edit"/);
    assert.match(html, /email-request Goals processed by this node/);
    assert.match(html, /including requests already waiting in Review/);
    assert.match(html, /first observed Review time/);
    const select = html.match(/<select id="s-auto-approve"[^>]*>([\s\S]*?)<\/select>/)[1];
    assert.match(select, new RegExp(`value="${enabled}" selected`));
    assert.doesNotMatch(select, new RegExp(`value="${!enabled}" selected`));
  }
});

test("Auto-approve uses the shared edit commit autosave and restores a rejected change", async () => {
  const { context, control, requests, alerts } = runtimeSettings();
  const flag = control("#s-auto-approve");
  flag.value = "false";
  context.bindNodeRuntimeConfigControls();
  assert.equal(flag.dataset.settingsSavedValue, "false");
  assert.equal(typeof flag.listeners["settings-editable-commit"], "function");
  flag.value = "true";
  await flag.listeners["settings-editable-commit"]();
  assert.equal(requests[0].method, "PATCH");
  assert.equal(requests[0].url, "/api/settings");
  assert.equal(requests[0].body.auto_approve, "true");
  assert.equal(flag.dataset.settingsSavedValue, "true");

  context.failSave = true;
  flag.value = "false";
  await flag.listeners["settings-editable-commit"]();
  assert.equal(requests[1].body.auto_approve, "false");
  assert.equal(flag.value, "true");
  assert.equal(alerts.length, 1);
});
