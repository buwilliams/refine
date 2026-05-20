// ---- Gaps: import -----------------------------------------------------------

const IMPORT_CHUNK_LINE_COUNT = 20;

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
    const draftsRoot = root.querySelector("#import-drafts");
    if (draftsRoot) {
      drawImportProgress(draftsRoot, {
        current: 0,
        total: 1,
        chunkSize: IMPORT_CHUNK_LINE_COUNT,
        lineCount: countImportLines(text),
        draftCount: 0,
      });
    }
    await withButtonBusy(btn, "Extracting…", async () => {
      try {
        const drafts = await extractImportDrafts(text, draftsRoot);
        drawImportDrafts(root, drafts, close);
      } catch (e) {
        if (draftsRoot) draftsRoot.innerHTML = "";
        toast(e.message, "error");
      }
    });
  });

  root.querySelector("#import-text").focus();
}

function countImportLines(text) {
  return text.split(/\r?\n/).map((line) => line.trim()).filter(Boolean).length;
}

function importTextChunks(text) {
  const lines = text.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
  if (lines.length <= IMPORT_CHUNK_LINE_COUNT) {
    return [{
      text: text.trim(),
      startLine: lines.length ? 1 : 0,
      endLine: lines.length,
      lineCount: lines.length,
    }];
  }
  const chunks = [];
  for (let i = 0; i < lines.length; i += IMPORT_CHUNK_LINE_COUNT) {
    const chunkLines = lines.slice(i, i + IMPORT_CHUNK_LINE_COUNT);
    chunks.push({
      text: chunkLines.join("\n"),
      startLine: i + 1,
      endLine: i + chunkLines.length,
      lineCount: chunkLines.length,
    });
  }
  return chunks;
}

async function extractImportDrafts(text, draftsRoot) {
  const chunks = importTextChunks(text);
  const lineCount = countImportLines(text);
  const drafts = [];
  if (draftsRoot) {
    drawImportProgress(draftsRoot, {
      current: 0,
      total: chunks.length,
      chunkSize: IMPORT_CHUNK_LINE_COUNT,
      lineCount,
      draftCount: 0,
    });
  }
  for (let i = 0; i < chunks.length; i += 1) {
    const chunk = chunks[i];
    if (draftsRoot) {
      drawImportProgress(draftsRoot, {
        current: i + 1,
        total: chunks.length,
        chunk,
        chunkSize: IMPORT_CHUNK_LINE_COUNT,
        lineCount,
        draftCount: drafts.length,
      });
    }
    let r = null;
    try {
      r = await api("POST", "/api/import/extract", { text: chunk.text });
    } catch (e) {
      const range = `lines ${chunk.startLine}-${chunk.endLine}`;
      throw new Error(
        `AI request ${i + 1} of ${chunks.length} failed (${range}): ${e.message}`,
      );
    }
    drafts.push(...(r.drafts || []));
    if (draftsRoot) {
      drawImportProgress(draftsRoot, {
        current: i + 1,
        total: chunks.length,
        chunk,
        chunkSize: IMPORT_CHUNK_LINE_COUNT,
        lineCount,
        draftCount: drafts.length,
        completed: i + 1 === chunks.length,
      });
    }
  }
  return drafts;
}

function drawImportProgress(root, state) {
  const chunked = state.total > 1;
  const chunkLabel = state.chunk
    ? ` lines ${state.chunk.startLine}-${state.chunk.endLine}`
    : "";
  const status = state.completed
    ? `Processed ${state.total} AI request${state.total === 1 ? "" : "s"}.`
    : state.current
      ? `Processing AI request ${state.current} of ${state.total}${chunkLabel}.`
      : chunked
        ? `Preparing ${state.total} AI requests of up to ${state.chunkSize} lines each.`
        : "Asking the selected AI provider to extract Gaps.";
  const detail = chunked
    ? `${state.lineCount} lines will be processed in chunks of ${state.chunkSize}; ${state.draftCount} draft${state.draftCount === 1 ? "" : "s"} extracted so far.`
    : "This may take up to a minute.";
  root.innerHTML = `
    <div class="loading-row">
      <span class="loading-spinner"></span>
      <span>${htmlEscape(status)}</span>
    </div>
    <p class="muted small" style="margin:8px 0 0">${htmlEscape(detail)}</p>
  `;
}

function drawImportDrafts(root, drafts, close, options = {}) {
  const drafts_root = root.querySelector("#import-drafts");
  if (!drafts.length) {
    drafts_root.innerHTML = `<p class="muted">No drafts extracted.</p>`;
    return;
  }
  const title = options.retry
    ? `Failed drafts (${drafts.length}) — correct &amp; retry`
    : `Extracted drafts (${drafts.length}) — review &amp; confirm`;
  drafts_root.innerHTML = `
    <h3 style="margin-top:0">${title}</h3>
    ${drafts.map((d, i) => `
      <div class="draft" data-idx="${i}">
        ${d.error ? `<p class="small" style="margin-top:0;color:#b42318">${htmlEscape(d.error)}</p>` : ""}
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
    const btn = actions.querySelector("#btn-persist");
    if (btn.disabled) return;
    const reporter = state.lastReporter || "";
    if (!reporter) return toast("Pick a reporter in the top-right selector", "error");
    const payload = $$(".draft", drafts_root).map((row) => ({
      name: row.querySelector(".d-name").value.trim(),
      actual: row.querySelector(".d-actual").value.trim(),
      target: row.querySelector(".d-target").value.trim(),
    }));
    await withButtonBusy(btn, "Saving…", async () => {
      try {
        let r = await api("POST", "/api/import/persist", { reporter, drafts: payload });
        r = await resolveBackgroundJobResponse(
          r,
          `Saving ${payload.length} gaps in the background`,
        );
        const failures = r.failures || [];
        const createdCount = r.count || 0;
        if (failures.length) {
          const failedDrafts = failures.map((failure) => {
            const original = payload[(failure.index || 1) - 1] || {};
            return {
              ...original,
              ...(failure.draft || {}),
              error: failure.error || "Could not save this Gap.",
            };
          });
          toast(
            `Created ${createdCount} gap${createdCount === 1 ? "" : "s"}; ${failures.length} need fixes`,
            "error",
          );
          drawImportDrafts(root, failedDrafts, close, { retry: true });
        } else {
          toast(`Created ${createdCount} gap(s)`, "info");
          // Stay on the underlying screen — same behavior as the New Gap
          // modal. `close(true)` only redirects when the user came in via
          // the `#/gaps/import` deep link.
          close(true);
        }
      } catch (e) { toast(e.message, "error"); }
    });
  });
}
