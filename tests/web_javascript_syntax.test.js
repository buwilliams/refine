const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const STATIC_JS_ROOT = path.join(
  __dirname,
  "../src/surfaces/web/static/js",
);

function shippedJavaScriptFiles(root) {
  return fs.readdirSync(root, { withFileTypes: true }).flatMap((entry) => {
    const resolved = path.join(root, entry.name);
    if (entry.isDirectory()) return shippedJavaScriptFiles(resolved);
    return entry.isFile() && entry.name.endsWith(".js") ? [resolved] : [];
  });
}

test("every shipped JavaScript asset passes the Node syntax gate", () => {
  const files = shippedJavaScriptFiles(STATIC_JS_ROOT);
  assert.ok(files.length > 0, "expected shipped JavaScript assets");
  for (const file of files) {
    execFileSync(process.execPath, ["--check", file], { stdio: "pipe" });
  }
});

