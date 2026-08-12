const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const UI_ROOT = path.join(__dirname, "../src/surfaces/web/static/js");

function jsFiles(dir) {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) return jsFiles(full);
    return entry.name.endsWith(".js") ? [full] : [];
  });
}

function uiSources() {
  return jsFiles(UI_ROOT).map((file) => ({
    name: path.relative(UI_ROOT, file),
    lines: fs.readFileSync(file, "utf8").split("\n"),
  }));
}

// Static, single-line selector arguments. Anything interpolated is dynamic and
// cannot be checked here.
const SELECTOR_CALL =
  /(?:\$\$?|querySelector|querySelectorAll|closest|matches)\(\s*(["'])((?:(?!\1)[^\\\r\n])*)\1/g;
const TEST_ID = /data-testid="([^"$]*)"/g;

// An empty call — `()` — in a selector or a test id is never legitimate: real
// pseudo-classes carry an argument (`:nth-child(2)`), and an id never has one.
// It is the signature of a rewrite that substituted an accessor call into a
// string, which the vm-based suites cannot catch because they have no DOM: the
// selector only throws when a browser parses it.
test("no selector literal in the web UI contains an empty call", () => {
  const offenders = [];
  for (const { name, lines } of uiSources()) {
    lines.forEach((line, i) => {
      for (const match of line.matchAll(SELECTOR_CALL)) {
        if (match[2].includes("()")) {
          offenders.push(`${name}:${i + 1}  ${match[2]}`);
        }
      }
    });
  }
  assert.deepEqual(offenders, []);
});

test("no rendered test id in the web UI contains an empty call", () => {
  const offenders = [];
  for (const { name, lines } of uiSources()) {
    lines.forEach((line, i) => {
      for (const match of line.matchAll(TEST_ID)) {
        if (match[1].includes("()")) {
          offenders.push(`${name}:${i + 1}  ${match[1]}`);
        }
      }
    });
  }
  assert.deepEqual(offenders, []);
});

// Both helpers are globals shared by every screen, so a screen that calls them
// without `dom-morph.js` loaded fails only at runtime.
test("dom-morph.js is loaded before the screens that use it", () => {
  const indexHtml = fs.readFileSync(
    path.join(__dirname, "../src/surfaces/web/static/index.html"),
    "utf8",
  );
  const order = [...indexHtml.matchAll(/<script src="([^"]+)"><\/script>/g)]
    .map((m) => m[1]);
  const idiomorph = order.findIndex((src) => src.includes("idiomorph"));
  const domMorph = order.findIndex((src) => src.includes("dom-morph.js"));

  assert.ok(idiomorph >= 0, "idiomorph must be vendored and loaded");
  assert.ok(domMorph >= 0, "dom-morph.js must be loaded");
  assert.ok(idiomorph < domMorph, "idiomorph must load before dom-morph.js");

  const users = order
    .map((src, i) => ({ src, i }))
    .filter(({ src }) => src.includes("/features/") || src.endsWith("command-registry.js"));
  for (const { src, i } of users) {
    assert.ok(i > domMorph, `${src} must load after dom-morph.js`);
  }
});

test("Reporter onboarding loads before router and init", () => {
  const indexHtml = fs.readFileSync(
    path.join(__dirname, "../src/surfaces/web/static/index.html"),
    "utf8",
  );
  const order = [...indexHtml.matchAll(/<script src="([^"]+)"><\/script>/g)]
    .map((match) => match[1]);
  const onboarding = order.indexOf("/static/js/features/reporter-onboarding.js");
  const router = order.indexOf("/static/js/router.js");
  const init = order.indexOf("/static/js/init.js");

  assert.ok(onboarding >= 0, "Reporter onboarding must be loaded");
  assert.ok(onboarding < router, "Reporter onboarding must load before router.js");
  assert.ok(onboarding < init, "Reporter onboarding must load before init.js");
});
