// Keep the homepage's CSS and JavaScript URLs tied to their content so caches
// cannot reuse assets from an earlier layout. Run with --write after editing.
const { createHash } = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");

function versionWebsiteAssets(root, { write = false } = {}) {
  const homepage = path.join(root, "src/surfaces/website/index.html");
  const original = fs.readFileSync(homepage, "utf8");
  let updated = original;
  for (const name of ["site.css", "site.js"]) {
    const asset = `/src/surfaces/website/assets/${name}`;
    const hash = createHash("sha256").update(fs.readFileSync(path.join(root, asset))).digest("hex").slice(0, 16);
    const reference = new RegExp(`(["'])${asset.replaceAll(".", "\\.")}(?:\\?[^"']*)?\\1`, "g");
    if (!updated.match(reference)) throw new Error(`Missing homepage reference to ${name}`);
    updated = updated.replace(reference, (_, quote) => `${quote}${asset}?v=${hash}${quote}`);
  }
  if (updated === original) return;
  if (!write) throw new Error("Website asset versions are stale. Run: node scripts/website-asset-versions.js --write");
  fs.writeFileSync(homepage, updated);
}

if (require.main === module) {
  try {
    const args = process.argv.slice(2);
    if (args.length > 1 || (args.length === 1 && !["--check", "--write"].includes(args[0]))) {
      throw new Error("Usage: node scripts/website-asset-versions.js [--check|--write]");
    }
    versionWebsiteAssets(path.join(__dirname, ".."), { write: args[0] === "--write" });
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}

module.exports = { versionWebsiteAssets };
