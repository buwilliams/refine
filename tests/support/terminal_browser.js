const assert = require('node:assert/strict');
const { openApp, apiFixture, SKIP } = require('./web_app');

// Real bootstrap, controls, shared terminal lifecycle, and xterm. Only daemon
// traffic and provider output are fixtures; no live Refine process is launched.
async function openTerminalApp({ mac = false, platform, profile = 'agent', mockClipboard = true } = {}) {
  const requests = [], launches = [];
  const skill = { id: 'inspect', name: 'Inspect release', enabled: true,
    prompt: 'Inspect the release.', trigger_source: 'custom',
    parameters: [{ name: 'count', kind: 'number', required: true, default: 3 }] };
  const app = await openApp({
    fixture(pathname, request) {
      if (pathname === '/api/skills') return { revision: 1, items: [skill], manual_skill_ids: ['inspect'] };
      if (pathname === '/api/skills/inspect') return { revision: 1, item: skill, trigger: { source: 'custom', enabled: true } };
      if (pathname === '/api/skills/inspect/inputs') return { revision: 1, parameters: skill.parameters };
      if (pathname === '/api/terminal/session') {
        const body = request.postDataJSON();
        launches.push(body);
        return { id: `session-${launches.length}`, process_id: `process-${launches.length}`,
          profile: body.profile, provider: 'codex', cwd: '/tmp/app' };
      }
      if (/\/api\/terminal\/session-\d+\/status$/.test(pathname)) return { alive: true };
      return apiFixture(pathname);
    },
    onRequest(pathname, request) {
      if (pathname.endsWith('/input')) requests.push(request.postDataJSON().data);
    },
  });
  try {
    const page = app.page;
    page.setDefaultTimeout(5000);
    await page.addInitScript((platform) => {
      if (platform) Object.defineProperty(navigator, 'platform', { value: platform });
      window.EventSource = class extends EventTarget {
        constructor(url) { super(); this.url = url; }
        close() {}
        emit(type, payload) { this.dispatchEvent(new MessageEvent(type, { data: JSON.stringify(payload) })); }
      };
    }, platform || (mac ? 'MacIntel' : null));
    await page.goto(`${app.origin}/#/dashboard`);
    await page.locator('#dash').waitFor();
    const launchAgent = async () => {
      await page.locator('.toolbar-add-menu > summary').click();
      await page.locator('[data-add-toolbar-tab="agent"]').click();
      await page.locator('[data-testid="terminal-stop"]').waitFor();
    };
    if (profile === 'skill') {
      await page.locator('#nav-context-menu > summary').click();
      await page.locator('[data-manual-skill="inspect"]').click();
      const modal = page.locator('[data-testid="automation-modal"]');
      await modal.locator('[data-parameter-index="0"]').fill('5');
      await modal.locator('[data-save]').click();
      await modal.waitFor({ state: 'detached' });
      await page.locator('[data-testid="terminal-stop"]').waitFor();
      assert.equal(launches[0].skill_id, 'inspect');
      assert.deepEqual(launches[0].parameters, { count: 5 });
      assert.equal(launches[0].goal_id, undefined);
    } else {
      await launchAgent();
    }
    assert.equal(launches[0].profile, profile);
    assert.equal(launches[0].surface, 'toolbar');
    const first = await page.evaluate(() => chatState.activeTabId);
    await launchAgent();
    const second = await page.evaluate(() => chatState.activeTabId);
    await page.evaluate(async ({ first, second, mockClipboard }) => {
      window.clipboardTabs = { a: first, b: second };
      chatState.bodyHeight = 400;
      for (const [id, label] of [[first, 'agent-a'], [second, 'agent-b']]) {
        await activateToolbarTab(id);
        const terminal = terminalStateFor(id);
        terminal.eventSource.emit('terminal_output', { seq: 1, data: `\x1b[32mOUTPUT-${label}\x1b[0m\r\nsecond line` });
        await new Promise(resolve => terminal.term.write('', resolve));
      }
      await activateToolbarTab(first);
      window.originalTerm = terminalStateFor().term;
      window.realClipboard = navigator.clipboard;
      window.copyWrites = [];
      if (mockClipboard) Object.defineProperty(navigator, 'clipboard', {
        configurable: true, value: { writeText: async text => copyWrites.push(text) },
      });
    }, { first, second, mockClipboard });
    return { ...app, requests, launches, first, second };
  } catch (error) {
    await app.close();
    throw error;
  }
}

async function selectOutput(page) {
  await page.evaluate(() => {
    terminalStateFor().term.select(0, 0, 14);
    terminalStateFor().term.focus();
  });
  await page.waitForFunction(() => !document.querySelector('[data-terminal-copy]').disabled);
}

module.exports = { openTerminalApp, selectOutput, SKIP };
