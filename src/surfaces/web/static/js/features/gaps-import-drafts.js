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
  const currentSessionPhase = () => (
    options.retry && draftState.some((draft) => draft.error)
      ? "failed"
      : "review"
  );

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
      if (saveSession) saveSession({ phase: currentSessionPhase(), drafts: draftState, featureDestination });
      updateImportPersistButton(root, draftState, featureDestination);
    });
    bindImportDraftPage(drafts_root, draftState, saveSession, {
      featureDestination: () => featureDestination,
      phase: currentSessionPhase,
      onSelectionChange: renderPage,
    });
    const persistReviewState = () => {
      syncImportDraftPage(drafts_root, draftState);
      if (saveSession) saveSession({ phase: currentSessionPhase(), drafts: draftState, featureDestination });
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
      if (saveSession) saveSession({ phase: currentSessionPhase(), drafts: draftState, featureDestination });
      renderPage();
    });
    $("[data-import-toggle-all]", drafts_root)?.addEventListener("click", () => {
      persistReviewState();
      visibleDrafts.forEach(({ draft }) => {
        draft.selected = !allFilteredSelected;
      });
      if (saveSession) saveSession({ phase: currentSessionPhase(), drafts: draftState, featureDestination });
      renderPage();
    });
    $("[data-import-select-duplicates]", drafts_root)?.addEventListener("click", () => {
      persistReviewState();
      reviewDrafts.forEach(({ draft }) => {
        draft.selected = !!draft.duplicate;
      });
      if (saveSession) saveSession({ phase: currentSessionPhase(), drafts: draftState, featureDestination });
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
      if (saveSession) saveSession({ phase: currentSessionPhase(), drafts: draftState, featureDestination });
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
      if (saveSession) saveSession({ phase: currentSessionPhase(), drafts: draftState, featureDestination });
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
      if (saveSession) saveSession({ phase: currentSessionPhase(), drafts: draftState, featureDestination });
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
      if (saveSession) saveSession({ phase: currentSessionPhase(), drafts: draftState, featureDestination });
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
        if (saveSession) saveSession({ phase: currentSessionPhase(), drafts: draftState, featureDestination });
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
    <button class="secondary" data-cancel data-testid="import-cancel">Cancel</button>
    <button id="btn-persist" data-testid="import-persist"></button>
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
    if (saveSession) saveSession({ phase: currentSessionPhase(), drafts: draftState, featureDestination });
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
    if (saveSession) saveSession({ phase: currentSessionPhase(), drafts: draftState, featureDestination });
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
        if (r.operation) {
          if (saveSession) saveSession({ phase: "saving", drafts: draftState, featureDestination, operationId: r.operation.id, result: null, error: "" });
          drawImportSaving(root, readImportSession(), close, saveSession);
          r = await waitForImportPersistOperation(r.operation.id, root, close, saveSession);
        } else {
          if (saveSession) saveSession({ phase: "saving", drafts: draftState, featureDestination, operationId: "", result: null, error: "" });
        }
        await handleImportPersistResult(root, r, payload, skipped, close, saveSession, {
          clearSession: clearSessionOnClose,
        });
      } catch (e) {
        if (e.code === "operation_cancelled") {
          if (clearSessionOnClose) clearImportSession();
          close(true, { force: true });
          return;
        }
        if (e.name === "AbortError") return;
        await showActionError(e, "Import failed");
      }
    });
  });
}
