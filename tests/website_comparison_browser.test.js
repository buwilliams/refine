const assert = require("node:assert/strict");
const path = require("node:path");
const test = require("node:test");
const fs = require("node:fs");
const { createHash } = require("node:crypto");
const { openWebsite, SKIP } = require("./support/website");

async function assertFocusedProductVisible(region, name, width) {
  assert.equal(await region.evaluate((el, name) => {
    const link = document.activeElement;
    const bounds = el.getBoundingClientRect();
    const pinned = el.querySelector("thead th").getBoundingClientRect();
    const rect = link.closest("th").getBoundingClientRect();
    return link.textContent === name && rect.left >= pinned.right && rect.right <= bounds.right;
  }, name), true, `focused ${name} column is fully visible beside pinned labels at ${width}`);
}

test("website server delivers the versioned CSS and JavaScript referenced by the homepage", { skip: SKIP }, async () => {
  const app = await openWebsite();
  try {
    const responses = [];
    app.page.on("response", response => {
      if (/\/site\.(css|js)(\?|$)/.test(response.url())) responses.push(response);
    });
    await app.page.goto(app.origin);
    assert.equal(responses.length, 2);
    for (const response of responses) {
      const url = new URL(response.url());
      const local = fs.readFileSync(path.join(__dirname, "..", url.pathname));
      assert.equal(response.status(), 200);
      assert.match(response.headers()["content-type"], url.pathname.endsWith("css") ? /text\/css/ : /javascript/);
      assert.equal(url.searchParams.get("v"), createHash("sha256").update(local).digest("hex").slice(0, 16));
      assert.deepEqual(await response.body(), local, "versioned requests resolve to the matching asset bytes");
    }
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test("comparison benefits and table guidance stay arranged and unclipped at four widths", { skip: SKIP }, async () => {
  const app = await openWebsite();
  try {
    for (const width of [1440, 1024, 390, 320]) {
      const { page } = app;
      await page.setViewportSize({ width, height: 900 });
      await page.goto(app.origin);
      const layout = await page.locator("#compare").evaluate(section => {
        const rect = el => {
          const r = el.getBoundingClientRect();
          return { left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height };
        };
        const box = selector => rect(section.querySelector(selector));
        return {
          cards: [...section.querySelectorAll(".compare-benefits article")].map(card => ({
            card: rect(card), icon: rect(card.querySelector(".compare-benefit-icon")),
            heading: rect(card.querySelector("h3")), description: rect(card.querySelector("p")),
          })),
          benefits: box(".compare-benefits"), intro: box(".compare-intro"),
          legend: box(".compare-legend"), hint: box(".compare-scroll-hint"), table: box(".compare-scroll"),
          legendItems: [...section.querySelectorAll(".compare-legend li")].map(rect),
          content: [...section.querySelectorAll(".compare-benefits *, .compare-intro, .compare-legend, .compare-legend li, .compare-scroll-hint")]
            .map(el => ({ ...rect(el), text: el.textContent, clipped: el.scrollWidth > el.clientWidth + 1 })),
        };
      });
      const near = (a, b, label) => assert.ok(Math.abs(a - b) <= 1, `${label} at ${width}`);
      assert.equal(layout.cards.length, 3);
      for (const [i, { card, icon, heading, description }] of layout.cards.entries()) {
        assert.ok(icon.width >= 30 && icon.height >= 30, `styled benefit icon at ${width}`);
        assert.ok(icon.top >= card.top && icon.bottom <= card.bottom);
        assert.ok(heading.bottom <= description.top && description.bottom <= card.bottom);
        near(heading.left, description.left, "heading and description align");
        if (width > 680) {
          near(card.top, layout.cards[0].card.top, "desktop benefit columns align");
          near(card.width, layout.cards[0].card.width, "desktop columns share width");
          near(heading.top, layout.cards[0].heading.top, "desktop headings align");
          near(description.top, layout.cards[0].description.top, "desktop descriptions align");
          near(icon.left, heading.left, "desktop icon aligns with text");
          assert.ok(heading.top >= icon.bottom && heading.top - icon.bottom <= 20);
          if (i) assert.ok(card.left > layout.cards[i - 1].card.right);
        } else {
          near(icon.left, card.left, "mobile icon aligns with card");
          assert.ok(icon.right < heading.left && heading.left - icon.right <= 16);
          assert.ok(Math.abs(icon.top - heading.top) <= 4, "mobile icon stays beside heading");
          if (i) assert.ok(card.top - layout.cards[i - 1].card.bottom >= 12, "mobile benefits are separated");
        }
      }
      const flow = [layout.benefits, layout.intro, layout.legend, layout.hint, layout.table];
      for (let i = 1; i < flow.length; i++) {
        near(flow[i].left, flow[0].left, "benefits and table guidance align");
        assert.ok(flow[i].top >= flow[i - 1].bottom && flow[i].top - flow[i - 1].bottom <= 48,
          `guidance stays grouped above the table at ${width}`);
      }
      assert.equal(layout.legendItems.length, 5);
      const legendRows = new Set(layout.legendItems.map(item => Math.round(item.top)));
      assert.ok(width > 680 ? legendRows.size === 1 : legendRows.size > 1, `legend wrapping at ${width}`);
      for (const item of layout.legendItems) {
        assert.ok(item.left >= layout.legend.left && item.right <= layout.legend.right);
        assert.ok(item.top >= layout.legend.top && item.bottom <= layout.legend.bottom);
      }
      for (const item of layout.content) {
        assert.ok(item.left >= 0 && item.right <= width && !item.clipped, `clipped content at ${width}: ${item.text}`);
      }
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
      if (process.env.COMPARISON_SCREENSHOT_DIR) {
        fs.mkdirSync(process.env.COMPARISON_SCREENSHOT_DIR, { recursive: true });
        const clip = await page.locator("#compare").boundingBox();
        await page.screenshot({ fullPage: true, clip,
          path: path.join(process.env.COMPARISON_SCREENSHOT_DIR, `comparison-${width}.png`) });
      }
    }
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

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
