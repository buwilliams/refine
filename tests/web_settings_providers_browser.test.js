const assert = require('node:assert/strict');
const test = require('node:test');
const fs = require('node:fs');
const path = require('node:path');
const {openApp, apiFixture, SKIP} = require('./support/web_app');
const defaults = require('../src/model/providers_defaults.json');
const evidence = path.join(__dirname, '../target/provider-checks/screenshots');
fs.mkdirSync(evidence, {recursive: true});

function fixture() {
  let catalog = structuredClone(defaults), node = null;
  return {
    get catalog() { return catalog; }, get node() { return node; },
    change(fn) { fn(catalog); catalog.revision++; },
    response(url, request) {
      const selection = () => ({catalog, node_override: node, effective_provider: node || catalog.default_provider, selection_source: node ? 'node' : 'system'});
      if (url === '/api/settings') {
        if (request.method() === 'PATCH') { const body = request.postDataJSON(); if ('agent_cli' in body) node = body.agent_cli; }
        return {settings: {...apiFixture(url).settings, agent_cli: node || catalog.default_provider}, providers: selection()};
      }
      if (url === '/api/providers') {
        if (request.method() === 'PUT') { catalog = request.postDataJSON(); catalog.revision++; }
        return selection();
      }
      return apiFixture(url);
    },
  };
}
async function addArgument(dialog, group, value) {
  await dialog.locator(`[data-add-argument="${group}"]`).click();
  await dialog.locator(`[data-args="${group}"] textarea`).last().fill(value);
}
async function save(dialog) { await dialog.locator('[data-save]').click(); await dialog.waitFor({state:'detached'}); }
async function nodeSelection(page, value) {
  const field = page.locator('[data-settings-editable-field]').filter({has: page.locator('#s-cli')});
  await field.locator('[data-settings-editable-toggle]').click();
  await page.locator('#s-cli').selectOption(value);
  await field.locator('[data-settings-editable-toggle]').click();
  await page.waitForTimeout(200);
}

test('visible provider controls preserve exact argv, variants, default, override, reload and keyboard behavior', {skip:SKIP}, async () => {
  const data = fixture();
  const app = await openApp({fixture: data.response});
  try {
    const {page} = app;
    await page.goto(`${app.origin}/#/settings/runtime`);
    await page.screenshot({animations:'disabled',path:path.join(evidence, 'runtime-desktop.png'), fullPage:true});
    await page.locator('#provider-add').click();
    let dialog = page.locator('[data-testid="automation-modal"]');
    await dialog.locator('#provider-name').fill('Local careful');
    assert.equal(await dialog.locator('#provider-id').inputValue(), 'local-careful');
    await dialog.locator('#provider-executable').fill('/Case Sensitive/Agent');
    for (const value of ['--mode', 'two words', '', '  spaces  ', '\nline one\nline two\n']) await addArgument(dialog, 'automated-args', value);
    await dialog.locator('[data-args="automated-args"] .provider-argument').nth(1).getByRole('button', {name:'Move argument down'}).click();
    await dialog.locator('[data-args="automated-args"] .provider-argument').nth(2).getByRole('button', {name:'Move argument up'}).click();
    await dialog.locator('#provider-name').scrollIntoViewIfNeeded();
    await page.screenshot({animations:'disabled',path:path.join(evidence, 'editor-desktop.png')});
    await page.reload();
    await page.locator('#provider-restore').click();
    dialog = page.locator('[data-testid="automation-modal"]');
    assert.equal(await dialog.locator('#provider-name').inputValue(), 'Local careful');
    assert.equal(await dialog.locator('[data-args="automated-args"] textarea').last().inputValue(), '\nline one\nline two\n');
    await save(dialog);
    assert.deepEqual(data.catalog.providers.at(-1).automated.args, ['--mode','two words','','  spaces  ','\nline one\nline two\n']);
    await page.locator('#provider-add').click();
    await dialog.locator('#provider-name').fill('Local fast');
    await dialog.locator('#provider-executable').fill('/Case Sensitive/Agent');
    await addArgument(dialog,'automated-args','--fast');
    await save(dialog);
    assert.equal(data.catalog.providers.at(-1).executable, data.catalog.providers.at(-2).executable);
    await page.locator('#provider-system-default').selectOption('local-careful');
    await page.reload();
    assert.equal(await page.locator('#provider-system-default').inputValue(), 'local-careful');
    await page.locator('#provider-save-default').click();
    await page.waitForFunction(() => document.querySelector('#provider-catalog-status')?.textContent.includes('saved'));
    assert.equal(data.catalog.default_provider, 'local-careful');
    await nodeSelection(page, 'local-fast'); assert.equal(data.node,'local-fast');
    await nodeSelection(page, ''); assert.equal(data.node,null);
    await page.locator('[data-testid="provider-catalog"]').scrollIntoViewIfNeeded();
    await page.screenshot({animations:'disabled',path:path.join(evidence,'providers-desktop.png')});
    data.change(c => c.providers.find(p=>p.id==='local-fast').automated.args.push('\r\nline\r\n'));
    await page.evaluate(() => { invalidateScreenDataCache(); return refreshSettingsTab('runtime',{force:true}); });
    await page.locator('[data-provider-edit="local-fast"]').click();
    await dialog.locator('#provider-name').fill('Local quick');
    await save(dialog); assert.equal(data.catalog.providers.at(-1).name,'Local quick');
    assert.equal(data.catalog.providers.at(-1).automated.args.at(-1),'\r\nline\r\n');
    await page.locator('[data-provider-edit="local-fast"]').click();
    await page.keyboard.press('Tab');
    assert.ok(await dialog.locator(':focus').count());
    await page.keyboard.press('Escape');
    await dialog.waitFor({state:'detached'});
    assert.equal(await page.locator('[data-provider-edit="local-fast"]').evaluate(el => el === document.activeElement), true);
    await page.setViewportSize({width:390,height:844});
    await page.waitForTimeout(150);
    assert.ok(await page.locator('#main').evaluate(el => el.clientHeight > 500));

    await page.locator('[data-testid="provider-catalog"]').scrollIntoViewIfNeeded();
    await page.screenshot({animations:'disabled',path:path.join(evidence,'providers-narrow.png')});
    await page.locator('[data-provider-edit="local-careful"]').click();
    await page.screenshot({animations:'disabled',path:path.join(evidence,'editor-narrow.png')});
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    assert.ok(await dialog.evaluate(el => el.scrollWidth <= el.clientWidth));
    await page.keyboard.press('Escape');
    await page.goto(`${app.origin}/#/settings/reporters`);
    await page.screenshot({animations:'disabled',path:path.join(evidence,'reporters-narrow.png')});
    await page.setViewportSize({width:1280,height:720});
    await page.screenshot({animations:'disabled',path:path.join(evidence,'reporters-desktop.png')});
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test('invalid input and genuine stale revision preserve drafts and reconcile unrelated and overlapping edits', {skip:SKIP}, async () => {
  const data = fixture(); const app = await openApp({fixture:data.response});
  try {
    const {page} = app;
    // Realistic API revision rejection layered over the fixture persistence.
    await page.route('**/api/providers', async route => {
      if (route.request().method() === 'PUT' && route.request().postDataJSON().revision !== data.catalog.revision) {
        return route.fulfill({status:409,contentType:'application/json',body:JSON.stringify({error:{message:'AI provider catalog changed; reload and reapply your edits'}})});
      }
      return route.fallback();
    });
    await page.goto(`${app.origin}/#/settings/runtime`);
    await page.locator('[data-provider-edit="claude"]').click();
    const dialog = page.locator('[data-testid="automation-modal"]');
    await dialog.locator('#provider-executable').fill('/Updated/Claude');
    await addArgument(dialog,'automated-args','{{unknown}}');
    await dialog.locator('[data-save]').click();
    assert.match(await dialog.locator('[data-automation-error]').innerText(), /supported templates/);
    await page.screenshot({animations:'disabled',path:path.join(evidence,'validation-desktop.png')});
    await page.setViewportSize({width:390,height:844});
    await page.screenshot({animations:'disabled',path:path.join(evidence,'validation-narrow.png')});
    await page.setViewportSize({width:1280,height:720});
    await page.evaluate(() => refreshSettingsTab('runtime',{force:true}));
    assert.equal(await dialog.locator('#provider-executable').inputValue(),'/Updated/Claude');
    await dialog.locator('[data-args="automated-args"] textarea').last().fill('--edited');
    data.change(catalog => { catalog.providers.find(p => p.id === 'gemini').name='Remote Gemini'; catalog.providers[0].name='Remote Claude'; });
    await dialog.locator('[data-save]').click();
    await dialog.locator('[data-provider-rebase]').click();
    await dialog.locator('[data-apply-rebase]').click();
    assert.equal(await dialog.locator('#provider-name').inputValue(),'Remote Claude');
    assert.equal(await dialog.locator('#provider-executable').inputValue(),'/Updated/Claude');
    await save(dialog);
    assert.equal(data.catalog.providers.find(p => p.id==='gemini').name,'Remote Gemini');
    await page.locator('[data-provider-edit="claude"]').click();
    await dialog.locator('#provider-name').fill('My Claude');
    data.change(catalog => { catalog.providers[0].name='Other Claude'; });
    await dialog.locator('[data-save]').click();
    await dialog.locator('[data-provider-rebase]').click();
    await dialog.locator('[data-provider-conflicts]').scrollIntoViewIfNeeded();
    await page.screenshot({animations:'disabled',path:path.join(evidence,'conflict-desktop.png')});
    await page.setViewportSize({width:390,height:844});
    await dialog.locator('[data-provider-conflicts]').scrollIntoViewIfNeeded();
    await page.screenshot({animations:'disabled',path:path.join(evidence,'conflict-narrow.png')});
    await page.setViewportSize({width:1280,height:720});
    await dialog.locator('#provider-conflict-0').selectOption('mine');
    await dialog.locator('[data-apply-rebase]').click();
    await save(dialog);
    assert.equal(data.catalog.providers[0].name,'My Claude');
    assert.deepEqual(app.pageErrors,[]);
  } finally { await app.close(); }
});

test('late responses and changed context cannot close or overwrite provider drafts', {skip:SKIP}, async () => {
  const data=fixture(); const app=await openApp({fixture:data.response});
  try {
    const {page}=app; await page.goto(`${app.origin}/#/settings/runtime`);
    await page.locator('[data-provider-edit="claude"]').click();
    const dialog=page.locator('[data-testid="automation-modal"]');
    await dialog.locator('#provider-name').fill('Retained in original context');
    let release; const gate=new Promise(resolve => release=resolve);
    await page.route('**/api/providers', async route => { if(route.request().method()==='PUT') { await gate; return route.fulfill({json:{}}); } return route.fallback(); });
    await dialog.locator('[data-save]').click();
    assert.equal(await dialog.locator('#provider-name').isDisabled(),true);
    assert.equal(await dialog.locator('[data-close]').isEnabled(),true);
    await page.evaluate(() => { nodeContextGeneration++; nodeContextTargetRoot='/different-project'; });
    release(); await page.waitForTimeout(150);
    assert.equal(await dialog.locator('#provider-name').inputValue(),'Retained in original context');
    assert.equal(await dialog.count(),1);
    assert.deepEqual(app.pageErrors,[]);
  } finally { await app.close(); }
});

test('advanced references and stdin, failed saves, and pending node selection survive refresh and reload', {skip:SKIP}, async () => {
  const data=fixture(); const app=await openApp({fixture:data.response});
  try {
    const {page}=app; await page.goto(`${app.origin}/#/settings/runtime`);
    const nodeField=page.locator('[data-settings-editable-field]').filter({has:page.locator('#s-cli')});
    await nodeField.locator('[data-settings-editable-toggle]').click();
    await page.locator('#s-cli').selectOption('codex');
    await page.reload();
    assert.equal(await page.locator('#s-cli').inputValue(),'codex');
    assert.equal(await page.locator('#s-cli').isVisible(),true);
    assert.equal(data.node,null);
    await nodeField.locator('[data-settings-editable-toggle]').click();
    await page.waitForTimeout(200); assert.equal(data.node,'codex');
    await page.locator('#provider-add').click();
    const dialog=page.locator('[data-testid="automation-modal"]');
    await dialog.locator('#provider-name').fill('Stdin variant');
    await dialog.locator('#provider-executable').fill('/opt/agent');
    await dialog.locator('#provider-automated-transport').selectOption('native_stdin');
    assert.equal(await dialog.locator('[data-args="automated-context_args"] textarea').count(),0);
    await dialog.getByText('Credentials and output',{exact:true}).click();
    await dialog.locator('[data-add-credential]').click();
    await dialog.locator('[data-credential-target]').fill('OPENAI_API_KEY');
    await dialog.locator('[data-credential-source]').fill('LOCAL_AGENT_TOKEN');
    let failing=true;
    await page.route('**/api/providers', route => failing && route.request().method()==='PUT'
      ? route.fulfill({status:503,json:{error:{message:'Temporary storage failure'}}}) : route.fallback());
    await dialog.locator('[data-save]').click();
    await page.waitForFunction(()=>document.querySelector('[data-automation-error]')?.textContent.length > 0);
    assert.match(await dialog.locator('[data-automation-error]').innerText(), /Temporary|storage|503/);
    assert.equal(await dialog.locator('#provider-name').inputValue(),'Stdin variant');
    failing=false; await save(dialog);
    assert.deepEqual(data.catalog.providers.at(-1).credentials,{OPENAI_API_KEY:'LOCAL_AGENT_TOKEN'});
    assert.equal(data.catalog.providers.at(-1).automated.stdin,'{{context}}');
    assert.deepEqual(app.pageErrors,[]);
  } finally {await app.close();}
});

test('default conflicts require review and reference conflicts keep the editor without a revision rebase', {skip:SKIP}, async () => {
  const data=fixture();const app=await openApp({fixture:data.response});
  try {
    const {page}=app;await page.goto(`${app.origin}/#/settings/runtime`);
    await page.route('**/api/providers', route => {
      if(route.request().method()==='PUT') {
        const body=route.request().postDataJSON();
        if(body.revision!==data.catalog.revision) return route.fulfill({status:409,json:{error:{message:'AI provider catalog changed; reload and reapply your edits'}}});
        if(!body.providers.some(p=>p.id==='gemini')) return route.fulfill({status:409,json:{error:{message:'AI provider gemini is selected by node other; change or clear that node selection before deleting it'}}});
      }
      return route.fallback();
    });
    await page.locator('#provider-system-default').selectOption('codex');
    data.change(c => { c.default_provider='gemini'; c.providers[0].name='Remote name'; });
    await page.locator('#provider-save-default').click();
    await page.locator('#provider-default-rebase').click();
    await page.waitForFunction(()=>document.querySelector('#provider-catalog-error')?.textContent.includes('Latest default'));
    assert.equal(await page.locator('#provider-system-default').inputValue(),'codex');
    await page.locator('#provider-save-default').click();
    await page.waitForFunction(()=>document.querySelector('#provider-catalog-status')?.textContent.includes('saved'));
    assert.equal(data.catalog.default_provider,'codex');assert.equal(data.catalog.providers[0].name,'Remote name');
    await page.locator('[data-provider-edit="gemini"]').click();
    const dialog=page.locator('[data-testid="automation-modal"]');
    await dialog.locator('#provider-name').fill('Retained name');
    await dialog.locator('[data-delete]').click();
    await page.waitForFunction(()=>document.querySelector('[data-automation-error]')?.textContent.includes('selected by node'));
    assert.equal(await dialog.locator('[data-provider-rebase]').isVisible(),false);
    assert.equal(await dialog.locator('#provider-name').inputValue(),'Retained name');
    assert.deepEqual(app.pageErrors,[]);
  } finally {await app.close();}
});


test('a remotely deleted pending selection stays visible until the user chooses a replacement', {skip:SKIP}, async () => {
  const data=fixture();const app=await openApp({fixture:data.response});
  try {
    const {page}=app;await page.goto(`${app.origin}/#/settings/runtime`);
    await page.locator('#provider-system-default').selectOption('codex');
    const field=page.locator('[data-settings-editable-field]').filter({has:page.locator('#s-cli')});
    await field.locator('[data-settings-editable-toggle]').click();
    await page.locator('#s-cli').selectOption('codex');
    data.change(c=>{c.providers=c.providers.filter(p=>p.id!=='codex');});
    await page.reload();
    for(const selector of ['#provider-system-default','#s-cli']) {
      assert.equal(await page.locator(selector).inputValue(),'codex');
      assert.match(await page.locator(`${selector} option:checked`).innerText(),/Unavailable provider/);
    }
    assert.equal(data.node,null);
    await page.locator('#s-cli').selectOption('');
    await field.locator('[data-settings-editable-toggle]').click();
    await page.waitForTimeout(150);
    assert.equal(data.node,null);
    assert.deepEqual(app.pageErrors,[]);
  } finally {await app.close();}
});
