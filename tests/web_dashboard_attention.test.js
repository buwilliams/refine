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
      confirmRecovery: (confirmed, evidenceId) => dashboardRecoverySetConfirmed(confirmed, evidenceId),
      recoveryReady: () => dashboardRecoveryApplyReady(),
      recoveryPayload: () => dashboardRecoveryApplyPayload(),
      handleRecoveryConflict: (error) => dashboardRecoveryHandleConflict(error),
      recoveryState: () => ({
        phase: dashboardStateRecovery.phase,
        preview: dashboardStateRecovery.preview,
        authority: dashboardStateRecovery.authority,
        confirmedEvidenceId: dashboardStateRecovery.confirmedEvidenceId,
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
      recovery_kind: "missing_baseline",
      target_root: "/target",
      node_id: "default",
    },
  };
}

function recoveryPreview() {
  return {
    version: 1,
    evidence_id: "evidence-123",
    target_identity: "/target",
    repository_identity: "sha256:repository",
    configured_remote: "origin",
    local_state_head: "local-head",
    remote_state_head: "remote-head",
    baseline_status: "missing",
    live_snapshot: "live-snapshot",
    remote_snapshot: "remote-snapshot",
    path_counts: { live_only: 2, remote_only: 3, differing: 4, equal: 5 },
    conflicting_paths: ["goals/LIVE/goal.json", "goals/REMOTE/goal.json"],
    conflicting_paths_truncated: 7,
  };
}

function recoveryResult() {
  return {
    authority: "remote",
    baseline_created: true,
    remote_state_head: "published-head",
    local_state_head: "local-after",
    recovery_location: "refs/refine/state-recovery/evidence-123/remote",
    manifest_path: "/git/refine-state-recoveries/evidence-123-remote.json",
    path_counts: recoveryPreview().path_counts,
    detail: "Remote authority recovery completed.",
    state_sync_health: {
      status: "healthy",
      target_root: "/target",
      node_id: "default",
      revision: 2,
    },
  };
}

test("recovery preview renders complete evidence with neutral unselected authority", () => {
  const runtime = browserRuntime();
  runtime.resetRecovery();
  runtime.setRecoveryPreview(recoveryPreview());

  const html = runtime.renderRecovery(recoveryDashboard());

  for (const value of [
    "/target", "sha256:repository", "origin", "local-head", "remote-head",
    "live-snapshot", "remote-snapshot", "evidence-123", "goals/LIVE/goal.json",
  ]) assert.match(html, new RegExp(value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  assert.match(html, /Live only[\s\S]*Remote only[\s\S]*Differing[\s\S]*Equal/);
  assert.match(html, /7 more/);
  assert.doesNotMatch(html, /value="(?:live|remote)" checked/);
  assert.match(html, /data-recovery-apply disabled/);
});

test("authority and exact-preview confirmation are separate and invalidated safely", () => {
  const runtime = browserRuntime();
  runtime.resetRecovery();
  runtime.setRecoveryPreview(recoveryPreview());

  runtime.selectRecoveryAuthority("live");
  assert.equal(runtime.recoveryReady(), false);
  runtime.confirmRecovery(true, "different-evidence");
  assert.equal(runtime.recoveryReady(), false);
  runtime.confirmRecovery(true, "evidence-123");
  assert.equal(runtime.recoveryReady(), true);
  assert.deepEqual(
    JSON.parse(JSON.stringify(runtime.recoveryPayload())),
    { authority: "live", preview: recoveryPreview() },
  );

  runtime.selectRecoveryAuthority("remote");
  assert.equal(runtime.recoveryReady(), false);

  runtime.confirmRecovery(true, "evidence-123");
  runtime.clearRecoveryForRoute();
  assert.equal(runtime.recoveryState().preview, null);
  assert.equal(runtime.recoveryState().authority, "");
  assert.equal(runtime.recoveryState().confirmedEvidenceId, "");
});

test("busy retains preview while stale rejection requires a fresh review", () => {
  const runtime = browserRuntime();
  runtime.resetRecovery();
  runtime.setRecoveryPreview(recoveryPreview());
  runtime.selectRecoveryAuthority("live");
  runtime.confirmRecovery(true, "evidence-123");

  runtime.handleRecoveryConflict({
    message: "Git busy",
    error: { reason: "git_busy" },
  });
  assert.equal(runtime.recoveryState().phase, "git_busy");
  assert.equal(runtime.recoveryState().preview.evidence_id, "evidence-123");
  assert.equal(runtime.recoveryState().confirmedEvidenceId, "");

  runtime.handleRecoveryConflict({
    message: "Preview stale",
    error: { reason: "stale_preview" },
  });
  assert.equal(runtime.recoveryState().phase, "stale");
  assert.equal(runtime.recoveryState().preview, null);
  assert.equal(runtime.recoveryState().authority, "");
  assert.equal(runtime.recoveryState().previewRefreshRequired, true);
});

test("recovery preview is fetched only for typed eligible health", async () => {
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
  assert.equal(runtime.recoveryState().preview.evidence_id, "evidence-123");
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
    "published-head", "local-after", "refs/refine/state-recovery/evidence-123/remote",
    "/git/refine-state-recoveries/evidence-123-remote.json", "evidence-123",
    "Remote authority recovery completed.",
  ]) assert.match(html, new RegExp(value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
});

test("successful apply paints retained evidence before the health refetch settles", async () => {
  const runtime = browserRuntime();
  const dashboard = recoveryDashboard();
  dashboard.state_sync_health.revision = 1;
  runtime.setRecoveryContext(dashboard);
  runtime.setRecoveryPreview(recoveryPreview());
  runtime.selectRecoveryAuthority("remote");
  runtime.confirmRecovery(true, "evidence-123");
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

test("late apply completion cannot restore recovery state after route context changes", async () => {
  const runtime = browserRuntime();
  runtime.setRecoveryContext(recoveryDashboard());
  runtime.setRecoveryPreview(recoveryPreview());
  runtime.selectRecoveryAuthority("live");
  runtime.confirmRecovery(true, "evidence-123");
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
