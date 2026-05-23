// ---- Gaps: import -----------------------------------------------------------

const IMPORT_CHUNK_LINE_COUNT = 20;
const IMPORT_SESSION_KEY = "refine_import_session_v1";
const IMPORT_CSV_REQUIRED_FIELDS = [
  "actual (text)",
  "target (text)",
  "reporter (text)",
  "priority (low, medium, high)",
];
const IMPORT_DRAFT_PAGE_SIZE = 25;
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

function recoverImportSessionOnLoad() {
  const session = readImportSession();
  if (!session || !importSessionIsDirty(session)) return false;
  if (!location.hash.startsWith("#/gaps/import")) {
    location.hash = "#/gaps/import";
  }
  return true;
}

function newImportSession() {
  return {
    id: `import-${Date.now()}-${Math.random().toString(16).slice(2)}`,
    mode: "ai",
    phase: "empty",
    sourceText: "",
    csvText: "",
    uploadText: "",
    fileName: "",
    drafts: [],
    prepareJobId: "",
    jobId: "",
    result: null,
    error: "",
    updatedAt: new Date().toISOString(),
  };
}

function readImportSession() {
  try {
    const raw = localStorage.getItem(IMPORT_SESSION_KEY);
    return raw ? JSON.parse(raw) : null;
  } catch {
    return null;
  }
}

function writeImportSession(session) {
  session.updatedAt = new Date().toISOString();
  localStorage.setItem(IMPORT_SESSION_KEY, JSON.stringify(session));
}

function clearImportSession() {
  localStorage.removeItem(IMPORT_SESSION_KEY);
}

function importSessionIsDirty(session = readImportSession()) {
  if (!session) return false;
  if (session.phase && !["empty", "complete", "cancelled"].includes(session.phase)) return true;
  return !!(
    (session.sourceText || "").trim()
    || (session.csvText || "").trim()
    || (session.uploadText || "").trim()
    || (session.drafts || []).length
    || session.prepareJobId
    || session.jobId
  );
}

function importSessionHasDrafts(session) {
  return !!((session?.drafts || []).length || session?.jobId);
}

function importSessionIsBackgroundSaving(session) {
  return !!(session?.phase === "saving" && session?.jobId);
}

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
              <div class="import-file-control">
                <button type="button" class="secondary" id="import-csv-file-button">Choose CSV</button>
                <span class="import-file-name muted" id="import-csv-file-name" aria-live="polite">No file selected</span>
              </div>
              <input type="file" id="import-csv-file" class="visually-hidden" accept=".csv,text/csv">
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

  let session = readImportSession() || newImportSession();
  let activeMode = session.mode || "ai";
  let closed = false;
  let activeAbort = null;
  function close(navigateAway, options = {}) {
    if (closed) return;
    if (!options.force && importSessionIsBackgroundSaving(session)) {
      toast("Import is running in the background. Reopen Import to check progress; Refine will notify you when it finishes.", "info");
      options = { ...options, allowBackground: true };
    }
    if (!options.force && !options.allowBackground && importSessionIsDirty(session)) {
      toast("Use Cancel to discard or unwind this import before closing.", "error");
      return;
    }
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
  function saveSession(changes = {}) {
    session = { ...session, ...changes };
    writeImportSession(session);
  }
  function markDirtyFromInputs() {
    const sourceText = root.querySelector("#import-text")?.value || "";
    const csvText = root.querySelector("#import-csv-text")?.value || "";
    const hasText = !!(sourceText.trim() || csvText.trim() || (session.uploadText || "").trim());
    saveSession({
      mode: activeMode,
      phase: importSessionHasDrafts(session) ? session.phase : (hasText ? "editing" : "empty"),
      sourceText,
      csvText,
    });
  }
  function setImportMode(mode) {
    if (mode !== activeMode && importSessionHasDrafts(session)) {
      toast("Cancel this import before changing import type.", "error");
      return;
    }
    activeMode = mode;
    saveSession({ mode });
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
    if (draftsRoot && !importSessionHasDrafts(session)) draftsRoot.innerHTML = "";
    const focusTarget = mode === "ai"
      ? "#import-text"
      : mode === "csv"
        ? "#import-csv-text"
        : "#import-csv-file-button";
    root.querySelector(focusTarget)?.focus();
  }
  async function cancelImport() {
    const dirty = importSessionIsDirty(session);
    if (dirty) {
      const ok = await modalConfirm(
        "Cancel this import and discard the recoverable import state? Any running save job will be asked to stop and roll back Gaps it created.",
        { title: "Cancel import", okLabel: "Cancel import", danger: true },
      );
      if (!ok) return;
    }
    if (activeAbort) {
      activeAbort.abort();
      activeAbort = null;
    }
    if (session.jobId) {
      try {
        await api("POST", `/api/jobs/${session.jobId}/cancel`, {});
        await waitForImportJobCancellation(session.jobId, root, close, saveSession);
      } catch (e) {
        await showActionError(e, "Could not cancel import job");
        return;
      }
    }
    if (session.prepareJobId) {
      try {
        await api("POST", `/api/jobs/${session.prepareJobId}/cancel`, {});
        await waitForImportJobCancellation(session.prepareJobId, root, close, saveSession);
      } catch (e) {
        await showActionError(e, "Could not cancel import preparation");
        return;
      }
    }
    clearImportSession();
    close(true, { force: true });
  }
  document.addEventListener("keydown", onKey, true);
  root.addEventListener("click", (e) => {
    if (e.target === root) close(true);
  });
  root.querySelector("[data-cancel]").addEventListener("click", () => cancelImport());
  root.querySelector("#import-csv-file-button").addEventListener("click", () => {
    root.querySelector("#import-csv-file").click();
  });
  root.querySelector("#import-csv-file").addEventListener("change", async (e) => {
    const name = e.target.files?.[0]?.name || "No file selected";
    root.querySelector("#import-csv-file-name").textContent = name;
    if (e.target.files?.[0]) {
      try {
        const uploadText = await readImportCsvFile(e.target);
        saveSession({ mode: "upload", phase: "editing", fileName: name, uploadText });
      } catch (err) {
        toast(err.message, "error");
      }
    }
  });
  root.querySelector("#import-text").addEventListener("input", markDirtyFromInputs);
  root.querySelector("#import-csv-text").addEventListener("input", markDirtyFromInputs);
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
      saveSession({ phase: "extracting", mode: activeMode, sourceText: text, drafts: [], error: "" });
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
          activeAbort = new AbortController();
          const drafts = await extractImportDrafts(text, draftsRoot, activeAbort.signal);
          activeAbort = null;
          drafts.forEach((draft) => {
            draft.reporter = draft.reporter || state.lastReporter || "";
            draft.priority = draft.priority || "low";
          });
          await reviewImportDrafts(root, drafts, close, saveSession);
        } catch (e) {
          activeAbort = null;
          if (e.name === "AbortError") return;
          saveSession({ phase: "editing", error: e.message });
          if (draftsRoot) draftsRoot.innerHTML = "";
          toast(e.message, "error");
        }
      });
      return;
    }

    await withButtonBusy(btn, "Parsing…", async () => {
      try {
        saveSession({ phase: "parsing", mode: activeMode, error: "" });
        const csvText = activeMode === "csv"
          ? root.querySelector("#import-csv-text").value
          : session.uploadText || await readImportCsvFile(root.querySelector("#import-csv-file"));
        saveSession(activeMode === "csv" ? { csvText } : { uploadText: csvText });
        drawImportPrepareProgress(draftsRoot, {
          message: "Preparing CSV import",
          completed: 0,
          total: estimateImportCsvRows(csvText),
        });
        const drafts = await parseImportCsvBackend(csvText, draftsRoot, saveSession);
        if (saveSession) saveSession({ phase: "review", drafts, prepareJobId: "", error: "" });
        drawImportDrafts(root, drafts, close, { saveSession });
      } catch (e) {
        saveSession({ phase: "editing", prepareJobId: "", error: e.message });
        if (draftsRoot) {
          draftsRoot.innerHTML = `<p class="muted" style="color:var(--error)">${htmlEscape(e.message)}</p>`;
        }
        toast(e.message, "error");
      }
    });
  });

  root.querySelector("#import-text").value = session.sourceText || "";
  root.querySelector("#import-csv-text").value = session.csvText || "";
  if (session.fileName) {
    root.querySelector("#import-csv-file-name").textContent = session.fileName;
  }
  setImportMode(activeMode);
  if (session.prepareJobId) {
    drawImportPrepareProgress(root.querySelector("#import-drafts"), session.progress || {
      message: "Preparing CSV import",
      completed: 0,
      total: 0,
    });
    waitForImportPrepareJob(session.prepareJobId, root.querySelector("#import-drafts"), saveSession)
      .then((r) => {
        const drafts = r.drafts || [];
        if (saveSession) saveSession({ phase: "review", drafts, prepareJobId: "", error: "" });
        drawImportDrafts(root, drafts, close, { saveSession });
      })
      .catch(async (e) => {
        if (e.code === "job_cancelled") {
          clearImportSession();
          close(true, { force: true });
          return;
        }
        await showActionError(e, "Import failed");
      });
  } else if (session.jobId) {
    drawImportSaving(root, session, close, saveSession);
    const restoredDrafts = (session.drafts || []).map(normalizeImportDraft);
    const skipped = restoredDrafts.filter((draft) => draft.duplicateDecision === "duplicate").length;
    const payload = restoredDrafts
      .filter((draft) => draft.duplicateDecision !== "duplicate")
      .map(importDraftPayload);
    waitForImportPersistJob(session.jobId, root, close, saveSession)
      .then((r) => handleImportPersistResult(root, r, payload, skipped, close, saveSession))
      .catch(async (e) => {
        if (e.code === "job_cancelled") {
          clearImportSession();
          close(true, { force: true });
          return;
        }
        await showActionError(e, "Import failed");
      });
  } else if ((session.drafts || []).length) {
    drawImportDrafts(root, session.drafts, close, { saveSession, retry: session.phase === "failed" });
  } else if (["extracting", "parsing", "deduping"].includes(session.phase)) {
    saveSession({ phase: "editing" });
    const draftsRoot = root.querySelector("#import-drafts");
    if (draftsRoot) {
      draftsRoot.innerHTML = `<p class="muted">Import was interrupted before drafts were ready. Continue from the saved input above.</p>`;
    }
  }
  const focusTarget = activeMode === "ai"
    ? "#import-text"
    : activeMode === "csv"
      ? "#import-csv-text"
      : "#import-csv-file-button";
  root.querySelector(focusTarget)?.focus();
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

async function extractImportDrafts(text, draftsRoot, signal = null) {
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
      r = await api("POST", "/api/import/extract", { text: chunk.text }, { signal });
    } catch (e) {
      if (e.name === "AbortError") throw e;
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

async function reviewImportDrafts(root, drafts, close, saveSession = null) {
  if (saveSession) saveSession({ phase: "deduping", drafts });
  const annotated = await annotateImportDuplicateDrafts(drafts);
  if (saveSession) saveSession({ phase: "review", drafts: annotated, jobId: "", result: null, error: "" });
  drawImportDrafts(root, annotated, close, { saveSession });
}

async function annotateImportDuplicateDrafts(drafts) {
  if (!drafts.length) return drafts;
  const r = await api("POST", "/api/import/dedup", { drafts });
  const byIndex = new Map((r.matches || []).map((match) => [match.index - 1, match]));
  return drafts.map((draft, idx) => {
    const duplicate = byIndex.get(idx);
    if (!duplicate) return draft;
    return {
      ...draft,
      duplicate: duplicate.match,
      duplicateDecision: draft.duplicateDecision || "",
    };
  });
}

function estimateImportCsvRows(text) {
  return Math.max(0, countImportLines(text) - 1);
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

async function parseImportCsvBackend(text, progressRoot = null, saveSession = null) {
  let r = await api("POST", "/api/import/csv/parse", {
    text,
    background: true,
    dedup: true,
  });
  if (r.job) {
    if (saveSession) {
      saveSession({
        phase: "parsing",
        prepareJobId: r.job.id,
        progress: r.job.progress || {},
      });
    }
    r = await waitForImportPrepareJob(r.job.id, progressRoot, saveSession);
  }
  return r.drafts || [];
}

function drawImportPrepareProgress(root, progress = {}) {
  if (!root) return;
  const completed = Number(progress.completed || 0);
  const total = Number(progress.total || 0);
  const message = progress.message || "Preparing CSV import";
  const detail = total
    ? `${completed} of ${total} Gaps processed.`
    : "Preparing imported Gaps for review.";
  root.innerHTML = `
    <div class="loading-row">
      <span class="loading-spinner"></span>
      <span>${htmlEscape(message)}</span>
    </div>
    <p class="muted small" style="margin:8px 0 0">${htmlEscape(detail)}</p>
  `;
}

async function waitForImportPrepareJob(jobId, progressRoot = null, saveSession = null) {
  while (true) {
    const snap = await api("GET", `/api/jobs/${jobId}`);
    const job = snap.job || {};
    const progress = job.progress || {};
    if (saveSession) {
      saveSession({
        phase: "parsing",
        prepareJobId: jobId,
        progress,
      });
    }
    drawImportPrepareProgress(progressRoot, progress);
    if (job.status === "complete") {
      const result = job.result || {};
      if (result.http_status && result.http_status >= 400) {
        const raw = result.error || {};
        const err = new Error(raw.message || "CSV import preparation failed");
        err.details = raw.details;
        err.code = raw.code;
        throw err;
      }
      return result;
    }
    if (job.status === "cancelled") {
      const err = new Error("Import preparation cancelled");
      err.code = "job_cancelled";
      throw err;
    }
    if (job.status === "failed") {
      const err = new Error(job.error?.message || "CSV import preparation failed");
      err.details = job.error?.details;
      err.code = job.error?.code;
      throw err;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
}

function drawImportDrafts(root, drafts, close, options = {}) {
  const drafts_root = root.querySelector("#import-drafts");
  if (!drafts.length) {
    drafts_root.innerHTML = `<p class="muted">No drafts extracted.</p>`;
    return;
  }
  const draftState = drafts.map(normalizeImportDraft);
  const saveSession = options.saveSession || null;
  let page = 1;
  let showNeedsResolutionOnly = false;

  function renderPage() {
    const visibleDrafts = draftState
      .map((draft, index) => ({ draft, index }))
      .filter(({ draft }) => !showNeedsResolutionOnly || importDraftNeedsResolution(draft));
    const totalPages = Math.max(1, Math.ceil(visibleDrafts.length / IMPORT_DRAFT_PAGE_SIZE));
    page = Math.min(Math.max(1, page), totalPages);
    const start = (page - 1) * IMPORT_DRAFT_PAGE_SIZE;
    const pageDrafts = visibleDrafts.slice(start, start + IMPORT_DRAFT_PAGE_SIZE);
    const end = start + pageDrafts.length;
    const duplicateCount = draftState.filter((draft) => draft.duplicate).length;
    const unresolvedCount = draftState.filter(importDraftNeedsResolution).length;
    const title = options.retry
      ? `Failed drafts (${draftState.length}) — correct &amp; retry`
      : `Drafts (${draftState.length}) — review &amp; confirm`;
    drafts_root.innerHTML = `
      <h3 style="margin-top:0">${title}</h3>
      <div class="import-draft-toolbar">
        <span class="muted small">${renderImportDraftRange(start, end, visibleDrafts.length, draftState.length, showNeedsResolutionOnly)}</span>
        <label class="import-resolution-filter small">
          <input type="checkbox" data-import-unresolved-filter ${showNeedsResolutionOnly ? "checked" : ""}>
          Needs resolution only (${unresolvedCount})
        </label>
        ${renderImportDraftPager(page, totalPages)}
      </div>
      ${duplicateCount ? `<p class="muted small">${duplicateCount} possible duplicate${duplicateCount === 1 ? "" : "s"} found. Choose whether each is a duplicate before saving again.</p>` : ""}
      ${showNeedsResolutionOnly && !visibleDrafts.length
        ? `<p class="muted">No drafts need resolution.</p>`
        : `<div class="import-draft-list">
            ${pageDrafts.map(({ draft, index }) => renderImportDraftRow(draft, index)).join("")}
          </div>`}
      <div class="import-draft-footer">
        ${renderImportDraftPager(page, totalPages)}
      </div>
    `;
    bindImportDraftPage(drafts_root, draftState, saveSession);
    $("[data-import-unresolved-filter]", drafts_root)?.addEventListener("change", (e) => {
      syncImportDraftPage(drafts_root, draftState);
      if (saveSession) saveSession({ phase: "review", drafts: draftState });
      showNeedsResolutionOnly = e.target.checked;
      page = 1;
      renderPage();
    });
    $$("[data-import-page]", drafts_root).forEach((btn) => {
      btn.addEventListener("click", () => {
        syncImportDraftPage(drafts_root, draftState);
        if (saveSession) saveSession({ phase: "review", drafts: draftState });
        page += btn.dataset.importPage === "next" ? 1 : -1;
        renderPage();
      });
    });
  }

  renderPage();
  // Swap the primary action from "Extract drafts" to "Save N gap(s)".
  const actions = root.querySelector(".modal-actions");
  actions.innerHTML = `
    <button class="secondary" data-cancel>Cancel</button>
    <button id="btn-persist">Save ${draftState.length} gap${draftState.length === 1 ? "" : "s"}</button>
  `;
  actions.querySelector("[data-cancel]").addEventListener("click", async () => {
    const ok = await modalConfirm(
      "Cancel this import and discard its draft state?",
      { title: "Cancel import", okLabel: "Cancel import", danger: true },
    );
    if (!ok) return;
    clearImportSession();
    close(true, { force: true });
  });
  actions.querySelector("#btn-persist").addEventListener("click", async () => {
    const btn = actions.querySelector("#btn-persist");
    if (btn.disabled) return;
    syncImportDraftPage(drafts_root, draftState);
    if (saveSession) saveSession({ phase: "review", drafts: draftState });
    const unresolved = draftState.filter(importDraftNeedsResolution);
    if (unresolved.length) {
      showNeedsResolutionOnly = true;
      page = 1;
      renderPage();
      toast(
        `Resolve ${unresolved.length} draft${unresolved.length === 1 ? "" : "s"} before saving`,
        "error",
      );
      return;
    }
    const skipped = draftState.filter((draft) => draft.duplicateDecision === "duplicate").length;
    const payload = draftState
      .filter((draft) => draft.duplicateDecision !== "duplicate")
      .map(importDraftPayload);
    if (!payload.length) {
      toast(`Skipped ${skipped} duplicate${skipped === 1 ? "" : "s"}; no new gaps created`, "info");
      clearImportSession();
      close(true, { force: true });
      return;
    }
    await withButtonBusy(btn, "Saving…", async () => {
      try {
        let r = await api("POST", "/api/import/persist", {
          reporter: state.lastReporter || "",
          drafts: payload,
          background: true,
        });
        if (r.job) {
          if (saveSession) saveSession({ phase: "saving", drafts: draftState, jobId: r.job.id, result: null, error: "" });
          drawImportSaving(root, readImportSession(), close, saveSession);
          r = await waitForImportPersistJob(r.job.id, root, close, saveSession);
        } else {
          if (saveSession) saveSession({ phase: "saving", drafts: draftState, jobId: "", result: null, error: "" });
        }
        handleImportPersistResult(root, r, payload, skipped, close, saveSession);
      } catch (e) {
        if (e.code === "job_cancelled" || e.name === "AbortError") return;
        await showActionError(e, "Import failed");
      }
    });
  });
}

function drawImportSaving(root, session, close, saveSession = null) {
  if (!root.isConnected) return;
  const draftsRoot = root.querySelector("#import-drafts");
  const actions = root.querySelector(".modal-actions");
  if (!draftsRoot || !actions) return;
  const progress = session?.progress || {};
  const message = progress.message || "Saving import";
  const total = Number(progress.total || 0);
  const completed = Number(progress.completed || 0);
  draftsRoot.innerHTML = `
    <div class="loading-row">
      <span class="loading-spinner"></span>
      <span>${htmlEscape(message)}</span>
    </div>
    <p class="muted small" style="margin:8px 0 0">
      ${total ? htmlEscape(`${completed} of ${total} processed.`) : "This import is being saved in the background."}
    </p>
  `;
  actions.innerHTML = `
    <button class="secondary" data-cancel>Cancel</button>
    <button class="secondary" data-hide>Hide</button>
    <button id="btn-persist" disabled>Saving…</button>
  `;
  actions.querySelector("[data-cancel]").addEventListener("click", async () => {
    const ok = await modalConfirm(
      "Cancel this import? Refine will stop the save job and roll back Gaps created by this import.",
      { title: "Cancel import", okLabel: "Cancel import", danger: true },
    );
    if (!ok) return;
    if (session?.jobId) {
      await api("POST", `/api/jobs/${session.jobId}/cancel`, {});
      await waitForImportJobCancellation(session.jobId, root, close, saveSession);
    }
    if (saveSession) saveSession({ phase: "cancelled", jobId: "", drafts: [] });
    clearImportSession();
    close(true, { force: true });
  });
  actions.querySelector("[data-hide]").addEventListener("click", () => {
    close(true, { allowBackground: true });
  });
}

async function waitForImportPersistJob(jobId, root, close, saveSession = null) {
  while (true) {
    const snap = await api("GET", `/api/jobs/${jobId}`);
    const job = snap.job || {};
    if (saveSession) saveSession({ phase: "saving", jobId, progress: job.progress || {} });
    drawImportSaving(root, readImportSession(), close, saveSession);
    if (job.status === "complete") {
      const result = job.result || {};
      if (result.http_status && result.http_status >= 400) {
        const raw = result.error || {};
        const err = new Error(raw.message || "Background job failed");
        err.details = raw.details;
        err.code = raw.code;
        throw err;
      }
      return result;
    }
    if (job.status === "cancelled") {
      const err = new Error("Import cancelled");
      err.code = "job_cancelled";
      throw err;
    }
    if (job.status === "failed") {
      const err = new Error(job.error?.message || "Background job failed");
      err.details = job.error?.details;
      err.code = job.error?.code;
      throw err;
    }
    await new Promise((resolve) => setTimeout(resolve, 750));
  }
}

async function waitForImportJobCancellation(jobId, root, close, saveSession = null) {
  while (true) {
    const snap = await api("GET", `/api/jobs/${jobId}`);
    const job = snap.job || {};
    if (saveSession) {
      saveSession({
        phase: "saving",
        jobId,
        progress: { ...(job.progress || {}), message: "Cancelling" },
      });
    }
    drawImportSaving(root, readImportSession(), close, saveSession);
    if (job.status === "cancelled") return job;
    if (job.status === "complete") return job;
    if (job.status === "failed") {
      const err = new Error(job.error?.message || "Background job failed");
      err.details = job.error?.details;
      err.code = job.error?.code;
      throw err;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
}

function handleImportPersistResult(root, r, payload, skipped, close, saveSession = null) {
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
    if (saveSession) saveSession({ phase: "failed", drafts: failedDrafts, jobId: "", result: r });
    toast(
      root.isConnected
        ? `Created ${createdCount} gap${createdCount === 1 ? "" : "s"}; ${failures.length} need fixes`
        : `Import created ${createdCount} gap${createdCount === 1 ? "" : "s"}; ${failures.length} draft${failures.length === 1 ? "" : "s"} need fixes. Reopen Import to continue.`,
      "error",
    );
    if (root.isConnected) {
      drawImportDrafts(root, failedDrafts, close, { retry: true, saveSession });
    }
  } else {
    const duplicateText = handledDuplicates
      ? `; handled ${handledDuplicates} duplicate${handledDuplicates === 1 ? "" : "s"}`
      : "";
    toast(`Created ${createdCount} gap(s)${duplicateText}`, "info");
    clearImportSession();
    if (root.isConnected) close(true, { force: true });
  }
}

function normalizeImportDraft(draft) {
  return {
    name: draft.name || "",
    actual: draft.actual || "",
    target: draft.target || "",
    reporter: draft.reporter || state.lastReporter || "",
    priority: String(draft.priority || "low").toLowerCase(),
    duplicate: draft.duplicate || null,
    duplicateDecision: draft.duplicateDecision || "",
    error: draft.error || "",
  };
}

function importDraftNeedsResolution(draft) {
  return !!draft.error || (!!draft.duplicate && !draft.duplicateDecision);
}

function renderImportDraftRange(start, end, visibleCount, totalCount, filtered) {
  if (!visibleCount) return filtered ? `Showing 0 of ${totalCount}` : "Showing 0";
  const base = `Showing ${start + 1}-${end} of ${visibleCount}`;
  return filtered ? `${base} needing resolution (${totalCount} total)` : `${base} of ${totalCount}`;
}

function importDraftPayload(draft) {
  return {
    name: draft.name.trim(),
    actual: draft.actual.trim(),
    target: draft.target.trim(),
    reporter: draft.reporter.trim(),
    priority: draft.priority,
    duplicate_decision: draft.duplicateDecision || "",
  };
}

function renderImportDraftPager(page, totalPages) {
  if (totalPages <= 1) return "";
  return `
    <div class="pagination import-draft-pagination">
      <button type="button" class="secondary small" data-import-page="prev" ${page <= 1 ? "disabled" : ""}>Previous</button>
      <span class="muted small">Page ${page} of ${totalPages}</span>
      <button type="button" class="secondary small" data-import-page="next" ${page >= totalPages ? "disabled" : ""}>Next</button>
    </div>`;
}

function renderImportDraftRow(d, index) {
  return `
    <div class="draft" data-idx="${index}" data-duplicate-decision="${htmlEscape(d.duplicateDecision || "")}">
      ${d.error ? `<p class="small draft-error" style="margin-top:0;color:#b42318">${htmlEscape(d.error)}</p>` : ""}
      ${d.duplicate ? renderGapDuplicatePrompt(d.duplicate) : ""}
      <input type="text" class="d-name" value="${htmlEscape(d.name)}" placeholder="Name">
      <div class="form-grid two" style="margin-top:6px">
        <div class="form-row">
          <label class="small muted">Reporter</label>
          <input type="text" class="d-reporter" value="${htmlEscape(d.reporter)}" placeholder="Reporter">
        </div>
        <div class="form-row">
          <label class="small muted">Priority</label>
          <select class="d-priority">
            ${["low", "medium", "high"].map((priority) => `
              <option value="${priority}" ${d.priority === priority ? "selected" : ""}>${priority}</option>`).join("")}
          </select>
        </div>
      </div>
      <div class="form-row" style="margin-top:6px">
        <label class="small muted">Actual</label>
        <textarea class="d-actual" rows="2">${htmlEscape(d.actual)}</textarea>
      </div>
      <div class="form-row">
        <label class="small muted">Target</label>
        <textarea class="d-target" rows="3">${htmlEscape(d.target)}</textarea>
      </div>
    </div>`;
}

function bindImportDraftPage(root, draftState, saveSession = null) {
  $$(".draft", root).forEach((row) => {
    const draft = draftState[Number(row.dataset.idx)];
    if (!draft) return;
    row.dataset.duplicateDecision = draft.duplicateDecision || "";
    $$(".import-duplicate-actions button", row).forEach((btn) => {
      btn.classList.toggle(
        "selected",
        btn.dataset.duplicateDecision === row.dataset.duplicateDecision,
      );
      btn.addEventListener("click", () => {
        row.dataset.duplicateDecision = btn.dataset.duplicateDecision;
        draft.duplicateDecision = btn.dataset.duplicateDecision;
        if (saveSession) saveSession({ phase: "review", drafts: draftState });
        $$(".import-duplicate-actions button", row).forEach((candidate) => {
          candidate.classList.toggle("selected", candidate === btn);
        });
      });
    });
    $$(".d-name, .d-reporter, .d-priority, .d-actual, .d-target", row).forEach((field) => {
      const syncAndClearError = () => {
        syncImportDraftRow(row, draftState);
        draft.error = "";
        if (saveSession) saveSession({ phase: "review", drafts: draftState });
        row.querySelector(".draft-error")?.remove();
      };
      field.addEventListener("input", syncAndClearError);
      field.addEventListener("change", syncAndClearError);
    });
    $$(".d-actual, .d-target", row).forEach((field) => {
      field.addEventListener("input", () => {
        if (!row.querySelector(".import-duplicate")) return;
        row.dataset.duplicateDecision = "";
        draft.duplicateDecision = "";
        draft.duplicate = null;
        draft.error = "";
        if (saveSession) saveSession({ phase: "review", drafts: draftState });
        row.querySelector(".import-duplicate")?.remove();
        row.querySelector(".draft-error")?.remove();
      });
    });
  });
}

function syncImportDraftPage(root, draftState) {
  $$(".draft", root).forEach((row) => syncImportDraftRow(row, draftState));
}

function syncImportDraftRow(row, draftState) {
  const draft = draftState[Number(row.dataset.idx)];
  if (!draft) return;
  draft.name = row.querySelector(".d-name")?.value || "";
  draft.actual = row.querySelector(".d-actual")?.value || "";
  draft.target = row.querySelector(".d-target")?.value || "";
  draft.reporter = row.querySelector(".d-reporter")?.value || "";
  draft.priority = row.querySelector(".d-priority")?.value || "low";
  draft.duplicateDecision = row.dataset.duplicateDecision || "";
}
