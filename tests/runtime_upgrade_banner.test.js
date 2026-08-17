import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = fs.readFileSync(
  new URL("../src/surfaces/web/static/js/features/settings_runtime.js", import.meta.url),
  "utf8",
);
const context = {
  htmlEscape(value) {
    return String(value)
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;")
      .replaceAll("'", "&#39;");
  },
};
vm.createContext(context);
vm.runInContext(source, context);

function message(upgrade) {
  return context
    .renderRuntimeUpgradeBanner(upgrade)
    .replace(/<[^>]+>/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

test("published release banner preserves version upgrade language", () => {
  assert.match(
    message({
      runtime_kind: "published_release",
      current_version: "4.2.0",
      latest_version: "4.3.0",
      upgrade_available: true,
    }),
    /^Upgrade available 4\.3\.0/,
  );
  assert.equal(
    message({
      runtime_kind: "published_release",
      current_version: "4.2.0",
      latest_version: "4.2.0",
      upgrade_available: false,
    }),
    "Running latest 4.2.0",
  );
});

test("source banner reports only trusted HEAD and upstream relationships", () => {
  assert.equal(
    message({
      runtime_kind: "source",
      source: {
        running_from_head: true,
        relationship: "current",
        upstream: { remote: "origin", branch: "main" },
      },
    }),
    "Running from HEAD · Up to date with origin/main",
  );
  assert.equal(
    message({
      runtime_kind: "source",
      source: {
        running_from_head: true,
        relationship: "behind",
        upstream: { remote: "origin", branch: "main" },
      },
    }),
    "Running from HEAD · Behind origin/main",
  );
  assert.equal(
    message({
      runtime_kind: "source",
      source: {
        running_from_head: true,
        relationship: "ahead",
        upstream: { remote: "origin", branch: "main" },
      },
    }),
    "Running from HEAD · Ahead of origin/main",
  );
  assert.equal(
    message({
      runtime_kind: "source",
      source: {
        running_from_head: true,
        relationship: "diverged",
        upstream: { remote: "origin", branch: "main" },
      },
    }),
    "Running from HEAD · Diverged from origin/main",
  );
  assert.equal(
    message({
      runtime_kind: "source",
      source: {
        running_from_head: null,
        relationship: "unknown",
        unknown_reason: "upstream_cache_stale",
        upstream: { freshness: "stale" },
      },
    }),
    "Source runtime status unknown · cached upstream evidence is stale",
  );
});
