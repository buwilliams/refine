const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const staticRoot = path.join(__dirname, "../src/surfaces/web/static");

test("new goal modal provides a large responsive prompt editor", () => {
  const source = fs.readFileSync(
    path.join(staticRoot, "js/features/goals-new.js"),
    "utf8",
  );
  const styles = fs.readFileSync(
    path.join(staticRoot, "css/modals.css"),
    "utf8",
  );

  assert.match(source, /class="modal new-goal-modal"/);
  assert.doesNotMatch(source, /data-testid="new-goal-modal"[^>]*style=/);
  assert.match(
    styles,
    /\.new-goal-modal\s*\{[^}]*width:\s*60vw;[^}]*max-width:\s*60vw;/s,
  );
  assert.match(
    styles,
    /\.new-goal-modal textarea\[name="prompt"\]\s*\{[^}]*height:\s*clamp\(180px, 32vh, 320px\);/s,
  );
  assert.match(
    styles,
    /@media \(max-width: 760px\)[\s\S]*?\.new-goal-modal\s*\{[^}]*width:\s*calc\(100% - 24px\);[^}]*max-width:\s*calc\(100% - 24px\);/,
  );
});
