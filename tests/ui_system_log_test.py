"""Browser-side UI notices/errors flow into Toolbar > System log."""
from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path


def main() -> int:
    node = shutil.which("node")
    if not node:
        print("node unavailable; skipped UI system log JS test")
        return 0

    root = Path(__file__).resolve().parents[1]
    common_js = (root / "refine_ui/static/js/common.js").read_text(encoding="utf-8")
    toolbar_js = (
        root / "refine_ui/static/js/features/toolbar.js"
    ).read_text(encoding="utf-8")
    script = f"""
const assert = require("assert");
const vm = require("vm");
const listeners = {{}};
global.localStorage = {{
  getItem: () => "",
  setItem: () => {{}},
  removeItem: () => {{}},
}};
global.location = {{ hash: "#/system/processes", pathname: "/" }};
global.window = {{
  innerHeight: 900,
  addEventListener: (name, fn) => {{
    if (!listeners[name]) listeners[name] = [];
    listeners[name].push(fn);
  }},
}};
global.document = {{
  body: {{ appendChild: () => {{}} }},
  createElement: () => ({{
    className: "",
    textContent: "",
    remove: () => {{}},
  }}),
  addEventListener: () => {{}},
  querySelector: () => null,
  querySelectorAll: () => [],
}};
const fetchCalls = [];
global.fetch = (url, options = {{}}) => {{
  fetchCalls.push({{
    url,
    body: options.body ? JSON.parse(options.body) : null,
  }});
  return Promise.resolve({{ ok: true }});
}};
global.assert = assert;
global.fetchCalls = fetchCalls;
global.listeners = listeners;
const context = vm.createContext(global);

vm.runInContext({json.dumps(common_js)}, context);

vm.runInContext(`
toast("Queued before toolbar", "info");
assert.strictEqual(state.pendingSystemOperations.length, 1);
`, context);

vm.runInContext({json.dumps(toolbar_js)}, context);

vm.runInContext(`
drainPendingSystemOperations();
assert.strictEqual(systemOperationState.messages.length, 1);
assert.strictEqual(systemOperationState.messages[0].message, "Queued before toolbar");
assert.strictEqual(systemOperationState.messages[0].status, "info");

toast("Saved", "success");
assert.strictEqual(systemOperationState.messages.at(-1).message, "Saved");
assert.strictEqual(systemOperationState.messages.at(-1).status, "complete");

toast("Broken", "error");
assert.strictEqual(systemOperationState.messages.at(-1).message, "Broken");
assert.strictEqual(systemOperationState.messages.at(-1).status, "error");
assert.strictEqual(fetchCalls.at(-1).url, "/api/activity/ui-error");
assert.strictEqual(fetchCalls.at(-1).body.message, "Broken");
assert.strictEqual(fetchCalls.at(-1).body.source, "toast");

const formEl = {{ textContent: "", style: {{ display: "none" }} }};
showFormError(formEl, "Inline failed", {{ source: "form-test" }});
assert.strictEqual(formEl.textContent, "Inline failed");
assert.strictEqual(formEl.style.display, "");
assert.strictEqual(systemOperationState.messages.at(-1).message, "Inline failed");
assert.strictEqual(fetchCalls.at(-1).body.source, "form-test");

const beforeDuplicate = systemOperationState.messages.length;
recordSystemOperation({{ message: "Saved", status: "complete" }});
assert.strictEqual(systemOperationState.messages.length, beforeDuplicate);

assert.strictEqual(listeners.error.length, 1);
listeners.error[0]({{
  message: "Runtime exploded",
  filename: "app.js",
  lineno: 12,
  colno: 5,
  error: new Error("Runtime exploded"),
}});
assert.strictEqual(systemOperationState.messages.at(-1).message, "Runtime exploded");
assert.strictEqual(systemOperationState.messages.at(-1).status, "error");
assert.strictEqual(fetchCalls.at(-1).body.source, "window.error");

assert.strictEqual(listeners.unhandledrejection.length, 1);
listeners.unhandledrejection[0]({{ reason: new Error("Promise exploded") }});
assert.strictEqual(systemOperationState.messages.at(-1).message, "Promise exploded");
assert.strictEqual(fetchCalls.at(-1).body.source, "unhandledrejection");
`, context);
"""
    subprocess.run([node, "-"], input=script, text=True, check=True)
    print("UI system log tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
