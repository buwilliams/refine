// ---- Gaps: import -----------------------------------------------------------

const IMPORT_CHUNK_LINE_COUNT = 20;
const IMPORT_CSV_REQUIRED_FIELDS = [
  "actual (text)",
  "target (text)",
  "reporter (text)",
  "priority (low, medium, high)",
];
const IMPORT_MODES = {
  ai: {
    label: "AI Import",
    action: "Extract drafts",
  },
  csv: {
    label: "CSV Import",
    action: "Parse CSV",
  },
  upload: {
    label: "CSV Upload",
    action: "Parse upload",
  },
};

async function renderGapImport() {
  // Import is a modal layered over the gaps list, mirroring New Gap.
  await renderGapsList();
  openImportModal();
}

let _importModalOpen = false;

function openImportModal() {
  if (_importModalOpen) return;
  _importModalOpen = true;

  const reporter = state.lastReporter || "";
  const root = document.createElement("div");
  root.className = "modal-backdrop";
  root.innerHTML = `
    <div class="modal import-modal" role="dialog" aria-modal="true"
         aria-labelledby="import-title" style="max-width:760px">
      <div class="modal-title" id="import-title">Import gaps</div>
      <div class="modal-body" style="max-height:72vh;overflow:auto">
        <nav class="settings-tabs" id="import-tabs" role="tablist">
          ${Object.entries(IMPORT_MODES).map(([mode, meta]) => `
            <button type="button" class="settings-tab ${mode === "ai" ? "active" : ""}"
                    data-import-mode="${mode}" role="tab"
                    aria-selected="${mode === "ai" ? "true" : "false"}">
              ${htmlEscape(meta.label)}
            </button>`).join("")}
        </nav>
        <div class="card settings-tab-card import-tab-card">
          <section class="settings-pane import-panel active" data-import-panel="ai">
            <p class="muted small">Paste free-form text (meeting transcript, bug report,
            feedback dump). refine extracts a draft list — review and edit before saving.</p>
            <div class="muted small" style="margin-bottom:8px">
              Default reporter:
              <strong class="js-reporter-name">${htmlEscape(reporter || "none selected")}</strong>.
              Each draft can be edited before saving.
            </div>
            <div class="form-row">
              <label>Source text</label>
              <textarea id="import-text" rows="8" placeholder="Paste here…"></textarea>
            </div>
          </section>
          <section class="settings-pane import-panel" data-import-panel="csv">
            <div class="form-row">
              <label>CSV text
                <span class="muted small">— required fields: ${IMPORT_CSV_REQUIRED_FIELDS.map(htmlEscape).join(", ")}</span>
              </label>
              <textarea id="import-csv-text" rows="8" placeholder="actual,target,reporter,priority&#10;Current behavior,Desired behavior,Alice,medium"></textarea>
            </div>
          </section>
          <section class="settings-pane import-panel" data-import-panel="upload">
            <p class="muted small">
              Upload a CSV with headers:
              ${IMPORT_CSV_REQUIRED_FIELDS.map(htmlEscape).join(", ")}.
            </p>
            <div class="form-row">
              <label>CSV file</label>
              <input type="file" id="import-csv-file" accept=".csv,text/csv">
            </div>
          </section>
          <div id="import-drafts" class="import-drafts" style="margin-top:14px"></div>
        </div>
      </div>
      <div class="modal-actions">
        <button class="secondary" data-cancel>Cancel</button>
        <button id="btn-extract" data-ok>${IMPORT_MODES.ai.action}</button>
      </div>
    </div>
  `;
  document.body.appendChild(root);

  let activeMode = "ai";
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
  function setImportMode(mode) {
    activeMode = mode;
    root.querySelectorAll("[data-import-mode]").forEach((btn) => {
      const active = btn.dataset.importMode === mode;
      btn.classList.toggle("active", active);
      btn.setAttribute("aria-selected", active ? "true" : "false");
    });
    root.querySelectorAll("[data-import-panel]").forEach((panel) => {
      panel.classList.toggle("active", panel.dataset.importPanel === mode);
    });
    root.querySelector("#btn-extract").textContent = IMPORT_MODES[mode].action;
    const draftsRoot = root.querySelector("#import-drafts");
    if (draftsRoot) draftsRoot.innerHTML = "";
    const focusTarget = mode === "ai"
      ? "#import-text"
      : mode === "csv"
        ? "#import-csv-text"
        : "#import-csv-file";
    root.querySelector(focusTarget)?.focus();
  }
  document.addEventListener("keydown", onKey, true);
  root.addEventListener("click", (e) => {
    if (e.target === root) close(true);
  });
  root.querySelector("[data-cancel]").addEventListener("click", () => close(true));
  root.querySelectorAll("[data-import-mode]").forEach((btn) => {
    btn.addEventListener("click", () => setImportMode(btn.dataset.importMode));
  });

  root.querySelector("#btn-extract").addEventListener("click", async () => {
    const btn = root.querySelector("#btn-extract");
    if (btn.disabled) return;
    const draftsRoot = root.querySelector("#import-drafts");
    if (activeMode === "ai") {
      const text = root.querySelector("#import-text").value.trim();
      if (!text) return toast("Paste some text first", "error");
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
          drafts.forEach((draft) => {
            draft.reporter = draft.reporter || state.lastReporter || "";
            draft.priority = draft.priority || "low";
          });
          await reviewImportDrafts(root, drafts, close);
        } catch (e) {
          if (draftsRoot) draftsRoot.innerHTML = "";
          toast(e.message, "error");
        }
      });
      return;
    }

    await withButtonBusy(btn, "Parsing…", async () => {
      try {
        const csvText = activeMode === "csv"
          ? root.querySelector("#import-csv-text").value
          : await readImportCsvFile(root.querySelector("#import-csv-file"));
        const drafts = await parseImportCsvBackend(csvText);
        await reviewImportDrafts(root, drafts, close);
      } catch (e) {
        if (draftsRoot) {
          draftsRoot.innerHTML = `<p class="muted" style="color:var(--error)">${htmlEscape(e.message)}</p>`;
        }
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

async function reviewImportDrafts(root, drafts, close) {
  drawImportDrafts(root, drafts, close);
}

function readImportCsvFile(input) {
  const file = input?.files?.[0];
  if (!file) throw new Error("Choose a CSV file first");
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result || ""));
    reader.onerror = () => reject(new Error("Could not read CSV file"));
    reader.readAsText(file);
  });
}

async function parseImportCsvBackend(text) {
  const r = await api("POST", "/api/import/csv/parse", { text });
  return r.drafts || [];
}

function drawImportDrafts(root, drafts, close, options = {}) {
  const drafts_root = root.querySelector("#import-drafts");
  if (!drafts.length) {
    drafts_root.innerHTML = `<p class="muted">No drafts extracted.</p>`;
    return;
  }
  const duplicateCount = drafts.filter((draft) => draft.duplicate).length;
  const title = options.retry
    ? `Failed drafts (${drafts.length}) — correct &amp; retry`
    : `Drafts (${drafts.length}) — review &amp; confirm`;
  drafts_root.innerHTML = `
    <h3 style="margin-top:0">${title}</h3>
    ${duplicateCount ? `<p class="muted small">${duplicateCount} possible duplicate${duplicateCount === 1 ? "" : "s"} found. Choose whether each is a duplicate before saving again.</p>` : ""}
    ${drafts.map((d, i) => `
      <div class="draft" data-idx="${i}" data-duplicate-decision="${d.duplicate ? (d.duplicateDecision || "move_original_to_backlog") : (d.duplicateDecision || "")}">
        ${d.error ? `<p class="small draft-error" style="margin-top:0;color:#b42318">${htmlEscape(d.error)}</p>` : ""}
        ${d.duplicate ? renderGapDuplicatePrompt(d.duplicate) : ""}
        <input type="text" class="d-name" value="${htmlEscape(d.name || "")}" placeholder="Name">
        <div class="form-grid two" style="margin-top:6px">
          <div class="form-row">
            <label class="small muted">Reporter</label>
            <input type="text" class="d-reporter" value="${htmlEscape(d.reporter || state.lastReporter || "")}" placeholder="Reporter">
          </div>
          <div class="form-row">
            <label class="small muted">Priority</label>
            <select class="d-priority">
              ${["low", "medium", "high"].map((priority) => `
                <option value="${priority}" ${String(d.priority || "low").toLowerCase() === priority ? "selected" : ""}>${priority}</option>`).join("")}
            </select>
          </div>
        </div>
        <div class="form-row" style="margin-top:6px">
          <label class="small muted">Actual</label>
          <textarea class="d-actual" rows="2">${htmlEscape(d.actual || "")}</textarea>
        </div>
        <div class="form-row">
          <label class="small muted">Target</label>
          <textarea class="d-target" rows="3">${htmlEscape(d.target || "")}</textarea>
        </div>
      </div>`).join("")}
  `;
  $$(".import-duplicate-actions button", drafts_root).forEach((btn) => {
    btn.addEventListener("click", () => {
      const draft = btn.closest(".draft");
      draft.dataset.duplicateDecision = btn.dataset.duplicateDecision;
      $$(".import-duplicate-actions button", draft).forEach((candidate) => {
        candidate.classList.toggle("selected", candidate === btn);
      });
    });
  });
  $$(".draft", drafts_root).forEach((draft) => {
    const decision = draft.dataset.duplicateDecision;
    if (!decision) return;
    $$(".import-duplicate-actions button", draft).forEach((btn) => {
      btn.classList.toggle(
        "selected",
        btn.dataset.duplicateDecision === decision,
      );
    });
  });
  $$(".draft", drafts_root).forEach((draft) => {
    $$(".d-actual, .d-target", draft).forEach((field) => {
      field.addEventListener("input", () => {
        if (!draft.querySelector(".import-duplicate")) return;
        draft.dataset.duplicateDecision = "";
        draft.querySelector(".import-duplicate")?.remove();
        draft.querySelector(".draft-error")?.remove();
      });
    });
  });
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
    const rows = $$(".draft", drafts_root);
    const skipped = rows.filter((row) => row.dataset.duplicateDecision === "duplicate").length;
    const payload = rows
      .filter((row) => row.dataset.duplicateDecision !== "duplicate")
      .map((row) => ({
        name: row.querySelector(".d-name").value.trim(),
        actual: row.querySelector(".d-actual").value.trim(),
        target: row.querySelector(".d-target").value.trim(),
        reporter: row.querySelector(".d-reporter").value.trim(),
        priority: row.querySelector(".d-priority").value,
        duplicate_decision: row.dataset.duplicateDecision || "",
      }));
    if (!payload.length) {
      toast(`Skipped ${skipped} duplicate${skipped === 1 ? "" : "s"}; no new gaps created`, "info");
      close(true);
      return;
    }
    await withButtonBusy(btn, "Saving…", async () => {
      try {
        let r = await api("POST", "/api/import/persist", {
          reporter: state.lastReporter || "",
          drafts: payload,
        });
        r = await resolveBackgroundJobResponse(
          r,
          `Saving ${payload.length} gaps in the background`,
        );
        const failures = r.failures || [];
        const createdCount = r.count || 0;
        const duplicateActions = r.duplicate_actions || {};
        const handledDuplicates = (
          skipped
          + (duplicateActions.moved_to_backlog || 0)
          + (duplicateActions.move_noop || 0)
        );
        if (failures.length) {
          const failedDrafts = failures.map((failure) => {
            const original = payload[(failure.index || 1) - 1] || {};
            const duplicate = failure.code === "duplicate_gap"
              ? failure.duplicate?.match
              : null;
            return {
              ...original,
              ...(failure.draft || {}),
              duplicate,
              error: failure.error || "Could not save this Gap.",
            };
          });
          toast(
            `Created ${createdCount} gap${createdCount === 1 ? "" : "s"}; ${failures.length} need fixes`,
            "error",
          );
          drawImportDrafts(root, failedDrafts, close, { retry: true });
        } else {
          const duplicateText = handledDuplicates
            ? `; handled ${handledDuplicates} duplicate${handledDuplicates === 1 ? "" : "s"}`
            : "";
          toast(`Created ${createdCount} gap(s)${duplicateText}`, "info");
          // Stay on the underlying screen — same behavior as the New Gap
          // modal. `close(true)` only redirects when the user came in via
          // the `#/gaps/import` deep link.
          close(true);
        }
      } catch (e) { await showActionError(e, "Import failed"); }
    });
  });
}
