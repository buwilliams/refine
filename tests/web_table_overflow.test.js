const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const staticRoot = path.join(__dirname, "../src/surfaces/web/static");
const styles = fs.readFileSync(
  path.join(staticRoot, "css/common.css"),
  "utf8",
);

function sourceFiles(root) {
  return fs.readdirSync(root, { withFileTypes: true }).flatMap((entry) => {
    const target = path.join(root, entry.name);
    if (entry.isDirectory()) return sourceFiles(target);
    return /\.(?:html|js)$/.test(entry.name) ? [target] : [];
  });
}

test("every shared table cell wraps content that cannot break normally", () => {
  const cellRule = styles.match(/\.table th, \.table td \{([^}]*)\}/);

  assert.ok(cellRule, "shared table cell styles are missing");
  assert.match(cellRule[1], /overflow-wrap:\s*anywhere;/);
  assert.match(cellRule[1], /word-break:\s*normal;/);
});

test("every web table opts into the shared overflow-safe table styles", () => {
  const tables = sourceFiles(staticRoot).flatMap((file) => {
    const source = fs.readFileSync(file, "utf8");
    return [...source.matchAll(/<table\b[^>]*>/g)].map((match) => ({
      file,
      tag: match[0],
    }));
  });

  assert.ok(tables.length > 0, "no rendered web tables were found");
  for (const { file, tag } of tables) {
    assert.match(
      tag,
      /class=["'][^"']*\btable\b[^"']*["']/,
      `${path.relative(staticRoot, file)} does not use the shared table class: ${tag}`,
    );
  }
});
