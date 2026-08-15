const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

function browserRuntime() {
  const context = vm.createContext({
    URLSearchParams,
    location: { hash: "#/" },
    htmlEscape(value) { return String(value); },
  });
  for (const file of [
    "../src/surfaces/web/static/js/features/goals-list.js",
    "../src/surfaces/web/static/js/features/dashboard.js",
  ]) {
    vm.runInContext(fs.readFileSync(path.join(__dirname, file), "utf8"), context);
  }
  vm.runInContext(`
    globalThis.dashboardAttentionTest = {
      goalsHash: (item, reporter, scope) =>
        dashboardAttentionGoalsHash(item, reporter, scope),
      renderSyncHealth: (dashboard) => renderDashboardStateSyncHealth(dashboard),
    };
  `, context);
  return context.dashboardAttentionTest;
}

test("failed Goal recovery attention links to the selected reporter and node scope", () => {
  const runtime = browserRuntime();

  const hash = runtime.goalsHash(
    { filter: { status: "failed" } },
    "Buddy Williams",
    "current",
  );

  assert.equal(
    hash,
    "#/goals?status=failed&reporter=Buddy+Williams&node=current",
  );
});

test("degraded state sync labels aggregate counts and exposes freshness metadata", () => {
  const runtime = browserRuntime();
  const html = runtime.renderSyncHealth({
    aggregate_counts_authoritative: false,
    all_node_counts_label: "local projection; non-authoritative",
    state_sync_health: {
      status: "failed",
      last_attempt_at: "2026-08-15T12:00:00Z",
      last_success_at: "2026-08-14T12:00:00Z",
      failure_since: "2026-08-15T11:55:00Z",
      stale_since: "2026-08-14T12:15:00Z",
      last_error: "git fetch failed",
    },
  });

  assert.match(html, /data-state-sync-status="failed"/);
  assert.match(html, /All-node counts: local projection; non-authoritative/);
  assert.match(html, /Last attempt.*2026-08-15T12:00:00Z/);
  assert.match(html, /Last success.*2026-08-14T12:00:00Z/);
  assert.match(html, /Failure since.*2026-08-15T11:55:00Z/);
  assert.match(html, /Stale since.*2026-08-14T12:15:00Z/);
});

test("browser SSE reconciles state sync surfaces from authoritative endpoints", () => {
  const common = fs.readFileSync(
    path.join(__dirname, "../src/surfaces/web/static/js/common.js"),
    "utf8",
  );
  assert.match(common, /addEventListener\("state_sync_health"/);
  assert.match(common, /state\.currentRoute === "dashboard"\) refreshDashboard\(\)/);
  assert.match(common, /state\.currentRoute === "logs"\) loadLogs\(\)/);
  assert.match(common, /refreshCurrentSettingsSurface\(\)/);
});
