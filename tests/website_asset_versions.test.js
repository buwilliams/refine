const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { versionWebsiteAssets } = require("../scripts/website-asset-versions");

test("committed website asset references match the CSS and JavaScript content", () => {
  versionWebsiteAssets(path.join(__dirname, ".."));
});

test("asset edits require refreshed URLs and the updater preserves other homepage content", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "refine-website-assets-"));
  try {
    const site = path.join(root, "src/surfaces/website");
    fs.cpSync(path.join(__dirname, "../src/surfaces/website"), site, { recursive: true });
    const homepage = path.join(site, "index.html");
    for (const name of ["site.css", "site.js"]) {
      const before = fs.readFileSync(homepage, "utf8");
      fs.appendFileSync(path.join(site, "assets", name), "\n/* changed asset */\n");
      assert.throws(() => versionWebsiteAssets(root), /asset versions are stale/);
      assert.equal(fs.readFileSync(homepage, "utf8"), before, "check never modifies the homepage");
      versionWebsiteAssets(root, { write: true });
      versionWebsiteAssets(root);
      const after = fs.readFileSync(homepage, "utf8");
      assert.notEqual(after, before);
      const removeVersion = html => html.replace(new RegExp(`(${name.replace(".", "\\.")}\\?v=)[a-f0-9]+`, "g"), "$1HASH");
      assert.equal(removeVersion(after), removeVersion(before), "only the changed asset URL is updated");
      versionWebsiteAssets(root, { write: true });
      assert.equal(fs.readFileSync(homepage, "utf8"), after, "updates are repeatable");
    }
  } finally { fs.rmSync(root, { recursive: true, force: true }); }
});

test("missing asset references fail without partially updating the homepage", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "refine-website-assets-"));
  try {
    const site = path.join(root, "src/surfaces/website");
    fs.mkdirSync(path.join(site, "assets"), { recursive: true });
    fs.writeFileSync(path.join(site, "assets/site.css"), "body { color: navy; }");
    fs.writeFileSync(path.join(site, "assets/site.js"), "console.log('website');");
    const homepage = path.join(site, "index.html");
    for (const present of ["site.css", "site.js"]) {
      const missing = present === "site.css" ? "site.js" : "site.css";
      const original = `<link href="/src/surfaces/website/assets/${present}?v=outdated">`;
      fs.writeFileSync(homepage, original);
      for (const write of [false, true]) {
        assert.throws(() => versionWebsiteAssets(root, { write }), error => {
          assert.equal(error.message, `Missing homepage reference to ${missing}`);
          return true;
        });
        assert.equal(fs.readFileSync(homepage, "utf8"), original,
          "a missing reference must leave the entire homepage untouched");
      }
    }
  } finally { fs.rmSync(root, { recursive: true, force: true }); }
});
