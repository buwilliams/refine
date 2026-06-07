function countImportLines(text) {
  return text.split(/\r?\n/).map((line) => line.trim()).filter(Boolean).length;
}

async function extractImportDrafts(text, draftsRoot, signal = null, options = {}) {
  const lineCount = countImportLines(text);
  if (draftsRoot) {
    drawImportProgress(draftsRoot, {
      phase: "running",
      lineCount,
    });
  }
  let r = null;
  try {
    r = await api("POST", "/api/import/extract", { text, ...options }, { signal });
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
         data-testid="plan-drafts-modal"
         aria-labelledby="plan-drafts-title">
      <div class="modal-title" id="plan-drafts-title">Plan drafts</div>
      <div class="modal-body" data-testid="plan-drafts-body" style="max-height:72vh;overflow:auto">
        <div class="muted small" style="margin-bottom:8px">
          Review and edit drafted Gaps before saving.
        </div>
        <div id="import-drafts" class="import-drafts" data-testid="import-drafts"></div>
      </div>
      <div class="modal-actions">
        <button class="secondary" data-cancel data-testid="plan-drafts-cancel">Cancel</button>
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
    const drafts = await extractImportDrafts(text, draftsRoot, abort.signal, { purpose: "plan" });
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
