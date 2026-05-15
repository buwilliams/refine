// ---- Gaps: import -----------------------------------------------------------

async function renderGapImport() {
  // Import is a modal layered over the gaps list, mirroring New Gap.
  await renderGapsList();
  openImportModal();
}

let _importModalOpen = false;

function openImportModal() {
  if (_importModalOpen) return;
  const reporter = state.lastReporter || "";
  if (!reporter) {
    toast("Pick a reporter in the top-right selector first", "error");
    return;
  }
  _importModalOpen = true;

  const root = document.createElement("div");
  root.className = "modal-backdrop";
  root.innerHTML = `
    <div class="modal" role="dialog" aria-modal="true"
         aria-labelledby="import-title" style="max-width:680px">
      <div class="modal-title" id="import-title">Import gaps</div>
      <div class="modal-body" style="max-height:70vh;overflow:auto">
        <p class="muted small">Paste free-form text (meeting transcript, bug report,
        feedback dump). refine extracts a draft list — review and edit before saving.</p>
        <div class="muted small" style="margin-bottom:8px">
          Submitting as <strong class="js-reporter-name">${htmlEscape(reporter)}</strong>
          — applies to all extracted gaps. Change in the top-right reporter selector.
        </div>
        <div class="form-row">
          <label>Source text</label>
          <textarea id="import-text" rows="8" placeholder="Paste here…"></textarea>
        </div>
        <div id="import-drafts" class="import-drafts" style="margin-top:14px"></div>
      </div>
      <div class="modal-actions">
        <button class="secondary" data-cancel>Cancel</button>
        <button id="btn-extract" data-ok>Extract drafts</button>
      </div>
    </div>
  `;
  document.body.appendChild(root);

  let closed = false;
  function close(navigateAway) {
    if (closed) return;
    closed = true;
    _importModalOpen = false;
    document.removeEventListener("keydown", onKey, true);
    root.remove();
    if (navigateAway && location.hash.startsWith("#/gaps/import")) {
      location.hash = "#/gaps";
    }
  }
  function onKey(e) {
    if (e.key === "Escape") {
      e.preventDefault();
      close(true);
    }
    // Enter inside textareas always inserts a newline; no global Enter
    // submit, since this modal has two distinct submit steps.
  }
  document.addEventListener("keydown", onKey, true);
  root.addEventListener("click", (e) => {
    if (e.target === root) close(true);
  });
  root.querySelector("[data-cancel]").addEventListener("click", () => close(true));

  root.querySelector("#btn-extract").addEventListener("click", async () => {
    const btn = root.querySelector("#btn-extract");
    if (btn.disabled) return;
    const text = root.querySelector("#import-text").value.trim();
    if (!text) return toast("Paste some text first", "error");
    // Show an explicit loading indicator in the drafts area — the LLM
    // call typically takes 20-90s and the busy button alone isn't enough
    // signal that something's happening.
    const draftsRoot = root.querySelector("#import-drafts");
    if (draftsRoot) {
      draftsRoot.innerHTML = `
        <div class="loading-row">
          <span class="loading-spinner"></span>
          <span>Loading… asking the selected AI provider to extract Gaps from your text. This may take up to a minute.</span>
        </div>`;
    }
    await withButtonBusy(btn, "Extracting…", async () => {
      try {
        const r = await api("POST", "/api/import/extract", { text });
        drawImportDrafts(root, r.drafts || [], close);
      } catch (e) {
        if (draftsRoot) draftsRoot.innerHTML = "";
        toast(e.message, "error");
      }
    });
  });

  root.querySelector("#import-text").focus();
}

function drawImportDrafts(root, drafts, close) {
  const drafts_root = root.querySelector("#import-drafts");
  if (!drafts.length) {
    drafts_root.innerHTML = `<p class="muted">No drafts extracted.</p>`;
    return;
  }
  drafts_root.innerHTML = `
    <h3 style="margin-top:0">Extracted drafts (${drafts.length}) — review &amp; confirm</h3>
    ${drafts.map((d, i) => `
      <div class="draft" data-idx="${i}">
        <input type="text" class="d-name" value="${htmlEscape(d.name)}" placeholder="Name">
        <div class="form-row" style="margin-top:6px">
          <label class="small muted">Actual</label>
          <textarea class="d-actual" rows="2">${htmlEscape(d.actual)}</textarea>
        </div>
        <div class="form-row">
          <label class="small muted">Target</label>
          <textarea class="d-target" rows="3">${htmlEscape(d.target)}</textarea>
        </div>
      </div>`).join("")}
  `;
  // Swap the primary action from "Extract drafts" to "Save N gap(s)".
  const actions = root.querySelector(".modal-actions");
  actions.innerHTML = `
    <button class="secondary" data-cancel>Cancel</button>
    <button id="btn-persist">Save ${drafts.length} gap${drafts.length === 1 ? "" : "s"}</button>
  `;
  actions.querySelector("[data-cancel]").addEventListener("click", () => close(true));
  actions.querySelector("#btn-persist").addEventListener("click", async () => {
    const reporter = state.lastReporter || "";
    if (!reporter) return toast("Pick a reporter in the top-right selector", "error");
    const payload = $$(".draft", drafts_root).map((row) => ({
      name: row.querySelector(".d-name").value.trim(),
      actual: row.querySelector(".d-actual").value.trim(),
      target: row.querySelector(".d-target").value.trim(),
    }));
    try {
      const r = await api("POST", "/api/import/persist", { reporter, drafts: payload });
      toast(`Created ${r.count} gap(s)`, "info");
      // Stay on the underlying screen — same behavior as the New Gap
      // modal. `close(true)` only redirects when the user came in via
      // the `#/gaps/import` deep link.
      close(true);
    } catch (e) { toast(e.message, "error"); }
  });
}
