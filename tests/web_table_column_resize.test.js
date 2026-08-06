const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const CONFIG = {
  key: "goals-node",
  label: "Node",
  testId: "goals-node-resize",
  storageKey: "refine_goals_node_column_width",
  defaultWidth: 220,
  minWidth: 144,
  maxWidth: 480,
  step: 24,
};

function resizeRuntime(stored = null) {
  const values = new Map();
  if (stored !== null) values.set(CONFIG.storageKey, String(stored));
  const context = vm.createContext({
    sessionStorage: {
      getItem: (key) => values.has(key) ? values.get(key) : null,
      setItem: (key, value) => values.set(key, String(value)),
    },
  });
  vm.runInContext(
    fs.readFileSync(
      path.join(__dirname, "../src/surfaces/web/static/js/table-column-resize.js"),
      "utf8",
    ),
    context,
  );
  vm.runInContext(`
    globalThis.tableColumnResizeTest = {
      clamp: (value, config) => clampTableColumnWidth(value, config),
      read: (config) => readTableColumnWidth(config),
      save: (config, width) => saveTableColumnWidth(config, width),
      render: (config, width) => renderTableColumnResizeHandle(config, width),
    };
  `, context);
  return { runtime: context.tableColumnResizeTest, values };
}

test("table column widths use a useful default and enforce min/max constraints", () => {
  const { runtime } = resizeRuntime();

  assert.equal(runtime.read(CONFIG), 220);
  assert.equal(runtime.clamp(100, CONFIG), 144);
  assert.equal(runtime.clamp(900, CONFIG), 480);
  assert.equal(runtime.clamp(271.6, CONFIG), 272);
});

test("table column width survives rerenders through session state", () => {
  const { runtime, values } = resizeRuntime();

  assert.equal(runtime.save(CONFIG, 356), 356);
  assert.equal(runtime.read(CONFIG), 356);
  assert.equal(values.get(CONFIG.storageKey), "356");
});

test("resize handle renderer exposes pointer and keyboard semantics", () => {
  const { runtime } = resizeRuntime();
  const html = runtime.render(CONFIG, 220);

  assert.match(html, /role="separator" tabindex="0"/);
  assert.match(html, /aria-orientation="vertical"/);
  assert.match(html, /aria-label="Resize Node column"/);
  assert.match(html, /aria-valuemin="144"/);
  assert.match(html, /aria-valuemax="480"/);
  assert.match(html, /aria-valuenow="220"/);
  assert.match(html, /Left and Right Arrow keys/);
});
