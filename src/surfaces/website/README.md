# Website assets

The homepage references `site.css` and `site.js` with content-derived `?v=`
identifiers, following the screenshot URL convention. This prevents cached
assets from an earlier release being used with new HTML.

After editing either asset, refresh the references and include `index.html` in
the same change:

```sh
node scripts/website-asset-versions.js --write
node scripts/website-asset-versions.js --check
```

The check exits unsuccessfully when references are missing or outdated. It also
runs in the asset regression suite:

```sh
node --test tests/website_asset_versions.test.js
```

The comparison browser suite uses Chromium via Playwright and the real
`refine website` server. Build `target/debug/refine` or set `REFINE_BIN` to an
existing executable. Ensure Playwright and its Chromium browser are installed;
a successful browser run must report zero skipped tests.

```sh
REFINE_BIN=/absolute/path/to/refine node --test tests/website_comparison_browser.test.js
```

Set `COMPARISON_SCREENSHOT_DIR` to save comparison-section screenshots at 1440,
1024, 390, and 320 pixels, including the benefits and guidance above the table.
