const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const staticRoot = path.join(__dirname, "../src/surfaces/web/static/js");
const commonSource = fs.readFileSync(path.join(staticRoot, "common.js"), "utf8");

function namedFunctionSource(source, name) {
  const plain = source.indexOf(`function ${name}(`);
  const async = source.indexOf(`async function ${name}(`);
  const start = [plain, async].filter((offset) => offset >= 0).sort((a, b) => a - b)[0];
  assert.notEqual(start, undefined, `missing function ${name}`);
  const bodyStart = source.indexOf("{", source.indexOf(")", start));
  let depth = 0;
  for (let index = bodyStart; index < source.length; index += 1) {
    if (source[index] === "{") depth += 1;
    if (source[index] === "}") depth -= 1;
    if (depth === 0) return source.slice(start, index + 1);
  }
  throw new Error(`unterminated function ${name}`);
}

test("API callers can render an expected degraded status without recording Activity", async () => {
  const recorded = [];
  const context = vm.createContext({
    state: { screenDataCache: new Map() },
    screenDataCacheablePath() { return false; },
    async fetch() {
      return {
        ok: false,
        status: 404,
        statusText: "Not Found",
        async json() {
          return { error: { message: "checkout-local binary is unavailable" } };
        },
      };
    },
    recordUiError(...args) { recorded.push(args); },
  });
  vm.runInContext(`${namedFunctionSource(commonSource, "api")}; globalThis.apiForTest = api;`, context);

  await assert.rejects(
    context.apiForTest("GET", "/api/system/source", undefined, { recordError: false }),
    /checkout-local binary is unavailable/,
  );
  assert.equal(recorded.length, 0);

  await assert.rejects(
    context.apiForTest("GET", "/api/failing"),
    /checkout-local binary is unavailable/,
  );
  assert.equal(recorded.length, 1);
});

test("identical UI errors are persisted at most once per cooldown", () => {
  let now = 100_000;
  const notices = [];
  const requests = [];
  const constants = commonSource.match(
    /const UI_ERROR_RECORD_COOLDOWN_MS[^;]+;\s*const UI_ERROR_RECORD_LIMIT[^;]+;/,
  );
  assert.ok(constants);
  const context = vm.createContext({
    state: {},
    Date: { now() { return now; } },
    location: { hash: "#/settings/processes", pathname: "/" },
    recordUiNotice(...args) { notices.push(args); },
    fetch(...args) {
      requests.push(args);
      return Promise.resolve({ ok: true });
    },
  });
  vm.runInContext(
    `${constants[0]}\n${namedFunctionSource(commonSource, "recordUiError")}; globalThis.recordForTest = recordUiError;`,
    context,
  );

  const details = { source: "api", path: "/api/system/source", status: 404 };
  context.recordForTest("checkout-local binary is unavailable", details);
  context.recordForTest("checkout-local binary is unavailable", details);
  assert.equal(notices.length, 1);
  assert.equal(requests.length, 1);

  now += 30_000;
  context.recordForTest("checkout-local binary is unavailable", details);
  assert.equal(notices.length, 2);
  assert.equal(requests.length, 2);
});

test("passive source-status reads opt out of Activity error recording", () => {
  const settings = fs.readFileSync(path.join(staticRoot, "features/settings.js"), "utf8");
  const releases = fs.readFileSync(
    path.join(staticRoot, "features/settings_releases.js"),
    "utf8",
  );
  assert.match(
    settings,
    /api\("GET", "\/api\/system\/source", undefined, \{ recordError: false \}\)/,
  );
  assert.equal(
    (releases.match(/\{ cache: false, recordError: fetchRemote \}/g) || []).length,
    2,
  );
});
