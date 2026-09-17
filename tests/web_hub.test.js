const { selectMain } = require("./support/web_app");
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
    if (pathname === "/api/skills") return {revision:1,items:[{id:"maintain",name:"Maintain reports",enabled:true,scope:{}}],manual_skill_ids:["maintain"]};
    if (pathname === "/api/hub/sites") return { sites: [...sites.values()] };
    if (!pathname.startsWith("/api/hub/sites/")) return apiFixture(pathname);
    const [, , , site, ...rest] = pathname.split("/").filter(Boolean);
    if (!rest.length) {
      if (method === "PUT") sites.set(site, { revision: `site-${++revision}`, item: { id: site, ...body, publication: null } });
      return sites.get(site);
    }
    if (rest[0] === "assets") return method === "POST" ? {bytes_base64: Buffer.from("Report").toString("base64")} : { revision: "assets-1", item: {"index.html": {bytes: 6}} };
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
  return { fixture, sites, collections, records, writes };
}

test("Hub uses the existing origin and manages sites and paginated records", { skip: SKIP }, async () => {
  const data = hubFixture();
  const app = await openApp({ fixture: data.fixture });
  const page = app.page;
  try {
    await page.context().route("**/hub/sites/reports/", route => route.fulfill({ contentType: "text/html", body: "<title>Hosted report</title>Report" }));
    await page.goto(`${app.origin}/#/goals`);
    await selectMain(page, "settings");
    await page.getByTestId("create-menu-toggle").click();
    const popupReady = page.waitForEvent("popup");
    await page.locator('[data-hub-open="reports"]').click();
    const popup = await popupReady;
    await popup.waitForURL(`${app.origin}/hub/sites/reports/`);
    assert.equal(await popup.title(), "Hosted report");
    await popup.close();
    await selectMain(page, "settings");
    await page.getByTestId("create-menu-toggle").click();
    await page.locator('[data-hub-add]').click();
    assert.equal(await page.getByTestId('hub-modal').locator('[data-name]').getAttribute('id'), 'hub-site-name');
    assert.equal(await page.locator('.modal-body button[data-submit]').count(), 0);
    assert.equal(await page.locator('.modal-actions [data-submit]').count(), 1);
    await page.getByLabel('Name', {exact: true}).fill("Usage report");
    await page.locator('[data-description]').fill("Durable statistics");
    await page.locator('[data-hub-skill]').selectOption("maintain");
    await page.locator('[data-submit]').click();
    await page.locator('[data-run-hub-skill]').waitFor();
    assert.equal(data.writes.find(write => write.method === 'PUT' && write.body.name === 'Usage report').body.skill_id, 'maintain');
    assert.equal(await page.locator('[data-run-hub-skill]').isEnabled(), true);
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


test("Hub modal rows open from cells and keyboards; Controls uses the shared menu layout", { skip: SKIP }, async () => {
  const data = hubFixture();
  data.collections.set("events", {revision: "collection-1", item: {id: "events", indexes: {}}});
  data.records.set("one", {revision: "record-1", item: {id: "one", data: {value: 42}}});
  const app = await openApp({fixture: data.fixture});
  const page = app.page;
  try {
    await page.goto(`${app.origin}/#/goals`);
    await selectMain(page, "settings");
    await page.getByTestId("create-menu-toggle").click();
    await page.locator('[data-hub-add]').waitFor();
    const menu = await page.locator('#nav-hubs button').evaluateAll(buttons => buttons.map(button => ({
      classes: button.className, icons: button.querySelectorAll('svg.rail-icon').length,
      width: button.getBoundingClientRect().width, border: getComputedStyle(button).borderTopWidth,
      textOffset: button.querySelector('span').getBoundingClientRect().left - button.getBoundingClientRect().left
    })));
    assert.equal(menu.length, 2);
    assert.equal(await page.locator("[data-hub-manage]").count(), 0);
    for (const item of menu) {
      assert.match(item.classes, /nav-control-item nav-management-item/);
      assert.equal(item.icons, 1);
      assert.equal(item.border, "0px");
      assert.equal(item.width, menu[0].width);
      assert.equal(item.textOffset, menu[0].textOffset);
    }
    await selectMain(page, "settings");
    await page.getByTestId("create-menu-toggle").click();
    await page.goto(`${app.origin}/#/settings/hubs`);
    await page.locator('[data-hub-new-site]').waitFor();
    const tabs = await page.locator('.settings-tab').allTextContents();
    assert.equal(tabs[tabs.indexOf('Prompts') + 1], 'Hubs');
    await page.locator('[data-hub-new-site]').click();
    await page.locator('[data-hub-skill]').waitFor();
    assert.equal(await page.locator('.form-row label[for="hub-site-name"]').count(), 1);
    assert.equal(await page.locator('.form-row label[for="hub-site-description"]').count(), 1);
    await page.locator('[data-close]').click();
    assert.equal(await page.locator('[data-testid="hub-modal"]').count(), 0);
    let row = page.locator('tr[data-hub-site="reports"]');
    assert.equal(await row.locator('button').count(), 0);
    await row.locator('td').last().click();
    row = page.locator('tr[data-asset="index.html"]');
    assert.equal(await row.locator('button').count(), 0);
    await row.focus();
    await page.keyboard.press('Enter');
    await page.waitForFunction(() => document.querySelector('[data-content]')?.value === 'Report');
    assert.equal(await page.locator('[data-content]').inputValue(), "Report");
    await page.locator('[data-close]').click();
    await page.evaluate(() => openHubSite('reports'));
    row = page.locator('tr[data-collection="events"]');
    assert.equal(await row.locator('button').count(), 0);
    await row.focus(); await page.keyboard.press('Space');
    row = page.locator('tr[data-record="0"]');
    assert.equal(await row.locator('button').count(), 0);
    await row.locator('td').last().click();
    assert.deepEqual(JSON.parse(await page.locator('[data-data]').inputValue()), {value: 42});
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test("Skill history opens runs from row cells and keyboard activation", {skip: SKIP}, async () => {
  const run = {id: "run-one", event: {name: "Inspect report"}, state: "succeeded", created_at: "2026-09-11", results: {}};
  const app = await openApp({fixture: pathname => {
    if (pathname === '/api/event-invocations') return {items: [run], total: 1};
    if (pathname === '/api/event-invocations/run-one') return run;
    if (pathname === '/api/hub/sites') return {sites: []};
    return apiFixture(pathname);
  }});
  try {
    await app.page.goto(`${app.origin}/#/goals`);
    for (const key of [null, 'Enter', 'Space']) {
      await app.page.evaluate(() => openEventHistory());
      const row = app.page.locator('tr[data-open-run="run-one"]');
      assert.equal(await row.locator('button').count(), 0);
      if (key) { await row.focus(); await app.page.keyboard.press(key); }
      else await row.locator('td').last().click();
      await app.page.locator('.modal-title').filter({hasText: 'Inspect report'}).waitFor();
      await app.page.locator('[data-close]').click();
    }
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test("Refine Hub opens documentation instead of the site editor", { skip: SKIP }, async () => {
  const data = hubFixture();
  data.sites.set("refine", {revision:"bundled",item:{id:"refine",name:"Refine Hub",builtin:true,read_only:true,publication:{version:"4.3.0"}}});
  const app = await openApp({fixture:data.fixture});
  try {
    await app.page.context().route("**/hub/sites/refine/", route => route.fulfill({contentType:"text/html",body:"<h1>Refine Hub</h1>"}));
    await app.page.goto(`${app.origin}/#/settings/hubs`);
    const row = app.page.locator('[data-hub-site="refine"]');
    await row.waitFor();
    assert.match(await row.innerText(),/Built-in · Read-only/);
    const opened = app.page.waitForEvent("popup");
    await row.focus();
    await app.page.keyboard.press("Enter");
    const popup = await opened;
    await popup.waitForURL(`${app.origin}/hub/sites/refine/`);
    assert.equal(await app.page.getByTestId("hub-modal").count(),0);
    assert.equal(await app.page.locator('#guide-panel').count(),0);
    assert.equal(data.writes.length,0);
    await popup.close();
  } finally { await app.close(); }
});

test('Refine Hub offers its protected Skill and passes the selected Hub into a manual run', {skip:SKIP}, async () => {
  const data = hubFixture();
  const skill = {id:'update-refine-hub',name:'Update Refine Hub',prompt:'Maintain product documentation',enabled:true,scope:{},parameters:[],removable:false};
  data.sites.set('refine', {revision:'bundled',item:{id:'refine',name:'Refine Hub',builtin:true,skill_id:skill.id,publication:{version:'4.3.1'}}});
  const app = await openApp({fixture:(path, request) => {
    if (path === '/api/skills') return {revision:1,items:[skill],manual_skill_ids:[skill.id]};
    if (path === `/api/skills/${skill.id}`) return {revision:1,item:skill};
    if (path === `/api/skills/${skill.id}/inputs`) return {parameters:[{name:'request',kind:'text',required:true}]};
    return data.fixture(path,request);
  }});
  try {
    await app.page.goto(`${app.origin}/#/settings/hubs`);
    await app.page.evaluate(() => {createToolbarTab = async (mode, options) => {window.testHubLaunch = {mode, options};};});
    await app.page.locator('[data-hub-run="update-refine-hub"]').click();
    await app.page.locator('#event-parameter-0').fill('Explain the new Hub workflow');
    await app.page.locator('[data-save]').click();
    await app.page.waitForFunction(() => window.testHubLaunch);
    const launch = await app.page.evaluate(() => window.testHubLaunch);
    assert.deepEqual(launch.options.skillLaunch, {id:skill.id,parameters:{request:'Explain the new Hub workflow'},hubId:'refine'});
    assert.equal(launch.mode, 'skill');
    assert.equal(data.writes.length, 0);
    await app.page.evaluate(skill => openSkillEditor(skill), skill);
    assert.equal(await app.page.locator('[data-delete]').isHidden(), true);
  } finally {await app.close();}
});
