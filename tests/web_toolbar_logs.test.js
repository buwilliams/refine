const assert = require("node:assert/strict");
const test = require("node:test");
const { openApp, apiFixture, GOAL, SKIP } = require("./support/web_app");

test("Toolbar logs unify type filters, retained search, flat evidence, and opt-in System tail", { skip: SKIP }, async () => {
  const calls = [];
  const app = await openApp({ fixture(pathname, request) {
    if (pathname.startsWith("/api/activity")) {
      const url = new URL(request.url());
      calls.push({ method: request.method(), path: pathname, q: url.searchParams.get("q"), type: url.searchParams.get("log_type"), body: request.postDataJSON() });
      return { activity: [{ id: "p1:stdout:0", datetime: "2026-09-12T12:00:00Z", severity: "unknown", log_type: "stdout", process_id: "p1", message: "Retained needle output", details: { evidence: "needle <script>" } }], cursors: { "process:p1:stdout": 22 }, page: { total: 401, has_more: true } };
    }
    return apiFixture(pathname);
  }});
  try {
    await app.page.goto(`${app.origin}/#/logs`);
    const panel = app.page.getByTestId("toolbar-system-panel");
    await panel.waitFor();
    assert.equal(await app.page.getByTestId("nav-logs").count(), 0);
    assert.equal(await panel.getByTestId("log-follow").getAttribute("aria-pressed"), "false");
    assert.equal(calls.filter(c => c.path === "/api/activity/tail").length, 0);
    await app.page.evaluate(() => recordSystemOperation({ message: "Normal notice", status: "info" }));
    await panel.locator(".goal-log-line").filter({ hasText: "Normal notice" }).waitFor();
    await panel.getByTestId("log-follow").click();
    await panel.locator(".goal-log-line").filter({ hasText: "Retained needle output" }).waitFor();
    assert.ok(calls.some(c => c.path === "/api/activity/tail" && c.method === "POST"));
    await panel.getByTestId("log-follow").click();
    await panel.getByLabel("Type", { exact: true }).selectOption("stdout");
    const searchResponse = app.page.waitForResponse(response => response.url().includes("q=needle") && response.url().includes("log_type=stdout"));
    await panel.getByLabel("Search logs", { exact: true }).fill("needle");
    await searchResponse;
    await app.page.waitForFunction(() => document.querySelector(".goal-log-line mark"));
    assert.ok(calls.some(c => c.q === "needle" && c.type === "stdout"));
    assert.equal(await panel.locator("details, summary").count(), 0);
    assert.equal(await panel.locator("script").count(), 0);
    assert.equal(await panel.locator("pre").evaluate(el => getComputedStyle(el).overflowY), "visible");
    assert.match(await panel.getByTestId("goal-log-status").textContent(), /401 matches/);
    await panel.getByRole("button", { name: "Older", exact: true }).click();
    await app.page.waitForFunction(() => document.querySelector('[data-testid="goal-log-status"]').textContent.includes("page 2"));
    await app.page.screenshot({ path: "/tmp/refine-toolbar-logs.png" });
    await app.page.goto(`${app.origin}/#/goals/${GOAL.id}`);
    const viewLogs = app.page.getByRole("button", { name: "View Logs", exact: true });
    await viewLogs.waitFor();
    await viewLogs.click();
    await app.page.getByTestId("toolbar-goal-log-panel").waitFor();
    assert.equal(await app.page.locator('[data-testid="goal-detail-modal"]').count(), 0);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});
