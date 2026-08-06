const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const themeSource = fs.readFileSync(
  path.join(__dirname, "../src/surfaces/web/static/js/theme.js"),
  "utf8",
);

function themeRuntime({ stored = null, systemDark = false } = {}) {
  const storage = new Map(stored ? [["refine_color_theme", stored]] : []);
  const status = { textContent: "" };
  const attributes = new Map();
  const buttonListeners = new Map();
  const mediaListeners = new Map();
  const events = [];
  const button = {
    dataset: {},
    title: "",
    addEventListener(type, listener) { buttonListeners.set(type, listener); },
    querySelector(selector) { return selector === ".nav-theme-status" ? status : null; },
    setAttribute(name, value) { attributes.set(name, String(value)); },
  };
  const media = {
    matches: systemDark,
    addEventListener(type, listener) { mediaListeners.set(type, listener); },
  };
  const document = {
    documentElement: { dataset: {}, style: {} },
    readyState: "complete",
    getElementById(id) { return id === "btn-theme-toggle" ? button : null; },
  };
  const window = {
    matchMedia() { return media; },
    dispatchEvent(event) { events.push(event); },
  };
  class CustomEvent {
    constructor(type, options) {
      this.type = type;
      this.detail = options?.detail;
    }
  }
  const context = vm.createContext({ CustomEvent, document, localStorage: {
    getItem(key) { return storage.get(key) ?? null; },
    setItem(key, value) { storage.set(key, String(value)); },
  }, window });
  vm.runInContext(themeSource, context);
  return {
    attributes,
    button,
    click() { buttonListeners.get("click")?.(); },
    document,
    events,
    mediaChange(matches) { mediaListeners.get("change")?.({ matches }); },
    status,
    storage,
    theme: window.RefineTheme,
  };
}

test("theme initializes from the system preference and exposes an accessible toggle", () => {
  const browser = themeRuntime({ systemDark: true });

  assert.equal(browser.document.documentElement.dataset.theme, "dark");
  assert.equal(browser.document.documentElement.style.colorScheme, "dark");
  assert.equal(browser.attributes.get("aria-pressed"), "true");
  assert.equal(browser.attributes.get("aria-label"), "Use light mode");
  assert.equal(browser.status.textContent, "On");
  assert.equal(browser.button.dataset.themeBound, "true");
});

test("theme toggle switches modes, persists the choice, and announces the change", () => {
  const browser = themeRuntime({ stored: "dark", systemDark: false });

  assert.equal(browser.theme.current(), "dark", "stored preference should win");
  browser.click();

  assert.equal(browser.document.documentElement.dataset.theme, "light");
  assert.equal(browser.storage.get("refine_color_theme"), "light");
  assert.equal(browser.attributes.get("aria-pressed"), "false");
  assert.equal(browser.attributes.get("aria-label"), "Use dark mode");
  assert.equal(browser.status.textContent, "Off");
  assert.equal(browser.events.at(-1).type, "refine-theme-change");
  assert.equal(browser.events.at(-1).detail.theme, "light");
});

test("system theme changes remain live until the user stores a preference", () => {
  const browser = themeRuntime({ systemDark: false });

  browser.mediaChange(true);
  assert.equal(browser.document.documentElement.dataset.theme, "dark");
  assert.equal(browser.events.at(-1).detail.theme, "dark");

  browser.click();
  browser.mediaChange(true);
  assert.equal(browser.document.documentElement.dataset.theme, "light");
  assert.equal(browser.storage.get("refine_color_theme"), "light");
});
