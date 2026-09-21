const assert = require('node:assert/strict');
const test = require('node:test');
const {openApp, apiFixture, SKIP} = require('./support/web_app');
const defaults = require('../src/model/providers_defaults.json');

test('Runtime edits provider definitions, defaults and node inheritance while retaining failed drafts', {skip: SKIP}, async () => {
  let catalog = structuredClone(defaults), node = null;
  const requests = [];
  const selection = () => ({catalog, node_override: node, effective_provider: node || catalog.default_provider, selection_source: node ? 'node' : 'system'});
  const app = await openApp({fixture(path, request) {
    if (path === '/api/settings') {
      if (request.method() === 'PATCH') {
        const body = request.postDataJSON(); requests.push(body);
        if ('agent_cli' in body) node = body.agent_cli;
      }
      return {settings: {...apiFixture(path).settings, agent_cli: node || catalog.default_provider}, providers: selection()};
    }
    if (path === '/api/providers') {
      if (request.method() === 'PUT') { catalog = request.postDataJSON(); catalog.revision++; }
      return selection();
    }
    return apiFixture(path);
  }});
  try {
    const {page} = app;
    await page.goto(`${app.origin}/#/settings/runtime`);
    await page.locator('#provider-add').click();
    const dialog = page.locator('[data-testid="automation-modal"]');
    await dialog.locator('#provider-id').fill('MyAgent');
    await dialog.locator('#provider-name').fill('My Agent');
    await dialog.locator('#provider-executable').fill('/Case Sensitive/Agent');
    await dialog.locator('#provider-automated-args').fill('["--new", "two words"]');
    await dialog.locator('[data-save]').click();
    await dialog.waitFor({state: 'detached'});
    assert.equal(catalog.providers.at(-1).executable, '/Case Sensitive/Agent');
    assert.deepEqual(catalog.providers.at(-1).automated.context_args, ['{{context}}']);
    await page.locator('#provider-system-default').selectOption('MyAgent');
    await page.locator('#provider-save-default').click();
    await page.waitForFunction(() => document.querySelector('#s-cli option')?.textContent.includes('My Agent'));
    assert.equal(node, null);
    // The editable-field commit event is the same event emitted by its visible Save button.
    await page.locator('#s-cli').evaluate(el => { el.value = 'gemini'; el.dispatchEvent(new Event('settings-editable-commit', {bubbles: true})); });
    await page.waitForFunction(() => document.querySelector('#s-cli')?.value === 'gemini');
    await page.waitForTimeout(150);
    assert.equal(node, 'gemini');
    await page.locator('#s-cli').evaluate(el => { el.value = ''; el.dispatchEvent(new Event('settings-editable-commit', {bubbles: true})); });
    await page.waitForTimeout(150);
    assert.equal(node, null);
    await page.reload();
    await page.locator('[data-provider-edit="claude"]').click();
    await dialog.locator('#provider-executable').fill('/Updated/Claude');
    await dialog.locator('[data-save]').click();
    await dialog.waitFor({state: 'detached'});
    assert.equal(catalog.providers[0].executable, '/Updated/Claude');
    await page.locator('[data-provider-edit="MyAgent"]').click();
    await dialog.locator('#provider-name').fill('Retained draft');
    await page.route('**/api/providers', route => route.request().method() === 'PUT'
      ? route.fulfill({status: 409, contentType: 'application/json', body: JSON.stringify({error: 'Catalog changed'})}) : route.fallback());
    await dialog.locator('[data-save]').click();
    await page.waitForFunction(() => document.querySelector('[data-automation-error]')?.textContent.length > 0);
    assert.equal(await dialog.locator('#provider-name').inputValue(), 'Retained draft');
    await page.evaluate(() => refreshSettingsTab('runtime', {force: true}));
    assert.equal(await dialog.locator('#provider-name').inputValue(), 'Retained draft');
    assert.deepEqual(requests.filter(r => 'agent_cli' in r), [{agent_cli:'gemini'}, {agent_cli:null}]);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test('Runtime retains invalid provider edits and a pending system default across refresh', {skip: SKIP}, async () => {
  let catalog = structuredClone(defaults), saves = 0;
  const selection = () => ({catalog, node_override: null, effective_provider: catalog.default_provider, selection_source: 'system'});
  const app = await openApp({fixture(path, request) {
    if (path === '/api/settings') return {settings: {...apiFixture(path).settings, agent_cli: catalog.default_provider}, providers: selection()};
    if (path === '/api/providers') {
      if (request.method() === 'PUT') { catalog = request.postDataJSON(); catalog.revision++; saves++; }
      return selection();
    }
    return apiFixture(path);
  }});
  try {
    const {page} = app;
    await page.goto(`${app.origin}/#/settings/runtime`);
    await page.locator('#provider-system-default').selectOption('gemini');
    await page.evaluate(() => refreshSettingsTab('runtime', {force: true}));
    assert.equal(await page.locator('#provider-system-default').inputValue(), 'gemini');
    assert.equal(catalog.default_provider, 'claude');
    await page.locator('#provider-save-default').click();
    await page.waitForFunction(() => document.querySelector('#s-cli option')?.textContent.includes('Gemini'));
    assert.equal(catalog.default_provider, 'gemini');
    await page.locator('[data-provider-edit="claude"]').click();
    const dialog = page.locator('[data-testid="automation-modal"]');
    await dialog.locator('#provider-executable').fill('/Edited Agent');
    await dialog.locator('#provider-automated-args').fill('["unfinished"');
    await dialog.locator('[data-save]').click();
    await page.waitForFunction(() => document.querySelector('[data-automation-error]')?.textContent.length > 0);
    assert.equal(saves, 1, 'invalid JSON must not reach the server');
    await page.evaluate(() => refreshSettingsTab('runtime', {force: true}));
    assert.equal(await dialog.locator('#provider-executable').inputValue(), '/Edited Agent');
    assert.equal(await dialog.locator('#provider-automated-args').inputValue(), '["unfinished"');
    await dialog.locator('#provider-automated-args').fill('["--prompt", "{{context}}"]');
    await dialog.locator('[data-save]').click();
    await dialog.waitFor({state: 'detached'});
    assert.deepEqual(catalog.providers[0].automated.args, ['--prompt', '{{context}}']);
    await page.locator('[data-provider-edit="claude"]').click();
    await dialog.locator('[data-delete]').click();
    await dialog.waitFor({state: 'detached'});
    assert.equal(catalog.providers.some(p => p.id === 'claude'), false);
    await page.reload();
    assert.equal(await page.locator('[data-provider-edit="claude"]').count(), 0);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});
