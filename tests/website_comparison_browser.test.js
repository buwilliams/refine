const assert = require("node:assert/strict");
const path = require("node:path");
const test = require("node:test");
const { openApp, SKIP } = require("./support/web_app");

async function routeWebsite(page, origin) {
  await page.route(`${origin}/`, route => route.fulfill({
    path: path.join(__dirname, "../src/surfaces/website/index.html"),
  }));
  await page.route(`${origin}/src/surfaces/**`, route => route.fulfill({
    path: path.join(__dirname, "..", new URL(route.request().url()).pathname),
  }));
}

async function openWebsite() {
  const app = await openApp();
  await routeWebsite(app.page, app.origin);
  return app;
}

async function assertFocusedProductVisible(region, name, width) {
  assert.equal(await region.evaluate((el, name) => {
    const link = document.activeElement;
    const bounds = el.getBoundingClientRect();
    const pinned = el.querySelector("thead th").getBoundingClientRect();
    const rect = link.closest("th").getBoundingClientRect();
    return link.textContent === name && rect.left >= pinned.right && rect.right <= bounds.right;
  }, name), true, `focused ${name} column is fully visible beside pinned labels at ${width}`);
}

test("website comparison presents benefits, labeled capabilities, and qualified sources", { skip: SKIP }, async () => {
  const app = await openWebsite();
  try {
    const { page } = app;
    await page.goto(app.origin);
    const comparison = page.locator("#compare");
    assert.deepEqual(await comparison.getByRole("columnheader").allTextContents(),
      ["Capability", "Refine", "Direct agents", "Blitzy", "n8n"]);
    assert.deepEqual(await comparison.locator(".compare-benefits h3").allTextContents(), [
      "Use your agents and repositories", "Coordinate delivery across machines", "Keep ownership and recover work",
    ]);
    assert.match(await comparison.locator(".compare-intro").innerText(), /category; features vary by agent/);
    assert.equal(await comparison.locator("caption").count(), 1);
    const rows = comparison.locator("tbody tr");
    assert.equal(await rows.count(), 6);
    for (const row of await rows.all()) {
      assert.equal(await row.locator('th[scope="row"]').count(), 1);
      assert.equal(await row.getByRole("cell").count(), 4);
      for (const cell of await row.getByRole("cell").all()) {
        const label = (await cell.locator(".compare-status > span:last-child").innerText()).trim();
        assert.match(label, /^(Built in|Custom setup|Varies|Not documented|Not offered)$/);
        assert.equal(await cell.locator('.compare-mark[aria-hidden="true"]').count(), 1);
        assert.ok((await comparison.locator(".compare-legend").innerText()).includes(label));
      }
    }
    assert.equal(await comparison.getByRole("link", { name: "Blitzy", exact: true }).getAttribute("href"), "https://blitzy.com/");
    assert.equal(await comparison.getByRole("link", { name: "n8n", exact: true }).getAttribute("href"), "https://n8n.io/");
    assert.equal(await comparison.getByRole("link", { name: "n8n", exact: true }).evaluate(el => getComputedStyle(el).textTransform), "none");
    assert.equal(await comparison.getByRole("link", { name: "Get started with Refine" }).getAttribute("href"), "#start");
    assert.equal(await page.locator("#start").count(), 1);
    assert.equal(await comparison.locator(".compare-sources time").getAttribute("datetime"), "2026-09-15");
    const sources = await comparison.locator(".compare-sources a").evaluateAll(links => links.map(a => a.getAttribute("href")));
    assert.ok(sources.includes("/docs"));
    for (const host of ["blitzy.com", "docs.n8n.io", "github.com"]) {
      assert.ok(sources.some(href => new URL(href, app.origin).hostname === host), host);
    }
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test("comparison disclosures open and close with keyboard and touch", { skip: SKIP }, async () => {
  const app = await openWebsite();
  try {
    const { page } = app;
    await page.goto(app.origin);
    for (const details of await page.locator(".compare-details details").all()) {
      assert.equal(await details.evaluate(el => el.open), false);
      const summary = details.locator("summary");
      await summary.focus();
      await page.keyboard.press("Shift+Tab");
      await page.keyboard.press("Tab");
      assert.equal(await summary.evaluate(el => el === document.activeElement), true);
      assert.equal(await summary.evaluate(el => getComputedStyle(el).outlineStyle), "solid");
      await page.keyboard.press("Enter");
      assert.equal(await details.evaluate(el => el.open), true);
      assert.equal(await details.locator("p, dl").first().isVisible(), true);
      await page.keyboard.press("Space");
      assert.equal(await details.evaluate(el => el.open), false);
    }
    const context = await page.context().browser().newContext({ hasTouch: true, viewport: { width: 390, height: 900 } });
    try {
      const touchPage = await context.newPage();
      await routeWebsite(touchPage, app.origin);
      await touchPage.goto(app.origin);
      const details = touchPage.locator(".compare-details details").first();
      await details.locator("summary").tap();
      assert.equal(await details.evaluate(el => el.open), true);
      await details.locator("summary").tap();
      assert.equal(await details.evaluate(el => el.open), false);
    } finally { await context.close(); }
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test("comparison keeps Refine initially visible and other products reachable at four widths", { skip: SKIP }, async () => {
  const app = await openWebsite();
  try {
    const { page } = app;
    for (const width of [1440, 1024, 390, 320]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(app.origin);
      const region = page.getByRole("region", { name: "Comparison of agent workflows" });
      await region.scrollIntoViewIfNeeded();
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `page overflow at ${width}`);
      assert.equal(await region.evaluate(el => el.scrollLeft), 0);
      assert.equal(await region.evaluate(el => {
        const bounds = el.getBoundingClientRect();
        return [...el.querySelectorAll("tr")].every(row => {
          const label = row.children[0].getBoundingClientRect();
          const refine = row.children[1].getBoundingClientRect();
          return label.left >= bounds.left && label.right <= refine.left && refine.right <= bounds.right;
        });
      }), true, `capabilities and Refine are initially visible together at ${width}`);
      const metrics = await region.evaluate(el => ({ width: el.clientWidth, scrollWidth: el.scrollWidth }));
      const cells = await region.locator("th, td").evaluateAll(elements => elements.map(el => ({
        text: el.textContent, width: el.clientWidth, scrollWidth: el.scrollWidth,
      })));
      for (const cell of cells) assert.ok(cell.scrollWidth <= cell.width + 1, `clipped cell at ${width}: ${cell.text}`);
      const colors = await region.locator("tbody tr").first().locator("td").evaluateAll(elements => elements.map(el => getComputedStyle(el).backgroundColor));
      assert.notEqual(colors[0], colors[1], "Refine column is highlighted");
      await region.focus();
      await page.keyboard.press("Shift+Tab");
      await page.keyboard.press("Tab");
      assert.equal(await region.evaluate(el => el === document.activeElement), true);
      assert.equal(await region.evaluate(el => getComputedStyle(el).outlineStyle), "solid");
      if (metrics.scrollWidth > metrics.width) {
        assert.equal(await page.locator("#compare-scroll-hint").isVisible(), true);
        await page.keyboard.press("ArrowRight");
        await page.waitForFunction(() => document.querySelector(".compare-scroll").scrollLeft > 0);
        // Let Chromium finish its native key-scroll animation before positioning columns.
        await region.evaluate(el => new Promise(resolve => {
          let previous = el.scrollLeft;
          let stableFrames = 0;
          function settled() {
            stableFrames = el.scrollLeft === previous ? stableFrames + 1 : 0;
            previous = el.scrollLeft;
            if (stableFrames >= 6) resolve();
            else requestAnimationFrame(settled);
          }
          requestAnimationFrame(settled);
        }));
        for (const column of [3, 4, 5]) {
          await region.evaluate((el, column) => {
            const cell = el.querySelector(`thead th:nth-child(${column})`);
            el.scrollLeft += cell.getBoundingClientRect().right - el.getBoundingClientRect().right + 2;
          }, column);
          assert.equal(await region.evaluate((el, column) => {
            const cell = el.querySelector(`thead th:nth-child(${column})`).getBoundingClientRect();
            const label = el.querySelector("thead th").getBoundingClientRect();
            const bounds = el.getBoundingClientRect();
            return Math.abs(label.left - bounds.left) <= 1 && cell.left >= label.right && cell.right <= bounds.right;
          }, column), true, `product ${column} is fully visible beside pinned labels at ${width}`);
        }
      }
      await page.keyboard.press("Tab");
      assert.equal(await page.evaluate(() => document.activeElement.textContent), "Blitzy");
      await assertFocusedProductVisible(region, "Blitzy", width);
      assert.equal(await page.evaluate(() => getComputedStyle(document.activeElement).outlineStyle), "solid");
      await page.keyboard.press("Tab");
      assert.equal(await page.evaluate(() => document.activeElement.textContent), "n8n");
      await assertFocusedProductVisible(region, "n8n", width);
      await page.keyboard.press("Shift+Tab");
      await assertFocusedProductVisible(region, "Blitzy", width);
      for (const details of await page.locator(".compare-details details").all()) await details.locator("summary").click();
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `expanded details overflow at ${width}`);
    }
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});
