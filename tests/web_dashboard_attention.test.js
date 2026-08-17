const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

function browserRuntime() {
  const context = vm.createContext({
    URLSearchParams,
    location: { hash: "#/" },
    state: { currentRoute: "dashboard", dashboard: null },
    htmlEscape(value) { return String(value); },
    sharedNodeScopeFromHash() { return "current"; },
    captureNodeContextGeneration() { return 0; },
    isNodeContextGenerationCurrent(generation) { return generation === 0; },
  });
  for (const file of [
    "../src/surfaces/web/static/js/features/goals-list.js",
    "../src/surfaces/web/static/js/features/dashboard_state_recovery.js",
    "../src/surfaces/web/static/js/features/dashboard.js",
  ]) {
    vm.runInContext(fs.readFileSync(path.join(__dirname, file), "utf8"), context);
  }
  vm.runInContext(`
    globalThis.dashboardAttentionTest = {
      goalsHash: (item, reporter, scope) =>
        dashboardAttentionGoalsHash(item, reporter, scope),
      renderSyncHealth: (dashboard) => renderDashboardStateSyncHealth(dashboard),
      resetRecovery: () => { dashboardStateRecovery = newDashboardStateRecovery(""); },
      clearRecoveryForRoute: () => clearDashboardStateRecoveryForRouteChange(),
      setRecoveryPreview: (preview) => dashboardRecoverySetPreview(preview),
      setRecoveryContext: (dashboard) => {
        state.currentRoute = "dashboard";
        state.dashboard = dashboard;
        dashboardStateRecovery = newDashboardStateRecovery(dashboardRecoveryContextKey(dashboard));
      },
      setRecoveryRoute: (route) => { state.currentRoute = route; },
      renderRecovery: (dashboard) => renderDashboardStateRecovery(dashboard),
      selectRecoveryAuthority: (authority) => dashboardRecoverySelectAuthority(authority),
      confirmRecovery: (confirmed, fingerprint) => dashboardRecoverySetConfirmed(confirmed, fingerprint),
      recoveryFingerprint: () => dashboardRecoveryFingerprint(dashboardStateRecovery.preview),
      previewFingerprint: (preview) => dashboardRecoveryFingerprint(preview),
      recoveryReady: () => dashboardRecoveryApplyReady(),
      recoveryPayload: () => dashboardRecoveryApplyPayload(),
      handleRecoveryConflict: (error) => dashboardRecoveryHandleConflict(error),
      recoveryState: () => ({
        phase: dashboardStateRecovery.phase,
        preview: dashboardStateRecovery.preview,
        authority: dashboardStateRecovery.authority,
        confirmedFingerprint: dashboardStateRecovery.confirmedFingerprint,
        previewRefreshRequired: dashboardStateRecovery.previewRefreshRequired,
      }),
      setDashboardApi: (implementation) => { dashboardApi = implementation; },
      setRecoveryUiHooks: (redraw, refresh) => {
        redrawDashboardRecovery = redraw;
        refreshDashboard = refresh;
      },
      applyRecovery: () => applyDashboardStateRecovery(),
      reconcileRecovery: (dashboard) => reconcileDashboardStateRecovery(dashboard),
      completeRecovery: (result) => {
        dashboardStateRecovery.phase = "success";
        dashboardStateRecovery.result = result;
      },
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
      last_attempt_id: 42,
      last_attempt_source: "project_sync_operation",
      last_success_at: "2026-08-14T12:00:00Z",
      failure_since: "2026-08-15T11:55:00Z",
      stale_since: "2026-08-14T12:15:00Z",
      last_error: "git fetch failed",
      last_conflict_report_id: "report-42",
      last_conflict_report_location: "/run/8082/state-sync-conflicts/latest.json",
    },
  });

  assert.match(html, /data-state-sync-status="failed"/);
  assert.match(html, /All-node counts: local projection; non-authoritative/);
  assert.match(html, /Last attempt.*2026-08-15T12:00:00Z/);
  assert.match(html, /Attempt.*42 \(project_sync_operation\)/);
  assert.match(html, /Last success.*2026-08-14T12:00:00Z/);
  assert.match(html, /Failure since.*2026-08-15T11:55:00Z/);
  assert.match(html, /Stale since.*2026-08-14T12:15:00Z/);
  assert.match(html, /Complete conflict report report-42/);
});

test("dashboard state sync health renders below the workflow status grid", () => {
  const source = fs.readFileSync(
    path.join(__dirname, "../src/surfaces/web/static/js/features/dashboard.js"),
    "utf8",
  );
  const dashboardTemplate = source.slice(
    source.indexOf("renderInto(dash, `"),
    source.indexOf("`, () => {", source.indexOf("renderInto(dash, `")),
  );

  assert.ok(
    dashboardTemplate.indexOf("renderWorkflowVisualization")
      < dashboardTemplate.indexOf("renderDashboardStateSyncHealth"),
  );
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

function recoveryDashboard() {
  return {
    node_filter: "current",
    active_node_id: "default",
    state_sync_health: {
      status: "failed",
      last_conflict_report_id: "report-123",
      target_root: "/target",
      node_id: "default",
      failure_since: "failure-1",
      revision: 1,
    },
  };
}

function recoveryPreview() {
  return {
    version: 2,
    configured_remote: "origin",
    local_state_head: "local-head",
    remote_state_head: "remote-head",
    merge_base: "base-head",
    ancestry: "diverged",
    live_pending_paths: ["goals/PENDING/goal.json"],
    local_paths: ["goals/LOCAL/goal.json"],
    remote_paths: ["goals/REMOTE/goal.json"],
    resolvable_paths: ["nodes.json"],
    conflicts: [
      { path: "goals/SHARED/goal.json", summary: "goal SHARED: both nodes changed status" },
    ],
    detail: "Diverged from base-head.",
  };
}

function recoveryResult() {
  return {
    ok: true,
    attempts: 1,
    recovered: true,
    recovery: {
      ok: true,
      authority: "remote",
      overrides: [],
      local_state_head: "local-after",
      remote_state_head: "published-head",
      settled_paths: ["goals/SHARED/goal.json"],
      retained_refs: ["refs/refine/retained/live-abcdef123456"],
      detail: "Remote authority recovery completed.",
    },
    sync: { ok: true },
    detail: "Remote authority recovery completed.",
    health_settled: true,
    state_sync_health: {
      status: "healthy",
      target_root: "/target",
      node_id: "default",
      revision: 2,
    },
  };
}

test("recovery preview renders divergence evidence with neutral unselected authority", () => {
  const runtime = browserRuntime();
  runtime.resetRecovery();
  runtime.setRecoveryPreview(recoveryPreview());

  const html = runtime.renderRecovery(recoveryDashboard());

  for (const value of [
    "origin", "diverged", "local-head", "remote-head", "base-head",
    "goals/SHARED/goal.json", "both nodes changed status",
  ]) assert.match(html, new RegExp(value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  assert.match(html, /Live pending[\s\S]*Local only[\s\S]*Remote only[\s\S]*Resolvable[\s\S]*Contested/);
  assert.doesNotMatch(html, /value="(?:live|remote)" checked/);
  assert.match(html, /data-recovery-apply disabled/);
});

test("authority and exact-preview confirmation are separate and invalidated safely", () => {
  const runtime = browserRuntime();
  runtime.resetRecovery();
  runtime.setRecoveryPreview(recoveryPreview());

  runtime.selectRecoveryAuthority("live");
  assert.equal(runtime.recoveryReady(), false);
  runtime.confirmRecovery(true, "different-divergence");
  assert.equal(runtime.recoveryReady(), false);
  runtime.confirmRecovery(true, runtime.recoveryFingerprint());
  assert.equal(runtime.recoveryReady(), true);
  assert.deepEqual(
    JSON.parse(JSON.stringify(runtime.recoveryPayload())),
    { authority: "live", paths: [] },
  );

  runtime.selectRecoveryAuthority("remote");
  assert.equal(runtime.recoveryReady(), false);

  runtime.confirmRecovery(true, runtime.recoveryFingerprint());
  runtime.clearRecoveryForRoute();
  assert.equal(runtime.recoveryState().preview, null);
  assert.equal(runtime.recoveryState().authority, "");
  assert.equal(runtime.recoveryState().confirmedFingerprint, "");
});

test("busy retains preview while stale rejection requires a fresh review", () => {
  const runtime = browserRuntime();
  runtime.resetRecovery();
  runtime.setRecoveryPreview(recoveryPreview());
  runtime.selectRecoveryAuthority("live");
  runtime.confirmRecovery(true, runtime.recoveryFingerprint());

  runtime.handleRecoveryConflict({
    message: "Git busy",
    error: { reason: "git_busy" },
  });
  assert.equal(runtime.recoveryState().phase, "git_busy");
  assert.equal(runtime.recoveryState().preview.merge_base, "base-head");
  assert.equal(runtime.recoveryState().confirmedFingerprint, "");

  runtime.handleRecoveryConflict({
    message: "Preview stale",
    error: { reason: "stale_preview" },
  });
  assert.equal(runtime.recoveryState().phase, "stale");
  assert.equal(runtime.recoveryState().preview, null);
  assert.equal(runtime.recoveryState().authority, "");
  assert.equal(runtime.recoveryState().previewRefreshRequired, true);
});

test("recovery preview is fetched only for conflict-shaped failed health", async () => {
  const runtime = browserRuntime();
  runtime.resetRecovery();
  let requests = 0;
  runtime.setDashboardApi(async () => {
    requests++;
    return recoveryPreview();
  });

  await runtime.reconcileRecovery({
    ...recoveryDashboard(),
    state_sync_health: { status: "failed", target_root: "/target", node_id: "default" },
  });
  assert.equal(requests, 0);

  await runtime.reconcileRecovery(recoveryDashboard());
  assert.equal(requests, 1);
  assert.equal(runtime.recoveryState().preview.merge_base, "base-head");
});

test("successful recovery retains evidence and renders authoritative health clearing", () => {
  const runtime = browserRuntime();
  runtime.resetRecovery();
  runtime.setRecoveryPreview(recoveryPreview());
  runtime.completeRecovery(recoveryResult());

  const html = runtime.renderRecovery({
    ...recoveryDashboard(),
    state_sync_health: { status: "healthy", target_root: "/target", node_id: "default" },
  });

  assert.match(html, /State-sync error cleared:<\/strong> Yes/);
  for (const value of [
    "published-head", "local-after", "refs/refine/retained/live-abcdef123456",
    "goals/SHARED/goal.json", "Remote authority recovery completed.",
  ]) assert.match(html, new RegExp(value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
});

test("successful apply paints retained evidence before the health refetch settles", async () => {
  const runtime = browserRuntime();
  const dashboard = recoveryDashboard();
  dashboard.state_sync_health.revision = 1;
  runtime.setRecoveryContext(dashboard);
  runtime.setRecoveryPreview(recoveryPreview());
  runtime.selectRecoveryAuthority("remote");
  runtime.confirmRecovery(true, runtime.recoveryFingerprint());
  runtime.setDashboardApi(async () => recoveryResult());
  const phases = [];
  runtime.setRecoveryUiHooks(
    () => phases.push(runtime.recoveryState().phase),
    async () => { throw new Error("health refresh unavailable"); },
  );

  await runtime.applyRecovery();

  assert.deepEqual(phases, ["applying", "success", "success"]);
  assert.equal(runtime.recoveryState().phase, "success");
  assert.match(runtime.renderRecovery(dashboard), /State-sync error cleared:<\/strong> Yes/);
});

test("a newer conflict episode replaces retained success with fresh neutral evidence", async () => {
  const runtime = browserRuntime();
  const dashboard = recoveryDashboard();
  runtime.setRecoveryContext(dashboard);
  runtime.setRecoveryPreview(recoveryPreview());
  runtime.selectRecoveryAuthority("remote");
  runtime.confirmRecovery(true, runtime.recoveryFingerprint());
  let previewRequests = 0;
  runtime.setDashboardApi(async (method) => {
    if (method === "POST") return recoveryResult();
    previewRequests++;
    return { ...recoveryPreview(), merge_base: "base-head-2" };
  });
  runtime.setRecoveryUiHooks(() => {}, async () => {});

  await runtime.applyRecovery();
  assert.equal(runtime.recoveryState().phase, "success");

  await runtime.reconcileRecovery(dashboard);
  assert.equal(runtime.recoveryState().phase, "success");
  assert.equal(previewRequests, 0);

  await runtime.reconcileRecovery({
    ...dashboard,
    state_sync_health: {
      ...dashboard.state_sync_health,
      failure_since: "failure-2",
      revision: 3,
    },
  });

  assert.equal(previewRequests, 1);
  assert.equal(runtime.recoveryState().phase, "ready");
  assert.equal(runtime.recoveryState().preview.merge_base, "base-head-2");
  assert.equal(runtime.recoveryState().authority, "");
  assert.equal(runtime.recoveryReady(), false);
});

test("late apply completion cannot restore recovery state after route context changes", async () => {
  const runtime = browserRuntime();
  runtime.setRecoveryContext(recoveryDashboard());
  runtime.setRecoveryPreview(recoveryPreview());
  runtime.selectRecoveryAuthority("live");
  runtime.confirmRecovery(true, runtime.recoveryFingerprint());
  let resolveApply;
  runtime.setDashboardApi(() => new Promise((resolve) => { resolveApply = resolve; }));
  runtime.setRecoveryUiHooks(() => {}, async () => {});

  const apply = runtime.applyRecovery();
  runtime.setRecoveryRoute("goals");
  runtime.clearRecoveryForRoute();
  resolveApply(recoveryResult());
  await apply;

  assert.equal(runtime.recoveryState().phase, "idle");
  assert.equal(runtime.recoveryState().preview, null);
  assert.equal(runtime.recoveryState().authority, "");
});
