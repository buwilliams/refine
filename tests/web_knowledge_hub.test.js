const assert = require("node:assert/strict");
const test = require("node:test");
const { openApp, apiFixture, GOAL, SKIP } = require("./support/web_app");

function hubFixture() {
  const sites = new Map([["reports", { revision: "site-1", item: { id: "reports", name: "Reports", description: "History", publication: { collections: [] } } }]]);
  const collections = new Map();
  const records = new Map();
  const writes = [];
  let revision = 1;
  const fixture = (pathname, request) => {
    const method = request.method();
    const body = request.postDataJSON();
    if (method !== "GET" && pathname.startsWith("/api/hub")) writes.push({ pathname, method, body });
    if (pathname === "/api/hub/sites") return { sites: [...sites.values()] };
    if (!pathname.startsWith("/api/hub/sites/")) return apiFixture(pathname);
    const [, , , site, ...rest] = pathname.split("/").filter(Boolean);
    if (!rest.length) {
      if (method === "PUT") sites.set(site, { revision: `site-${++revision}`, item: { id: site, ...body, publication: null } });
      return sites.get(site);
    }
    if (rest[0] === "assets") return { revision: "assets-1", item: {} };
    if (rest[0] === "collections" && rest.length === 1) return { collections: [...collections.values()] };
    const collection = rest[1];
    if (rest.length === 2) {
      const result = { revision: `collection-${++revision}`, item: { id: collection, indexes: body.indexes } };
      collections.set(collection, result);
      return result;
    }
    if (rest[2] === "records") {
      const result = { revision: `record-${++revision}`, item: { id: rest[3], data: body.data } };
      records.set(rest[3], result);
      return result;
    }
    if (rest[2] === "query") {
      const offset = Number(body.cursor || 0), limit = body.limit || 100;
      const rows = [...records.values()];
      return { rows: rows.slice(offset, offset + limit), total: rows.length, next_cursor: offset + limit < rows.length ? String(offset + limit) : null };
    }
    return {};
  };
  return { fixture, sites, records, writes };
}

test("Knowledge Hub uses the existing origin and manages sites and paginated records", { skip: SKIP }, async () => {
  const data = hubFixture();
  const app = await openApp({ fixture: data.fixture });
  const page = app.page;
  try {
    await page.context().route("**/hub/sites/reports/", route => route.fulfill({ contentType: "text/html", body: "<title>Hosted report</title>Report" }));
    await page.goto(`${app.origin}/#/goals`);
    await page.getByTestId("context-menu-toggle").click();
    const popupReady = page.waitForEvent("popup");
    await page.locator('[data-hub-open="reports"]').click();
    const popup = await popupReady;
    await popup.waitForURL(`${app.origin}/hub/sites/reports/`);
    assert.equal(await popup.title(), "Hosted report");
    await popup.close();
    await page.getByTestId("context-menu-toggle").click();
    await page.locator('[data-hub-add]').click();
    await page.locator('[data-name]').fill("Usage report");
    await page.locator('[data-description]').fill("Durable statistics");
    await page.locator('[data-submit]').click();
    const binary = Buffer.from([0, 137, 255, 10, 42]);
    await page.locator('[data-files]').setInputFiles({ name: "sample.bin", mimeType: "application/octet-stream", buffer: binary });
    await page.waitForFunction(() => !document.querySelector('[data-testid="hub-modal"]')?._busy);
    await page.locator('[data-new-collection]').waitFor();
    const upload = data.writes.find(write => write.method === "PUT" && write.pathname.endsWith("/assets"));
    assert.ok(upload);
    assert.equal(upload.body.bytes, undefined);
    assert.deepEqual(Buffer.from(upload.body.bytes_base64, "base64"), binary);
    await page.locator('[data-new-collection]').click();
    await page.getByTestId('hub-modal').locator('[data-id]').fill("events");
    await page.locator('[data-write]').click();
    for (const value of [1, 2]) {
      await page.locator('[data-new]').click();
      await page.locator('[data-data]').fill(JSON.stringify({ value, timestamp: "2026-09-11T00:00:00Z" }));
      await page.locator('[data-write]').click();
      await page.locator('[data-summary]').filter({ hasText: `${value} results` }).waitFor();
    }
    await page.locator('[data-query]').fill('{"version":1,"limit":1}');
    await page.locator('[data-run]').click();
    await page.locator('[data-next]:not([disabled])').waitFor();
    await page.locator('[data-next]').click();
    await page.locator('[data-prev]:not([disabled])').waitFor();
    assert.equal(await page.locator('[data-next]').isDisabled(), true);
    await page.locator('[data-prev]').click();
    await page.locator('[data-next]:not([disabled])').waitFor();
    assert.equal(data.records.size, 2);
    assert.equal(data.sites.size, 2);
    assert.equal(data.writes.filter(write => write.pathname.includes("/records/")).length, 2);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test("Workflow controls send the Goal revision and explicit override through the shared API", { skip: SKIP }, async () => {
  let submitted;
  const app = await openApp({ fixture: (pathname, request) => {
    if (pathname === "/api/hub/sites") return { sites: [] };
    if (pathname === "/api/workflow/goals/GOAL1") return { ...GOAL, workflow_revision: 7, status: "failed" };
    if (pathname === "/api/workflow/goals/GOAL1/move") { submitted = request.postDataJSON(); return { accepted: true }; }
    return apiFixture(pathname);
  } });
  try {
    await app.page.goto(`${app.origin}/#/goals`);
    await app.page.evaluate(() => openWorkflowControl({ id: "GOAL1" }));
    await app.page.locator('[data-to]').selectOption("done");
    await app.page.locator('[data-reason]').fill("Explicit status-only completion");
    await app.page.locator('[data-force]').check();
    await app.page.locator('[data-apply]').click();
    await app.page.locator('[data-testid="hub-modal"]').waitFor({ state: "detached" });
    assert.equal(submitted.to, "done");
    assert.equal(submitted.force, true);
    assert.equal(submitted.expected_revision, 7);
    assert.equal(submitted.actor, "Reporter");
    assert.ok(submitted.request_id);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});
