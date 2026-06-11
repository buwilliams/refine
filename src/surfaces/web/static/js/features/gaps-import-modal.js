function openImportModal() {
  if (_importModalOpen) return;
  _importModalOpen = true;

  const reporter = state.lastReporter || "";
  const root = document.createElement("div");
  root.className = "modal-backdrop";
  root.innerHTML = `
    <div class="modal import-modal" role="dialog" aria-modal="true"
         data-testid="import-modal"
         aria-labelledby="import-title">
      <div class="modal-title" id="import-title">Import gaps</div>
      <div class="modal-body" data-testid="import-modal-body" style="max-height:72vh;overflow:auto">
        <nav class="settings-tabs" id="import-tabs" role="tablist" data-testid="import-tabs">
          ${Object.entries(IMPORT_MODES).map(([mode, meta]) => `
            <button type="button" class="settings-tab ${mode === "ai" ? "active" : ""}"
                    data-import-mode="${mode}" role="tab"
                    data-testid="import-tab-${htmlEscape(mode)}"
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
              <textarea id="import-text" data-testid="import-text" rows="8" placeholder="Paste here…"></textarea>
            </div>
          </section>
          <section class="settings-pane import-panel" data-import-panel="csv">
            <div class="form-row">
              <label>CSV text
                <span class="muted small">— required fields: ${IMPORT_CSV_REQUIRED_FIELDS.map(htmlEscape).join(", ")}</span>
              </label>
              <textarea id="import-csv-text" data-testid="import-csv-text" rows="8" placeholder="actual,target,reporter,priority&#10;Current behavior,Desired behavior,Alice,medium"></textarea>
            </div>
            <label class="checkbox-row">
              <input type="checkbox" id="import-csv-distribute" data-testid="import-csv-distribute">
              <span>Distribute across nodes</span>
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
                <button type="button" class="secondary" id="import-csv-file-button" data-testid="import-csv-file-button">Choose CSV</button>
                <span class="import-file-name muted" id="import-csv-file-name" data-testid="import-csv-file-name" aria-live="polite">No file selected</span>
              </div>
              <input type="file" id="import-csv-file" data-testid="import-csv-file" class="visually-hidden" accept=".csv,text/csv">
            </div>
            <label class="checkbox-row">
              <input type="checkbox" id="import-upload-distribute" data-testid="import-upload-distribute">
              <span>Distribute across nodes</span>
            </label>
          </section>
          <div id="import-drafts" class="import-drafts" data-testid="import-drafts" style="margin-top:14px"></div>
        </div>
      </div>
      <div class="modal-actions">
        <button class="secondary" data-cancel data-testid="import-cancel">Cancel</button>
        <button id="btn-extract" data-ok data-testid="import-extract">${IMPORT_MODES.ai.action}</button>
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
        "Cancel this import and discard the recoverable import state? Any running save operation will be asked to stop and roll back Gaps it created.",
        { title: "Cancel import", okLabel: "Cancel import", danger: true },
      );
      if (!ok) return;
    }
    if (activeAbort) {
      activeAbort.abort();
      activeAbort = null;
    }
    if (session.operationId) {
      try {
        await api("POST", `/api/operations/${session.operationId}/cancel`, {});
        await waitForImportOperationCancellation(session.operationId, root, close, saveSession);
      } catch (e) {
        await showActionError(e, "Could not cancel import operation");
        return;
      }
    }
    if (session.prepareOperationId) {
      try {
        await api("POST", `/api/operations/${session.prepareOperationId}/cancel`, {});
        await waitForImportOperationCancellation(session.prepareOperationId, root, close, saveSession);
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
        if (saveSession) saveSession({ prepareOperationId: "", error: "" });
        await reviewImportDrafts(root, drafts, close, saveSession);
      } catch (e) {
        saveSession({ phase: "editing", prepareOperationId: "", error: e.message });
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
  if (session.prepareOperationId) {
    drawImportPrepareProgress(root.querySelector("#import-drafts"), session.progress || {
      message: "Preparing CSV import",
      completed: 0,
      total: 0,
    });
    waitForImportPrepareOperation(session.prepareOperationId, root.querySelector("#import-drafts"), saveSession)
      .then((r) => {
        const drafts = r.drafts || [];
        if (saveSession) saveSession({ phase: "review", drafts, prepareOperationId: "", error: "" });
        drawImportDrafts(root, drafts, close, { saveSession });
      })
      .catch(async (e) => {
        if (e.code === "operation_cancelled") {
          clearImportSession();
          close(true, { force: true });
          return;
        }
        await showActionError(e, "Import failed");
      });
  } else if (session.operationId) {
    drawImportSaving(root, session, close, saveSession);
    const restoredDrafts = (session.drafts || []).map(normalizeImportDraft);
    const skipped = restoredDrafts.filter((draft) => draft.duplicateDecision === "duplicate").length;
    const payload = restoredDrafts
      .filter((draft) => draft.duplicateDecision !== "duplicate")
      .map(importDraftPayload);
    waitForImportPersistOperation(session.operationId, root, close, saveSession)
      .then((r) => handleImportPersistResult(root, r, payload, skipped, close, saveSession))
      .catch(async (e) => {
        if (e.code === "operation_cancelled") {
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
