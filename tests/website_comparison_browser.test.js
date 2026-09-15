const assert = require("node:assert/strict");
const path = require("node:path");
const test = require("node:test");
const { openApp, SKIP } = require("./support/web_app");

async function openWebsite() {
  const app = await openApp();
  await app.page.route(`${app.origin}/`, route => route.fulfill({
    path: path.join(__dirname, "../src/surfaces/website/index.html"),
  }));
  await app.page.route(`${app.origin}/src/surfaces/**`, route => route.fulfill({
    path: path.join(__dirname, "..", new URL(route.request().url()).pathname),
  }));
  return app;
}

test("website comparison identifies products and exposes labeled, sourced descriptions", { skip: SKIP }, async () => {
  const app = await openWebsite();
  try {
    const { page } = app;
    await page.goto(app.origin);
    const comparison = page.locator("#compare");
    assert.deepEqual(await comparison.getByRole("columnheader").allTextContents(),
      ["Capability", "Direct agents", "Blitzy", "n8n", "Refine"]);
    assert.doesNotMatch(await comparison.innerText(), /agent loops|conductor|rest are missing/i);
    const rows = comparison.locator("tbody tr");
    assert.equal(await rows.count(), 8);
    for (const row of await rows.all()) {
      assert.equal(await row.locator('th[scope="row"]').count(), 1);
      assert.equal(await row.getByRole("cell").count(), 4);
      for (const cell of await row.getByRole("cell").all()) {
        assert.ok((await cell.innerText()).trim().length > 0);
      }
    }
    assert.equal(await comparison.getByRole("link", { name: "Blitzy", exact: true }).getAttribute("href"), "https://blitzy.com/");
    assert.equal(await comparison.getByRole("link", { name: "n8n", exact: true }).getAttribute("href"), "https://n8n.io/");
    assert.equal(await comparison.getByRole("link", { name: "n8n", exact: true }).evaluate(el => getComputedStyle(el).textTransform), "none");
    assert.ok(await comparison.locator(".compare-sources time").getAttribute("datetime"));
    const sources = await comparison.locator(".compare-sources a").evaluateAll(links => links.map(a => a.getAttribute("href")));
    assert.ok(sources.includes("/docs"));
    for (const host of ["blitzy.com", "n8n.io", "docs.n8n.io", "github.com"]) {
      assert.ok(sources.some(href => new URL(href, app.origin).hostname === host), host);
    }
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test("comparison remains readable and keyboard scrollable at desktop and narrow widths", { skip: SKIP }, async () => {
  const app = await openWebsite();
  try {
    const { page } = app;
    for (const width of [1440, 1024, 390, 320]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(app.origin);
      const region = page.getByRole("region", { name: "Comparison of agent workflows" });
      await region.scrollIntoViewIfNeeded();
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `page overflow at ${width}`);
      const metrics = await region.evaluate(el => ({ width: el.clientWidth, scrollWidth: el.scrollWidth }));
      const cells = await region.locator("th, td").evaluateAll(elements => elements.map(el => ({
        text: el.textContent, width: el.clientWidth, scrollWidth: el.scrollWidth,
      })));
      for (const cell of cells) assert.ok(cell.scrollWidth <= cell.width + 1, `clipped cell at ${width}: ${cell.text}`);
      const colors = await region.locator("tbody tr").first().locator("td").evaluateAll(elements => elements.map(el => getComputedStyle(el).backgroundColor));
      assert.notEqual(colors[3], colors[0], "Refine column is highlighted");
      await region.focus();
      await page.keyboard.press("Shift+Tab");
      await page.keyboard.press("Tab");
      assert.equal(await region.evaluate(el => el === document.activeElement), true);
      assert.equal(await region.evaluate(el => getComputedStyle(el).outlineStyle), "solid");
      if (metrics.scrollWidth > metrics.width) {
        await page.keyboard.press("ArrowRight");
        await page.waitForFunction(() => document.querySelector(".compare-scroll").scrollLeft > 0);
        await region.evaluate(el => { el.scrollLeft = el.scrollWidth; });
        assert.equal(await region.evaluate(el => {
          const cell = el.querySelector("thead th:last-child").getBoundingClientRect();
          const bounds = el.getBoundingClientRect();
          return cell.left >= bounds.left && cell.right <= bounds.right;
        }), true, "last column can be brought fully into view");
      }
      await page.keyboard.press("Tab");
      assert.equal(await page.evaluate(() => document.activeElement.textContent), "Blitzy");
      assert.equal(await page.evaluate(() => getComputedStyle(document.activeElement).outlineStyle), "solid");
      await page.keyboard.press("Tab");
      assert.equal(await page.evaluate(() => document.activeElement.textContent), "n8n");
    }
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});
