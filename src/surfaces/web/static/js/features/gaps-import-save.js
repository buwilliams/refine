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
    <button class="secondary" data-cancel data-testid="import-save-cancel">Cancel</button>
    <button class="secondary" data-hide data-testid="import-save-hide">Hide</button>
    <button id="btn-persist" data-testid="import-save-progress" disabled>Saving…</button>
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
    if (saveSession) saveSession({ phase: "saving", jobId, progress: job.progress || {} });
    drawImportSaving(root, readImportSession(), close, saveSession);
    await new Promise((resolve) => setTimeout(resolve, 750));
  }
}

async function waitForImportJobCancellation(jobId, root, close, saveSession = null) {
  while (true) {
    const snap = await api("GET", `/api/jobs/${jobId}`);
    const job = snap.job || {};
    if (job.status === "cancelled") return job;
    if (job.status === "complete") return job;
    if (job.status === "failed") {
      const err = new Error(job.error?.message || "Background job failed");
      err.details = job.error?.details;
      err.code = job.error?.code;
      throw err;
    }
    if (saveSession) {
      saveSession({
        phase: "saving",
        jobId,
        progress: { ...(job.progress || {}), message: "Cancelling" },
      });
    }
    drawImportSaving(root, readImportSession(), close, saveSession);
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
        error: failure.error || failure.message || "Could not save this Gap.",
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
