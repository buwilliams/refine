// ---- Gaps: import -----------------------------------------------------------

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
    featureDestination: {
      mode: "standalone",
      newName: "",
      newDescription: "",
      existingId: "",
    },
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
         aria-labelledby="import-title">
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
            <label class="checkbox-row">
              <input type="checkbox" id="import-csv-distribute">
              <span>Distribute across cluster nodes</span>
            </label>
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
            <label class="checkbox-row">
              <input type="checkbox" id="import-upload-distribute">
              <span>Distribute across cluster nodes</span>
            </label>
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
    const extractButton = root.querySelector("#btn-extract");
    if (mode !== activeMode && (importSessionHasDrafts(session) || !extractButton)) {
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
    if (extractButton) extractButton.textContent = IMPORT_MODES[mode].action;
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
          phase: "starting",
          lineCount: countImportLines(text),
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
        const distribute = activeMode === "csv"
          ? !!root.querySelector("#import-csv-distribute")?.checked
          : !!root.querySelector("#import-upload-distribute")?.checked;
        const drafts = await parseImportCsvBackend(csvText, draftsRoot, saveSession, { distribute });
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

async function extractImportDrafts(text, draftsRoot, signal = null) {
  const lineCount = countImportLines(text);
  if (draftsRoot) {
    drawImportProgress(draftsRoot, {
      phase: "running",
      lineCount,
    });
  }
  let r = null;
  try {
    r = await api("POST", "/api/import/extract", { text }, { signal });
  } catch (e) {
    if (e.name === "AbortError") throw e;
    throw new Error(`AI extraction failed: ${e.message}`);
  }
  const drafts = r.drafts || [];
  if (draftsRoot) {
    drawImportProgress(draftsRoot, {
      phase: "complete",
      lineCount,
      draftCount: drafts.length,
    });
  }
  return drafts;
}

function drawImportProgress(root, state) {
  const lineCount = Number(state.lineCount || 0);
  const draftCount = Number(state.draftCount || 0);
  const status = state.phase === "complete"
    ? `AI extracted ${draftCount} draft${draftCount === 1 ? "" : "s"}.`
    : "Asking the selected AI provider to extract Gaps.";
  const detail = lineCount
    ? `The full ${lineCount}-line input is being sent as one request so the agent can use the whole context.`
    : "The full input is being sent as one request so the agent can use the whole context.";
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

async function openPlanDraftModalFromText(text) {
  const root = document.createElement("div");
  root.className = "modal-backdrop";
  root.innerHTML = `
    <div class="modal import-modal" role="dialog" aria-modal="true"
         aria-labelledby="plan-drafts-title">
      <div class="modal-title" id="plan-drafts-title">Plan drafts</div>
      <div class="modal-body" style="max-height:72vh;overflow:auto">
        <div class="muted small" style="margin-bottom:8px">
          Review and edit drafted Gaps before saving.
        </div>
        <div id="import-drafts" class="import-drafts"></div>
      </div>
      <div class="modal-actions">
        <button class="secondary" data-cancel>Cancel</button>
      </div>
    </div>
  `;
  document.body.appendChild(root);
  let closed = false;
  let abort = new AbortController();
  function close(_navigateAway, _options = {}) {
    if (closed) return;
    closed = true;
    abort.abort();
    document.removeEventListener("keydown", onKey, true);
    root.remove();
  }
  function onKey(e) {
    if (e.key === "Escape") {
      e.preventDefault();
      close(false);
    }
  }
  document.addEventListener("keydown", onKey, true);
  root.addEventListener("click", (e) => {
    if (e.target === root) close(false);
  });
  root.querySelector("[data-cancel]").addEventListener("click", () => close(false));
  const draftsRoot = root.querySelector("#import-drafts");
  try {
    const drafts = await extractImportDrafts(text, draftsRoot, abort.signal);
    drafts.forEach((draft) => {
      draft.reporter = draft.reporter || state.lastReporter || "";
      draft.priority = draft.priority || "low";
    });
    if (closed) return;
    const annotated = await annotateImportDuplicateDrafts(drafts);
    if (closed) return;
    drawImportDrafts(root, annotated, close, {
      clearSession: false,
      featureDestination: {
        mode: "new",
        newName: inferPlanFeatureName(text),
        newDescription: "Created from Plan Mode.",
        existingId: "",
      },
    });
  } catch (e) {
    if (e.name === "AbortError") return;
    if (draftsRoot) {
      draftsRoot.innerHTML = `<p class="muted" style="color:var(--error)">${htmlEscape(e.message)}</p>`;
    }
  }
}

function inferPlanFeatureName(text) {
  const firstLine = String(text || "")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .find(Boolean) || "Planned Feature";
  const cleaned = firstLine
    .replace(/^(plan|feature|proposal)\s*[:\-]\s*/i, "")
    .trim() || "Planned Feature";
  return cleaned.length > 80 ? cleaned.slice(0, 77).trimEnd() + "..." : cleaned;
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

async function parseImportCsvBackend(text, progressRoot = null, saveSession = null, options = {}) {
  let r = await api("POST", "/api/import/csv/parse", {
    text,
    background: true,
    dedup: true,
    distribute: !!options.distribute,
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
  const isParsing = /^Pars/i.test(message);
  const detail = total
    ? `${completed} of ${total} Gaps ${isParsing ? "parsed" : "processed"}.`
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
  const clearSessionOnClose = options.clearSession !== false;
  let featureDestination = normalizeImportFeatureDestination(
    options.featureDestination || (readImportSession() || {}).featureDestination,
  );
  let page = 1;
  let showNeedsResolutionOnly = false;
  let originalUpdateField = "actual";

  function renderPage() {
    const reviewDrafts = draftState
      .map((draft, index) => ({ draft, index }))
      .filter(({ draft }) => !importDraftHiddenFromReview(draft));
    const visibleDrafts = reviewDrafts
      .filter(({ draft }) => !showNeedsResolutionOnly || importDraftNeedsResolution(draft));
    const totalPages = Math.max(1, Math.ceil(visibleDrafts.length / IMPORT_DRAFT_PAGE_SIZE));
    page = Math.min(Math.max(1, page), totalPages);
    const start = (page - 1) * IMPORT_DRAFT_PAGE_SIZE;
    const pageDrafts = visibleDrafts.slice(start, start + IMPORT_DRAFT_PAGE_SIZE);
    const end = start + pageDrafts.length;
    const pageSelectedCount = pageDrafts.filter(({ draft }) => draft.selected).length;
    const pageAllSelected = !!pageDrafts.length && pageSelectedCount === pageDrafts.length;
    const pageSomeSelected = pageSelectedCount > 0 && !pageAllSelected;
    const allFilteredSelected = !!visibleDrafts.length && visibleDrafts.every(({ draft }) => draft.selected);
    const duplicateCount = reviewDrafts.filter(({ draft }) => draft.duplicate).length;
    const unresolvedCount = reviewDrafts.filter(({ draft }) => importDraftNeedsResolution(draft)).length;
    const selectedCount = reviewDrafts.filter(({ draft }) => draft.selected).length;
    const title = options.retry
      ? `Failed drafts (${reviewDrafts.length}) — correct &amp; retry`
      : `Drafts (${reviewDrafts.length}) — review &amp; confirm`;
    drafts_root.innerHTML = `
      <h3 style="margin-top:0">${title}</h3>
      ${renderImportDraftActionBar({
        start,
        end,
        visibleCount: visibleDrafts.length,
        totalCount: reviewDrafts.length,
        filtered: showNeedsResolutionOnly,
        unresolvedCount,
        selectedCount,
        duplicateCount,
        totalPages,
        page,
        pageAllSelected,
        pageSomeSelected,
        allFilteredSelected,
        updateField: originalUpdateField,
      })}
      ${renderImportFeatureDestination(featureDestination)}
      ${duplicateCount ? `<p class="muted small">${duplicateCount} possible duplicate${duplicateCount === 1 ? "" : "s"} found. Resolve them with the bulk actions before saving.</p>` : ""}
      ${!visibleDrafts.length
        ? `<p class="muted">${showNeedsResolutionOnly ? "No drafts need resolution." : "No drafts remain in this review."}</p>`
        : renderImportDraftTable(pageDrafts, {
            pageAllSelected,
            pageSomeSelected,
            draftCount: draftState.length,
          })}
      <div class="import-draft-footer">
        ${renderImportDraftPager(page, totalPages)}
      </div>
    `;
    updateImportPersistButton(root, draftState, featureDestination);
    bindImportFeatureDestination(drafts_root, (next) => {
      featureDestination = next;
      if (saveSession) saveSession({ phase: "review", drafts: draftState, featureDestination });
      updateImportPersistButton(root, draftState, featureDestination);
    });
    bindImportDraftPage(drafts_root, draftState, saveSession, {
      featureDestination: () => featureDestination,
      onSelectionChange: renderPage,
    });
    const persistReviewState = () => {
      syncImportDraftPage(drafts_root, draftState);
      if (saveSession) saveSession({ phase: "review", drafts: draftState, featureDestination });
    };
    $("[data-import-unresolved-filter]", drafts_root)?.addEventListener("change", (e) => {
      persistReviewState();
      showNeedsResolutionOnly = e.target.checked;
      page = 1;
      renderPage();
    });
    $("[data-import-toggle-page]", drafts_root)?.addEventListener("click", () => {
      syncImportDraftPage(drafts_root, draftState);
      for (const { draft } of visibleDrafts.slice(start, start + IMPORT_DRAFT_PAGE_SIZE)) {
        draft.selected = !pageAllSelected;
      }
      if (saveSession) saveSession({ phase: "review", drafts: draftState, featureDestination });
      renderPage();
    });
    $("[data-import-toggle-all]", drafts_root)?.addEventListener("click", () => {
      persistReviewState();
      visibleDrafts.forEach(({ draft }) => {
        draft.selected = !allFilteredSelected;
      });
      if (saveSession) saveSession({ phase: "review", drafts: draftState, featureDestination });
      renderPage();
    });
    $("[data-import-select-duplicates]", drafts_root)?.addEventListener("click", () => {
      persistReviewState();
      reviewDrafts.forEach(({ draft }) => {
        draft.selected = !!draft.duplicate;
      });
      if (saveSession) saveSession({ phase: "review", drafts: draftState, featureDestination });
      renderPage();
    });
    $("[data-import-dismiss-duplicates]", drafts_root)?.addEventListener("click", () => {
      persistReviewState();
      const targets = reviewDrafts
        .map(({ draft }) => draft)
        .filter((draft) => draft.duplicate);
      targets.forEach((draft) => {
        draft.duplicateDecision = "duplicate";
        draft.selected = false;
      });
      if (saveSession) saveSession({ phase: "review", drafts: draftState, featureDestination });
      toast(`Dismissed ${targets.length} duplicate${targets.length === 1 ? "" : "s"}`, "info");
      renderPage();
    });
    $("[data-import-originals]", drafts_root)?.addEventListener("click", () => {
      persistReviewState();
      const targets = draftState.filter((draft) => draft.selected && draft.duplicate);
      if (!targets.length) {
        toast("Select duplicate drafts to import as originals.", "warn");
        return;
      }
      targets.forEach((draft) => {
        draft.duplicateDecision = "original";
        draft.selected = false;
      });
      if (saveSession) saveSession({ phase: "review", drafts: draftState, featureDestination });
      toast(`Marked ${targets.length} duplicate draft${targets.length === 1 ? "" : "s"} to import`, "info");
      renderPage();
    });
    $("[data-import-backlog-originals]", drafts_root)?.addEventListener("click", () => {
      persistReviewState();
      const targets = draftState.filter((draft) => draft.selected && draft.duplicate);
      if (!targets.length) {
        toast("Select duplicate drafts whose originals should move to backlog.", "warn");
        return;
      }
      targets.forEach((draft) => {
        draft.duplicateDecision = "move_original_to_backlog";
        draft.selected = false;
      });
      if (saveSession) saveSession({ phase: "review", drafts: draftState, featureDestination });
      toast(`Marked ${targets.length} original Gap${targets.length === 1 ? "" : "s"} for backlog`, "info");
      renderPage();
    });
    $("[data-import-update-field]", drafts_root)?.addEventListener("change", (e) => {
      originalUpdateField = e.target.value || "actual";
    });
    $("[data-import-update-originals]", drafts_root)?.addEventListener("click", () => {
      persistReviewState();
      const field = $("[data-import-update-field]", drafts_root)?.value || "actual";
      const targets = draftState.filter((draft) => draft.selected && draft.duplicate);
      if (!targets.length) {
        toast("Select duplicate drafts to update originals.", "warn");
        return;
      }
      targets.forEach((draft) => {
        draft.duplicateDecision = `update_original_${field}`;
        draft.selected = false;
      });
      if (saveSession) saveSession({ phase: "review", drafts: draftState, featureDestination });
      toast(`Marked ${targets.length} original Gap${targets.length === 1 ? "" : "s"} for ${field} update`, "info");
      renderPage();
    });
    $$("[data-import-draft-move]", drafts_root).forEach((btn) => {
      btn.addEventListener("click", () => {
        syncImportDraftPage(drafts_root, draftState);
        const idx = parseInt(btn.dataset.idx || "-1", 10);
        const direction = btn.dataset.importDraftMove;
        const swapIdx = direction === "up" ? idx - 1 : idx + 1;
        if (idx < 0 || swapIdx < 0 || swapIdx >= draftState.length) return;
        [draftState[idx], draftState[swapIdx]] = [draftState[swapIdx], draftState[idx]];
        if (saveSession) saveSession({ phase: "review", drafts: draftState, featureDestination });
        renderPage();
      });
    });
    $$("[data-import-page]", drafts_root).forEach((btn) => {
      btn.addEventListener("click", () => {
        persistReviewState();
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
    <button id="btn-persist"></button>
  `;
  updateImportPersistButton(root, draftState, featureDestination);
  actions.querySelector("[data-cancel]").addEventListener("click", async () => {
    const ok = await modalConfirm(
      "Cancel this import and discard its draft state?",
      { title: "Cancel import", okLabel: "Cancel import", danger: true },
    );
    if (!ok) return;
    if (clearSessionOnClose) clearImportSession();
    close(true, { force: true });
  });
  actions.querySelector("#btn-persist").addEventListener("click", async () => {
    const btn = actions.querySelector("#btn-persist");
    if (btn.disabled) return;
    syncImportDraftPage(drafts_root, draftState);
    if (saveSession) saveSession({ phase: "review", drafts: draftState, featureDestination });
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
      if (clearSessionOnClose) clearImportSession();
      close(true, { force: true });
      return;
    }
    featureDestination = readImportFeatureDestination(drafts_root);
    if (saveSession) saveSession({ phase: "review", drafts: draftState, featureDestination });
    let destinationPayload = {};
    try {
      destinationPayload = importFeatureDestinationPayload(featureDestination);
    } catch (e) {
      toast(e.message, "error");
      return;
    }
    await withButtonBusy(btn, "Saving…", async () => {
      try {
        let r = await api("POST", "/api/import/persist", {
          reporter: state.lastReporter || "",
          drafts: payload,
          background: true,
          ...destinationPayload,
        });
        if (r.job) {
          if (saveSession) saveSession({ phase: "saving", drafts: draftState, featureDestination, jobId: r.job.id, result: null, error: "" });
          drawImportSaving(root, readImportSession(), close, saveSession);
          r = await waitForImportPersistJob(r.job.id, root, close, saveSession);
        } else {
          if (saveSession) saveSession({ phase: "saving", drafts: draftState, featureDestination, jobId: "", result: null, error: "" });
        }
        await handleImportPersistResult(root, r, payload, skipped, close, saveSession, {
          clearSession: clearSessionOnClose,
        });
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

async function handleImportPersistResult(root, r, payload, skipped, close, saveSession = null, options = {}) {
  await refreshReportersAfterImport();
  const failures = r.failures || [];
  const createdCount = r.count || 0;
  const duplicateActions = r.duplicate_actions || {};
  const handledDuplicates = (
    skipped
    + (duplicateActions.moved_to_backlog || 0)
    + (duplicateActions.move_noop || 0)
    + (duplicateActions.updated_original || 0)
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
      drawImportDrafts(root, failedDrafts, close, {
        retry: true,
        saveSession,
        clearSession: options.clearSession !== false,
      });
    }
  } else {
    const duplicateText = handledDuplicates
      ? `; handled ${handledDuplicates} duplicate${handledDuplicates === 1 ? "" : "s"}`
      : "";
    toast(`Created ${createdCount} gap(s)${duplicateText}`, "info");
    if (options.clearSession !== false) clearImportSession();
    if (root.isConnected) close(true, { force: true });
  }
}

async function refreshReportersAfterImport() {
  try {
    await refreshReporters();
  } catch {
    // SSE still refreshes reporters for other tabs or transient API failures.
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
    node_id: draft.node_id || "",
    selected: !!draft.selected,
    error: draft.error || "",
  };
}

function importDraftNeedsResolution(draft) {
  if (importDraftHiddenFromReview(draft)) return false;
  return !!draft.error || (!!draft.duplicate && !draft.duplicateDecision);
}

function importDraftHiddenFromReview(draft) {
  return draft.duplicateDecision === "duplicate";
}

function importDraftCreatesGap(draft) {
  const decision = draft.duplicateDecision || "";
  return !(
    decision === "duplicate"
    || decision === "move_original_to_backlog"
    || decision.startsWith("update_original_")
  );
}

function importDraftCreateCount(drafts) {
  return drafts.filter(importDraftCreatesGap).length;
}

function updateImportPersistButton(root, draftState, featureDestination = null) {
  const btn = root.querySelector("#btn-persist");
  if (!btn) return;
  const count = importDraftCreateCount(draftState);
  const destination = normalizeImportFeatureDestination(featureDestination);
  const suffix = destination.mode === "new"
    ? " to new Feature"
    : destination.mode === "existing"
      ? " to Feature"
      : "";
  btn.textContent = `Save (${count}) gap${count === 1 ? "" : "s"}${suffix}`;
}

function normalizeImportFeatureDestination(raw = null) {
  const mode = ["standalone", "new", "existing"].includes(raw?.mode)
    ? raw.mode
    : "standalone";
  return {
    mode,
    newName: String(raw?.newName || ""),
    newDescription: String(raw?.newDescription || ""),
    existingId: String(raw?.existingId || ""),
  };
}

function renderImportFeatureDestination(destination) {
  const dest = normalizeImportFeatureDestination(destination);
  return `
    <div class="import-feature-destination">
      <div class="small" style="font-weight:600">Save destination</div>
      <div class="filter-row">
        <label class="checkbox-row">
          <input type="radio" name="import-feature-mode" value="standalone" ${dest.mode === "standalone" ? "checked" : ""}>
          <span>Standalone Gaps</span>
        </label>
        <label class="checkbox-row">
          <input type="radio" name="import-feature-mode" value="new" ${dest.mode === "new" ? "checked" : ""}>
          <span>New Feature</span>
        </label>
        <label class="checkbox-row">
          <input type="radio" name="import-feature-mode" value="existing" ${dest.mode === "existing" ? "checked" : ""}>
          <span>Existing Feature</span>
        </label>
      </div>
      <div class="import-feature-destination-fields" data-import-feature-fields="new" ${dest.mode === "new" ? "" : "hidden"}>
        <div class="form-row">
          <label>Feature name</label>
          <input type="text" data-import-feature-new-name value="${htmlEscape(dest.newName)}" placeholder="Settings redesign">
        </div>
        <div class="form-row">
          <label>Feature description</label>
          <textarea data-import-feature-new-description rows="3">${htmlEscape(dest.newDescription)}</textarea>
        </div>
      </div>
      <div class="import-feature-destination-fields" data-import-feature-fields="existing" ${dest.mode === "existing" ? "" : "hidden"}>
        <div class="form-row">
          <label>Feature</label>
          <select data-import-feature-existing class="modal-input" data-selected="${htmlEscape(dest.existingId)}">
            <option value="">Loading Features...</option>
          </select>
        </div>
      </div>
      <p class="muted small" data-import-feature-summary>${htmlEscape(importFeatureDestinationSummary(dest))}</p>
    </div>`;
}

function bindImportFeatureDestination(root, onChange) {
  const apply = () => {
    const dest = readImportFeatureDestination(root);
    root.querySelectorAll("[data-import-feature-fields]").forEach((el) => {
      el.hidden = el.dataset.importFeatureFields !== dest.mode;
    });
    const summary = root.querySelector("[data-import-feature-summary]");
    if (summary) summary.textContent = importFeatureDestinationSummary(dest);
    onChange(dest);
  };
  root.querySelectorAll("input[name='import-feature-mode']").forEach((input) => {
    input.addEventListener("change", apply);
  });
  root.querySelector("[data-import-feature-new-name]")?.addEventListener("input", debounce(apply, 150));
  root.querySelector("[data-import-feature-new-description]")?.addEventListener("input", debounce(apply, 150));
  const select = root.querySelector("[data-import-feature-existing]");
  if (select) {
    select.addEventListener("change", apply);
    populateImportFeatureSelect(select).then(apply).catch(() => {
      select.innerHTML = `<option value="">Could not load Features</option>`;
    });
  }
}

function readImportFeatureDestination(root) {
  return normalizeImportFeatureDestination({
    mode: root.querySelector("input[name='import-feature-mode']:checked")?.value || "standalone",
    newName: root.querySelector("[data-import-feature-new-name]")?.value || "",
    newDescription: root.querySelector("[data-import-feature-new-description]")?.value || "",
    existingId: root.querySelector("[data-import-feature-existing]")?.value || "",
  });
}

async function populateImportFeatureSelect(select) {
  const selected = select.dataset.selected || "";
  const data = await api("GET", "/api/features?limit=100&node=current");
  const features = data.features || [];
  select.innerHTML = features.length
    ? features.map((feature) => `
        <option value="${htmlEscape(feature.id)}" ${feature.id === selected ? "selected" : ""}>
          ${htmlEscape(feature.name || feature.id)} · ${htmlEscape(feature.status || "backlog")} · ${feature.done_count || 0}/${feature.gap_count || 0} done
        </option>`).join("")
    : `<option value="">No Features available</option>`;
}

function importFeatureDestinationSummary(dest) {
  if (dest.mode === "new") {
    return dest.newName
      ? `Creates Feature "${dest.newName}" and saves imported Gaps in reviewed order.`
      : "Creates a new Feature and saves imported Gaps in reviewed order.";
  }
  if (dest.mode === "existing") {
    return dest.existingId
      ? `Appends imported Gaps to Feature ${dest.existingId} in reviewed order.`
      : "Choose an existing Feature before saving.";
  }
  return "Saves imported Gaps as standalone Gaps.";
}

function importFeatureDestinationPayload(destination) {
  const dest = normalizeImportFeatureDestination(destination);
  if (dest.mode === "new") {
    if (!dest.newName.trim()) {
      throw new Error("Feature name is required");
    }
    return {
      new_feature_name: dest.newName.trim(),
      new_feature_description: dest.newDescription.trim(),
    };
  }
  if (dest.mode === "existing") {
    if (!dest.existingId.trim()) {
      throw new Error("Choose a Feature before saving");
    }
    return { feature_id: dest.existingId.trim() };
  }
  return {};
}

function renderImportDraftActionBar({
  start,
  end,
  visibleCount,
  totalCount,
  filtered,
  unresolvedCount,
  selectedCount,
  duplicateCount,
  totalPages,
  page,
  pageAllSelected,
  pageSomeSelected,
  allFilteredSelected,
  updateField,
}) {
  const pageInfo = renderImportDraftRange(start, end, visibleCount, totalCount, filtered);
  return `
    <details class="filter-shell import-review-shell" open>
      <summary>
        <span class="filter-shell-title">Filters &amp; bulk actions</span>
        <span class="filter-pill">${selectedCount} selected</span>
        ${filtered ? `<span class="filter-pill">Needs resolution</span>` : ""}
      </summary>
      <div class="filter-shell-body">
        <div class="filter-bar">
          <div class="filter-row filter-row-primary">
            <label class="import-resolution-filter small">
              <input type="checkbox" data-import-unresolved-filter ${filtered ? "checked" : ""}>
              Needs resolution (${unresolvedCount})
            </label>
            <span class="muted small">${htmlEscape(pageInfo)}</span>
            <span class="muted small">${selectedCount} selected</span>
            ${duplicateCount ? `<span class="muted small">${duplicateCount} duplicate${duplicateCount === 1 ? "" : "s"}</span>` : ""}
          </div>
          <div class="filter-row filter-row-bulk">
            <span class="muted small">Bulk update selected:</span>
            <button type="button" class="secondary small" data-import-toggle-page ${visibleCount ? "" : "disabled"}>
              ${pageAllSelected ? "Deselect page" : "Select page"}
            </button>
            <button type="button" class="secondary small" data-import-toggle-all ${visibleCount ? "" : "disabled"}>
              ${allFilteredSelected ? "Deselect all" : "Select all"}
            </button>
            <button type="button" class="secondary small" data-import-select-duplicates ${duplicateCount ? "" : "disabled"}>Select duplicates</button>
            <button type="button" class="secondary small" data-import-dismiss-duplicates ${duplicateCount ? "" : "disabled"}>Dismiss duplicates</button>
            <button type="button" class="secondary small" data-import-originals>Import selected</button>
            <button type="button" class="secondary small" data-import-backlog-originals>Move originals to backlog</button>
            <select data-import-update-field aria-label="Original Gap field">
              ${["actual", "target", "reporter", "priority"].map((field) => `
                <option value="${field}" ${field === updateField ? "selected" : ""}>${field}</option>`).join("")}
            </select>
            <button type="button" class="secondary small" data-import-update-originals>Update originals</button>
          </div>
        </div>
        ${totalPages > 1 ? `<span class="muted small">Page ${page} of ${totalPages}</span>` : ""}
      </div>
    </details>`;
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
    node_id: (draft.node_id || "").trim(),
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

function renderImportDraftTable(pageDrafts, { pageAllSelected, pageSomeSelected, draftCount }) {
  return `
    <table class="table import-drafts-table">
      <colgroup>
        <col class="import-col-select">
        <col class="import-col-order">
        <col class="import-col-name">
        <col class="import-col-reporter">
        <col class="import-col-priority">
        <col class="import-col-node">
        <col class="import-col-actual">
        <col class="import-col-target">
      </colgroup>
      <thead>
        <tr>
          <th class="gap-select-col">
            <input type="checkbox" data-import-toggle-page-checkbox
                   aria-label="Select page"
                   ${pageAllSelected ? "checked" : ""}
                   data-indeterminate="${pageSomeSelected ? "1" : "0"}">
          </th>
          <th>Order</th>
          <th>Name</th>
          <th>Reporter</th>
          <th>Priority</th>
          <th>Node</th>
          <th>Actual</th>
          <th>Target</th>
        </tr>
      </thead>
      <tbody>
        ${pageDrafts.map(({ draft, index }) => renderImportDraftRow(draft, index, draftCount)).join("")}
      </tbody>
    </table>`;
}

function renderImportDraftRow(d, index, draftCount) {
  return `
    <tr class="draft ${importDraftNeedsResolution(d) ? "needs-resolution" : ""}"
        data-idx="${index}" data-duplicate-decision="${htmlEscape(d.duplicateDecision || "")}">
      <td class="gap-select-col">
        <input type="checkbox" data-import-draft-select ${d.selected ? "checked" : ""}
               aria-label="Select draft ${index + 1}">
      </td>
      <td>
        <div class="actions compact-actions">
          <button type="button" class="secondary small" data-import-draft-move="up" data-idx="${index}" ${index === 0 ? "disabled" : ""}>Up</button>
          <button type="button" class="secondary small" data-import-draft-move="down" data-idx="${index}" ${index >= draftCount - 1 ? "disabled" : ""}>Down</button>
        </div>
      </td>
      <td>
        <input type="text" class="d-name" value="${htmlEscape(d.name)}" placeholder="Name">
        ${d.error ? `<p class="small draft-error">${htmlEscape(d.error)}</p>` : ""}
        ${d.duplicate ? `<p class="muted small import-decision-label">${htmlEscape(importDuplicateDecisionLabel(d.duplicateDecision))}</p>` : ""}
      </td>
      <td><input type="text" class="d-reporter" value="${htmlEscape(d.reporter)}" placeholder="Reporter"></td>
      <td>
        <select class="d-priority">
          ${["low", "medium", "high"].map((priority) => `
            <option value="${priority}" ${d.priority === priority ? "selected" : ""}>${priority}</option>`).join("")}
        </select>
      </td>
      <td><input type="text" class="d-node" value="${htmlEscape(d.node_id || "")}" placeholder="current"></td>
      <td>
        <textarea class="d-actual" rows="3">${htmlEscape(d.actual)}</textarea>
        ${d.duplicate ? renderImportDuplicateActual(d.duplicate) : ""}
      </td>
      <td>
        <textarea class="d-target" rows="3">${htmlEscape(d.target)}</textarea>
        ${d.duplicate ? renderImportDuplicateTarget(d.duplicate) : ""}
      </td>
    </tr>`;
}

function importDuplicateDecisionLabel(decision) {
  if (decision === "duplicate") return "Duplicate dismissed";
  if (decision === "original") return "Will import as original";
  if (decision === "move_original_to_backlog") return "Will move original to backlog";
  if (decision?.startsWith("update_original_")) {
    return `Will update original ${decision.replace("update_original_", "")}`;
  }
  return "Needs duplicate resolution";
}

function renderImportDuplicateActual(match) {
  return `
    <div class="import-duplicate">
      <div class="small" style="font-weight:600">Possible duplicate</div>
      <p class="muted small" style="margin:4px 0">
        ${htmlEscape(match.name || match.id)} · ${htmlEscape(match.node_display_name || match.node_id || "Default")}
        · ${htmlEscape(match.status || "")}
      </p>
      <div class="small muted">Matched actual</div>
      <p>${htmlEscape(match.actual || "")}</p>
    </div>`;
}

function renderImportDuplicateTarget(match) {
  return `
    <div class="import-duplicate">
      <div class="small muted">Matched target</div>
      <p>${htmlEscape(match.target || "")}</p>
    </div>`;
}

function bindImportDraftPage(root, draftState, saveSession = null, options = {}) {
  const saveReview = () => {
    if (!saveSession) return;
    const destination = typeof options.featureDestination === "function"
      ? options.featureDestination()
      : options.featureDestination;
    saveSession({
      phase: "review",
      drafts: draftState,
      ...(destination ? { featureDestination: destination } : {}),
    });
  };
  const pageToggle = root.querySelector("[data-import-toggle-page-checkbox]");
  if (pageToggle) {
    pageToggle.indeterminate = pageToggle.dataset.indeterminate === "1";
    pageToggle.addEventListener("change", () => {
      root.querySelector("[data-import-toggle-page]")?.click();
    });
  }
  $$(".draft", root).forEach((row) => {
    const draft = draftState[Number(row.dataset.idx)];
    if (!draft) return;
    row.dataset.duplicateDecision = draft.duplicateDecision || "";
    row.querySelector("[data-import-draft-select]")?.addEventListener("change", (e) => {
      draft.selected = e.target.checked;
      saveReview();
      if (typeof options.onSelectionChange === "function") options.onSelectionChange();
    });
    $$(".import-duplicate-actions button", row).forEach((btn) => {
      btn.classList.toggle(
        "selected",
        btn.dataset.duplicateDecision === row.dataset.duplicateDecision,
      );
      btn.addEventListener("click", () => {
        row.dataset.duplicateDecision = btn.dataset.duplicateDecision;
        draft.duplicateDecision = btn.dataset.duplicateDecision;
        saveReview();
        $$(".import-duplicate-actions button", row).forEach((candidate) => {
          candidate.classList.toggle("selected", candidate === btn);
        });
      });
    });
    $$(".d-name, .d-reporter, .d-priority, .d-actual, .d-target", row).forEach((field) => {
      const syncAndClearError = () => {
        syncImportDraftRow(row, draftState);
        draft.error = "";
        saveReview();
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
        saveReview();
        row.querySelector(".import-duplicate")?.remove();
        row.querySelectorAll(".import-duplicate").forEach((el) => el.remove());
        row.querySelector(".draft-error")?.remove();
        row.querySelector(".import-decision-label")?.remove();
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
  draft.selected = !!row.querySelector("[data-import-draft-select]")?.checked;
  draft.name = row.querySelector(".d-name")?.value || "";
  draft.actual = row.querySelector(".d-actual")?.value || "";
  draft.target = row.querySelector(".d-target")?.value || "";
  draft.reporter = row.querySelector(".d-reporter")?.value || "";
  draft.priority = row.querySelector(".d-priority")?.value || "low";
  draft.node_id = row.querySelector(".d-node")?.value || "";
  draft.duplicateDecision = row.dataset.duplicateDecision || "";
}
