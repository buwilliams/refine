// ---- Gaps: detail -----------------------------------------------------------

// Gap detail is rendered as a modal layered over whatever screen the user
// was on (Dashboard, Gaps list, etc.) so that opening a Gap doesn't blow
// away context. `navigate()` handles the `#/gaps/<id>` route by calling
// `openGapDetailModal` directly — so `renderGapDetail` is no longer wired
// into the routes table. Kept as a thin wrapper in case any callers find
// it useful later.
async function renderGapDetail(r) {
  openGapDetailModal(r.id);
}

let _gapModalRoot = null;
let _gapRoundFormDraft = null;

function gapDetailContainer() {
  return _gapModalRoot?.querySelector(".gap-detail-modal-body") || null;
}

function openGapDetailModal(gapId) {
  // Make sure something is underneath. On a cold-load deep link (e.g. user
  // pastes `#/gaps/abc123` into a new tab), `#main` is empty — paint the
  // dashboard underneath so dismissing the modal leaves the user on a
  // sensible page.
  ensureGapModalUnderlay();

  if (_gapModalRoot) {
    // Modal is already open — swap the body to the new gap.
    const body = _gapModalRoot.querySelector(".gap-detail-modal-body");
    if (body) body.innerHTML = `<p class="muted">Loading…</p>`;
    loadGapDetail(gapId);
    return;
  }

  const root = document.createElement("div");
  root.className = "modal-backdrop gap-detail-backdrop";
  root.innerHTML = `
    <div class="modal gap-detail-modal" role="dialog" aria-modal="true"
         aria-label="Gap detail">
      <button class="modal-close" type="button" aria-label="Close">×</button>
      <div class="gap-detail-modal-body"><p class="muted">Loading…</p></div>
    </div>
  `;
  document.body.appendChild(root);
  _gapModalRoot = root;

  function onKey(e) {
    if (e.key === "Escape") { e.preventDefault(); dismiss(); }
  }
  function dismiss() {
    closeGapDetailModal({ navigateAway: true });
  }
  document.addEventListener("keydown", onKey, true);
  root._cleanup = () => document.removeEventListener("keydown", onKey, true);
  root.addEventListener("click", (e) => {
    if (e.target === root) dismiss();
  });
  root.querySelector(".modal-close").addEventListener("click", dismiss);

  loadGapDetail(gapId);
}

function closeGapDetailModal({ navigateAway = false } = {}) {
  if (!_gapModalRoot) return;
  _gapModalRoot._cleanup?.();
  _gapModalRoot.remove();
  _gapModalRoot = null;
  _gapRoundFormDraft = null;
  state.currentGap = null;
  state.currentGapData = null;
  if (navigateAway) {
    // Restore the URL to whatever was underneath. If we're already there
    // somehow (shouldn't happen), no-op so we don't trigger a redundant
    // re-render.
    const target = state.underlayHash || "#/";
    if (location.hash !== target) location.hash = target;
    else state.currentRoute = parseHash().route;
  }
}

function ensureGapModalUnderlay() {
  const main = $("#main");
  if (main && main.innerHTML.trim()) return;
  // Paint the dashboard underneath. We don't change `state.currentRoute`
  // here — the caller will set it to "gaps_detail" — but the dashboard's
  // render is keyed off DOM state, not route, so this works.
  renderDashboard();
}

async function loadGapDetail(gapId) {
  try {
    const { gap } = await api("GET", "/api/gaps/" + gapId);
    drawGapDetail(gap);
  } catch (e) {
    const container = gapDetailContainer();
    if (container) {
      container.innerHTML = `<p class="muted">Could not load gap: ${htmlEscape(e.message)}</p>`;
    }
  }
}

// User-driven workflow transitions for a Gap. Each state declares its
// `back` and `forward` neighbors. System-owned states have no user buttons —
// `in-progress` (dispatcher owns), `qa` (Quality owns), `ready-merge`
// (merger owns), and `awaiting-rebuild` (target-app rebuild owns) have no user buttons
// because they're system-driven phases the agent passes through
// automatically (todo → in-progress → qa → ready-merge → awaiting-rebuild → review).
// Forward from `review` goes through the dedicated /verify endpoint for
// approval. No user action moves a Gap into `review`; successful rebuild does.
//
// failed / cancelled only expose a back arrow — there's no obvious
// forward target for them (they're terminal-ish in opposite directions
// from done). Failed Gaps normally go back to todo and rerun; merge-stage
// failures use the latest workflow transition to requeue the existing branch.
const GAP_WORKFLOW = {
  backlog:      { forward: { label: "Todo →",     next: "todo"   } },
  todo:         { back:    { label: "← Backlog",  next: "backlog" } },
  // in-progress: no user buttons — dispatcher owns.
  // qa: no user buttons — Quality owns.
  // ready-merge: no user buttons — merger owns.
  // awaiting-rebuild: no user buttons — target-app rebuild owns.
  review:       { back:    { label: "← Todo",     next: "todo"   },
                  forward: { label: "Verify →",   next: "done", verify: true } },
  done:         { back:    { label: "← Review",   next: "review" } },
  failed:       { back:    { label: "← Todo",     next: "todo"   } },
  cancelled:    { back:    { label: "← Todo",     next: "todo"   } },
};

function workflowForGap(gap, latest) {
  if (gap.status === "failed" && isQualityRetryGap(latest)) {
    return { back: { label: "← QA", next: "qa", retryQuality: true } };
  }
  if (gap.status === "failed" && isMergeRetryGap(latest)) {
    return { back: { label: "← Merge", next: "ready-merge", retryMerge: true } };
  }
  return GAP_WORKFLOW[gap.status] || {};
}

function isQualityRetryGap(latest) {
  const message = latest?.latest_workflow_log?.message || "";
  return message.includes("Workflow status changed:") &&
         message.includes("qa") &&
         message.includes("failed");
}

function isMergeRetryGap(latest) {
  const message = latest?.latest_workflow_log?.message || "";
  return message.includes("Workflow status changed:") &&
         message.includes("ready-merge") &&
         message.includes("failed");
}

function drawGapDetail(gap) {
  if (!gap) return;
  if (_gapRoundFormDraft && _gapRoundFormDraft.gapId !== gap.id) {
    _gapRoundFormDraft = null;
  }
  const hadEditRoundForm = !!document.querySelector('#round-form[data-kind="edit"]');
  const roundFormDraft = captureRoundFormDraft(gap.id);
  if (roundFormDraft) {
    _gapRoundFormDraft = roundFormDraft;
  } else if (hadEditRoundForm && _gapRoundFormDraft?.gapId === gap.id) {
    _gapRoundFormDraft = null;
  }
  state.currentGapData = gap;
  renderBanners([]);
  // Preserve the notes-card open state across re-renders of the same gap so
  // saving a note (or an SSE-driven refresh) doesn't snap it shut.
  const notesOpen = document.querySelector(
    `.notes-card[data-gap-id="${gap.id}"]`,
  )?.open ?? false;
  // Same idea for the per-round wrapper. Metadata refreshes still redraw the
  // modal; preserve expanded sections so status or project updates do not
  // collapse the user's working context.
  const prevRoundOpen = {};
  document.querySelectorAll("details.round[data-round-idx]").forEach((el) => {
    prevRoundOpen[el.dataset.roundIdx] = el.open;
  });
  const rounds = gap.rounds || [];
  const latest = rounds[rounds.length - 1] || null;
  const failureBanner = computeFailureBanner(gap, latest);
  const governanceBanner = computeGovernanceBanner(gap, latest);
  const nodeDisplayName = gap.node_display_name || gap.node_id || "Unknown";
  const nodeOwnerTitle = gap.node_id
    ? `Node owner: ${nodeDisplayName} (${gap.node_id})`
    : `Node owner: ${nodeDisplayName}`;

  const isLatestEditable = (gap.status === "backlog" ||
                            gap.status === "todo" ||
                            gap.status === "failed");
  const hasPreservedDraft = hasPreservedRoundFormDraft(gap.id);
  const cancelEnabled = !["done", "cancelled"].includes(gap.status);
  // Chat is always available — the value is the Gap context the runner
  // primes into the provider session. The chat runs in the Gap's worktree
  // when one exists and falls back to the client repo when it doesn't.

  // Dynamic workflow buttons: each state shows the previous/next state
  // it can move to as back / forward buttons. The user-driven workflow
  // skips system-owned statuses. Forward from review goes through the existing
  // `verify` endpoint for approval; everything else is a bookkeeping status
  // update via PATCH /api/gaps/<id>.
  const workflow = workflowForGap(gap, latest);
  const backBtn = workflow.back ? `
    <button id="btn-state-back">${htmlEscape(workflow.back.label)}</button>
  ` : "";
  const forwardBtn = workflow.forward ? `
    <button id="btn-state-forward">${htmlEscape(workflow.forward.label)}</button>
  ` : "";

  const container = gapDetailContainer();
  if (!container) return;
  container.innerHTML = `
    <div class="gap-detail">
      <div class="row" style="align-items:center;margin-bottom:8px">
        <h2 style="margin:0">${htmlEscape(gap.name)}</h2>
        <span class="status-pill ${gap.status}">${workflowStatusLabel(gap.status)}</span>
        <span class="priority-pill priority-${gap.priority || "low"}">priority: ${gap.priority || "low"}</span>
        <span class="status-pill gap-node-owner" title="${htmlEscape(nodeOwnerTitle)}">node: ${htmlEscape(nodeDisplayName)}</span>
      </div>
      <div class="actions" style="margin-bottom:10px">
        ${backBtn}
        ${forwardBtn}
        <div class="gap-action-group">
          <button class="gap-action-primary" id="btn-chat">Open Chat</button>
          <details class="nav-menu gap-action-menu" id="gap-action-menu">
            <summary class="btn gap-action-more" aria-label="More Gap actions"></summary>
            <div class="nav-menu-panel gap-action-panel">
              <button class="nav-menu-item" type="button" id="btn-view-logs">View Logs</button>
              <button class="nav-menu-item" type="button" id="btn-reporter">Reporter</button>
              <button class="nav-menu-item" type="button" id="btn-rename">Rename</button>
              <button class="nav-menu-item" type="button" id="btn-priority">Change Priority</button>
              <button class="nav-menu-item" type="button" id="btn-cancel" ${cancelEnabled ? "" : "disabled"}>Cancel</button>
              <button class="nav-menu-item danger" type="button" id="btn-delete">Delete</button>
            </div>
          </details>
        </div>
      </div>
      <div class="muted small" style="margin-bottom:14px">
        ID <code>${gap.id}</code> · created ${fmtTime(gap.created)} · updated ${fmtTime(gap.updated)}
        ${gap.branch_name ? ` · branch <code>${gap.branch_name}</code>` : ""}
      </div>

      ${failureBanner ? `
        <div class="banner ${failureBanner.severity}">
          <span class="banner-msg">${htmlEscape(failureBanner.message)}</span>
          <span class="banner-actions">${failureBanner.actionsHtml}</span>
        </div>` : ""}
      ${governanceBanner ? `
        <div class="banner ${governanceBanner.severity}">
          <span class="banner-msg">${htmlEscape(governanceBanner.message)}</span>
        </div>` : ""}

      ${latest ? renderGovernanceSummary(latest) : ""}
      ${latest ? renderQualitySummary(latest) : ""}

      <h3>Rounds (${rounds.length})</h3>
      ${rounds.length === 0 ? `<p class="muted">No rounds yet.</p>` :
        rounds.map((rnd, idx) => renderRound(
          rnd, idx,
          idx === rounds.length - 1,
          prevRoundOpen,
        )).join("")}

      ${(isLatestEditable || hasPreservedDraft) ? `
        <div class="card" style="margin-top:14px">
          <h3>Edit latest round</h3>
          ${renderRoundForm("edit", latest, {
            draft: _gapRoundFormDraft,
            disabled: !isLatestEditable,
            formId: isLatestEditable ? "round-form" : "round-form-draft",
          })}
        </div>` : ""}

      ${gap.status === "review" ? `
        <div class="card" style="margin-top:14px">
          <h3>Submit follow-up round</h3>
          ${renderRoundForm("submit", null)}
        </div>` : ""}

      <details class="card notes-card" data-gap-id="${gap.id}" style="margin-top:14px" ${notesOpen ? "open" : ""}>
        <summary class="notes-card-summary">
          <span><strong>Notes (${(gap.notes || []).length})</strong></span>
          <span class="muted small">Saved to gap.json and included in attached
            Chat context.</span>
          <span class="spacer"></span>
          <span id="gap-notes-status" class="muted small"></span>
        </summary>
        <div style="margin-top:10px">
          <div id="notes-list">
            ${(gap.notes || []).length === 0
              ? `<p class="muted small">No notes yet.</p>`
              : (gap.notes || []).map(renderNote).join("")}
          </div>
          <details class="note-composer" style="margin-top:10px">
            <summary>+ Add a note</summary>
            <div class="form-row" style="margin-top:8px">
              <textarea id="new-note-body" rows="3"
                        placeholder="Anything the agent or team should know — links to specs, prior decisions, constraints, related code paths."></textarea>
            </div>
            <div class="actions">
              <button id="btn-add-note">Save note</button>
            </div>
          </details>
        </div>
      </details>
    </div>
  `;

  $("#btn-chat")?.addEventListener("click", () => {
    openChatDock({ gapId: gap.id });
  });
  $("#btn-view-logs")?.addEventListener("click", () => {
    closeGapActionMenu();
    location.hash = `#/logs?gap_id=${encodeURIComponent(gap.id)}`;
  });

  // Workflow back / forward buttons. Forward from `review` calls the
  // existing /verify endpoint for approval; every other arrow is a plain
  // status PATCH.
  const wireWorkflow = (btnId, target) => {
    if (!target) return;
    $(btnId)?.addEventListener("click", async () => {
      const btn = $(btnId);
      const busyLabel = target.verify
        ? "Verifying…"
        : target.retryMerge
          ? "Queueing merge…"
          : `Moving to ${target.next}…`;
      await withButtonBusy(btn, busyLabel, async () => {
        try {
          if (target.verify) {
            const r = await api("POST", `/api/gaps/${gap.id}/verify`);
            if (r.ok) toast(r.message || "Verified", "info");
            else toast(r.message || "Verify did not complete", "error");
          } else if (target.retryQuality) {
            const r = await api("POST", `/api/gaps/${gap.id}/retry-quality`);
            if (r.ok) toast(r.message || "Queued for QA", "info");
            else toast(r.message || "QA retry did not queue", "error");
          } else if (target.retryMerge) {
            const r = await api("POST", `/api/gaps/${gap.id}/retry-merge`);
            if (r.ok) toast(r.message || "Queued for merge", "info");
            else toast(r.message || "Merge retry did not queue", "error");
          } else {
            await api("PATCH", `/api/gaps/${gap.id}`, { status: target.next });
            toast(`Moved to ${target.next}`, "info");
          }
          await loadGapDetail(gap.id);
        } catch (e) { await showActionError(e); }
      });
    });
  };
  wireWorkflow("#btn-state-back", workflow.back);
  wireWorkflow("#btn-state-forward", workflow.forward);
  $("#btn-reporter")?.addEventListener("click", async () => {
    closeGapActionMenu();
    await openGapReporterModal(gap);
  });
  $("#btn-rename")?.addEventListener("click", async () => {
    closeGapActionMenu();
    const name = await modalPrompt("New name", gap.name,
                                   { title: "Rename Gap" });
    if (!name || !name.trim()) return;
    try {
      await api("PATCH", "/api/gaps/" + gap.id, { name: name.trim() });
      await loadGapDetail(gap.id);
    } catch (e) { await showActionError(e); }
  });
  $(".note-composer")?.addEventListener("toggle", (e) => {
    if (e.target.open) $("#new-note-body")?.focus();
  });
  $("#btn-add-note")?.addEventListener("click", async () => {
    const btn = $("#btn-add-note");
    const ta = $("#new-note-body");
    if (!ta) return;
    const body = (ta.value || "").trim();
    if (!body) return toast("Note can't be empty", "error");
    const author = state.lastReporter || "";
    const nextNotes = [...(gap.notes || []), { author, body }];
    await withButtonBusy(btn, "Saving…", async () => {
      try {
        await api("PATCH", "/api/gaps/" + gap.id, { notes: nextNotes });
        toast("Note added", "info");
        await loadGapDetail(gap.id);
      } catch (e) { await showActionError(e); }
    });
  });
  $$("[data-note-edit]").forEach((el) => el.addEventListener("click", async (e) => {
    e.preventDefault();
    const id = el.dataset.noteEdit;
    const existing = (gap.notes || []).find((n) => n.id === id);
    if (!existing) return;
    const body = await modalPrompt(
      "Edit note", existing.body,
      { title: "Edit note", okLabel: "Save" },
    );
    if (body === null) return;
    const trimmed = (body || "").trim();
    if (!trimmed) return toast("Note can't be empty", "error");
    const nextNotes = (gap.notes || []).map(
      (n) => n.id === id ? { ...n, body: trimmed } : n,
    );
    try {
      await api("PATCH", "/api/gaps/" + gap.id, { notes: nextNotes });
      toast("Note updated", "info");
      await loadGapDetail(gap.id);
    } catch (err) { await showActionError(err); }
  }));
  $$("[data-note-delete]").forEach((el) => el.addEventListener("click", async (e) => {
    e.preventDefault();
    const id = el.dataset.noteDelete;
    const ok = await modalConfirm(
      "Delete this note?",
      { title: "Delete note", okLabel: "Delete", danger: true },
    );
    if (!ok) return;
    const nextNotes = (gap.notes || []).filter((n) => n.id !== id);
    try {
      await api("PATCH", "/api/gaps/" + gap.id, { notes: nextNotes });
      toast("Note deleted", "info");
      await loadGapDetail(gap.id);
    } catch (err) { await showActionError(err); }
  }));
  $("#btn-priority")?.addEventListener("click", async () => {
    closeGapActionMenu();
    const current = gap.priority || "low";
    const body = () => `
      <div class="modal-title">Change priority</div>
      <div class="modal-body">
        <label for="modal-priority-select">Priority</label>
        <select class="modal-input" id="modal-priority-select" style="width:100%">
          ${["low", "medium", "high"].map((p) =>
            `<option value="${p}" ${p === current ? "selected" : ""}>${p}</option>`,
          ).join("")}
        </select>
      </div>
      <div class="modal-actions">
        <button class="secondary" data-cancel>Cancel</button>
        <button data-ok>Save</button>
      </div>`;
    const next = await _openModal(
      body, { cancel: null, ok: current }, ".modal-input",
    );
    if (next === null || next === current) return;
    try {
      await api("PATCH", "/api/gaps/" + gap.id, { priority: next });
      toast(`Priority set to ${next}`, "info");
      await loadGapDetail(gap.id);
    } catch (err) {
      await showActionError(err);
    }
  });
  $("#btn-cancel")?.addEventListener("click", async () => {
    closeGapActionMenu();
    const btn = $("#btn-cancel");
    if (btn.disabled) return;
    const ok = await modalConfirm(
      "Cancel this Gap? Any running subprocess will be stopped and the worktree + branch cleaned up.",
      { title: "Cancel Gap", okLabel: "Cancel Gap", danger: true,
        cancelLabel: "Keep working" },
    );
    if (!ok) return;
    await withButtonBusy(btn, "Cancelling…", async () => {
      try {
        await api("POST", `/api/gaps/${gap.id}/cancel`);
        toast("Cancelled", "info");
        await loadGapDetail(gap.id);
      } catch (e) { await showActionError(e); }
    });
  });
  $("#btn-delete")?.addEventListener("click", async () => {
    closeGapActionMenu();
    const ok = await modalConfirm(
      `Delete Gap "${gap.name}"? This cannot be undone.`,
      { title: "Delete Gap", okLabel: "Delete", danger: true },
    );
    if (!ok) return;
    try {
      await api("DELETE", "/api/gaps/" + gap.id);
      location.hash = "#/gaps";
    } catch (e) { await showActionError(e); }
  });

  bindFailureBannerActions(gap);
  bindRoundFormSubmit(gap);
  restoreRoundFormDraftFocus(gap.id);
}

function closeGapActionMenu() {
  const menu = $("#gap-action-menu");
  if (menu) menu.open = false;
}

async function openGapReporterModal(gap) {
  if (typeof refreshReporters === "function") {
    try {
      await refreshReporters();
    } catch {}
  }
  const latest = (gap.rounds || [])[Math.max(0, (gap.rounds || []).length - 1)] || {};
  const current = latest.reporter || "";
  const reporters = state.reporters || [];
  const options = reporters
    .map((r) => `<option value="${htmlEscape(r.name)}" ${r.name === current ? "selected" : ""}>${htmlEscape(r.name)}</option>`)
    .join("");
  const missingCurrent = current && !reporters.some((r) => r.name === current)
    ? `<option value="${htmlEscape(current)}" selected>${htmlEscape(current)}</option>`
    : "";
  const body = () => `
    <div class="modal-title">Change reporter</div>
    <div class="modal-body">
      <label for="modal-reporter-select">Reporter</label>
      <select class="modal-input" id="modal-reporter-select" style="width:100%">
        <option value="">— pick reporter —</option>
        ${missingCurrent}
        ${options}
        <option value="__add__">+ Add new reporter…</option>
      </select>
      <p class="muted small" style="margin-top:6px">
        Rewrites the latest round's reporter. Earlier rounds keep their original reporter.
      </p>
    </div>
    <div class="modal-actions">
      <button class="secondary" data-cancel>Cancel</button>
      <button data-ok>Save</button>
    </div>`;
  let next = await _openModal(body, { cancel: null, ok: current }, ".modal-input");
  if (next === null) return;
  if (next === "__add__") {
    const name = await modalPrompt("Name for the new reporter:", "", { title: "Add reporter" });
    next = (name || "").trim();
    if (!next) return;
    try {
      await api("POST", "/api/reporters", { name: next });
      await refreshReporters();
    } catch (e) {
      await showActionError(e, "Could not add reporter");
      return;
    }
  }
  next = (next || "").trim();
  if (!next || next === current) return;
  try {
    let r = await api("POST", "/api/gaps/bulk", {
      selected_ids: [gap.id],
      update: { reporter: next },
      background: false,
    });
    r = await resolveBackgroundJobResponse(r, "Reporter update is running in the background");
    if (r.updated) toast(`Reporter set to ${next}`, "info");
    else toast("Reporter was not changed", "warn");
    await loadGapDetail(gap.id);
  } catch (e) {
    await showActionError(e, "Reporter update failed");
  }
}

function captureRoundFormDraft(gapId) {
  const form = document.querySelector('#round-form[data-kind="edit"]');
  if (!form || state.currentGapData?.id !== gapId) return null;
  const actualEl = form.elements.actual;
  const targetEl = form.elements.target;
  if (!actualEl || !targetEl) return null;
  const rounds = state.currentGapData.rounds || [];
  const latest = rounds[rounds.length - 1] || {};
  const actual = actualEl.value || "";
  const target = targetEl.value || "";
  const dirty = actual !== (latest.actual || "") || target !== (latest.target || "");
  if (!dirty) return null;
  const activeEl = form.contains(document.activeElement) ? document.activeElement : null;
  const activeName = activeEl?.name || "";
  return {
    gapId,
    actual,
    target,
    activeName,
    selectionStart: typeof activeEl?.selectionStart === "number" ? activeEl.selectionStart : null,
    selectionEnd: typeof activeEl?.selectionEnd === "number" ? activeEl.selectionEnd : null,
  };
}

function hasPreservedRoundFormDraft(gapId) {
  return !!(_gapRoundFormDraft && _gapRoundFormDraft.gapId === gapId);
}

function restoreRoundFormDraftFocus(gapId) {
  const draft = _gapRoundFormDraft;
  if (!draft || draft.gapId !== gapId || !draft.activeName) return;
  const form = document.querySelector('#round-form[data-kind="edit"]');
  const el = form?.elements?.[draft.activeName];
  if (!el || el.readOnly || el.disabled) return;
  el.focus();
  if (typeof el.setSelectionRange === "function" &&
      draft.selectionStart !== null && draft.selectionEnd !== null) {
    el.setSelectionRange(draft.selectionStart, draft.selectionEnd);
  }
}

function renderRound(rnd, idx, isLatest, prevRoundOpen = {}) {
  // Preserve the user's open/closed choice across re-renders. New rounds
  // (no prior entry in the snapshot) default to "open on latest" — the
  // historical behavior.
  const key = String(idx);
  const roundOpen = key in prevRoundOpen ? prevRoundOpen[key] : isLatest;
  return `
    <details class="round" data-round-idx="${idx}" ${roundOpen ? "open" : ""}>
      <summary class="round-head">
        <strong>Round ${idx + 1}</strong>
        ${isLatest ? `<span class="status-pill review">latest</span>` : ""}
        ${isLatest && rnd.rule_state && rnd.rule_state !== "unclassified"
          ? `<span class="status-pill ${rnd.rule_state === "passed" ? "done" : "failed"}">governance: ${htmlEscape(rnd.rule_state)}</span>`
          : ""}
        ${isLatest && rnd.quality_state && rnd.quality_state !== "unclassified"
          ? `<span class="status-pill ${rnd.quality_state === "passed" ? "qa" : "failed"}">quality: ${htmlEscape(rnd.quality_state)}</span>`
          : ""}
        <span class="spacer"></span>
        <span class="muted small">${htmlEscape(rnd.reporter || "(no reporter)")} · ${fmtTime(rnd.created)}</span>
      </summary>
      <div class="round-body">
        <dl class="pair">
          <dt>actual</dt><dd>${htmlEscape(rnd.actual || "").replace(/\n/g, "<br>")}</dd>
          <dt>target</dt><dd>${htmlEscape(rnd.target || "").replace(/\n/g, "<br>")}</dd>
        </dl>
      </div>
    </details>
  `;
}

function renderGovernanceSummary(round) {
  if (!round || !round.rule_state || round.rule_state === "unclassified") {
    return "";
  }
  const actions = round.governance_rule_actions || [];
  return `
    <div class="card" style="margin:0 0 14px">
      <h3>Governance</h3>
      <div class="row" style="gap:8px;flex-wrap:wrap">
        <span class="status-pill ${round.rule_state === "passed" ? "done" : "failed"}">rules: ${htmlEscape(round.rule_state)}</span>
        <span class="status-pill ${round.product_state === "pass" ? "done" : "failed"}">product: ${htmlEscape(round.product_state || "unclassified")}</span>
        <span class="status-pill ${round.constitution_state === "pass" ? "done" : "failed"}">constitution: ${htmlEscape(round.constitution_state || "unclassified")}</span>
        <span class="status-pill todo">meta: ${htmlEscape(round.meta_rule_state || "none")}</span>
      </div>
      ${round.governance_message ? `<p style="margin-bottom:6px">${htmlEscape(round.governance_message)}</p>` : ""}
      ${round.governance_details ? `<details><summary>Details</summary><pre>${htmlEscape(round.governance_details)}</pre></details>` : ""}
      ${actions.length ? `
        <details style="margin-top:8px">
          <summary>Rule actions (${actions.length})</summary>
          ${actions.map((a) => `
            <div class="log-entry info">
              <div>${htmlEscape(a.action || "")}${a.text ? `: ${htmlEscape(a.text)}` : ""}</div>
              ${a.reason ? `<div class="meta">${htmlEscape(a.reason)}</div>` : ""}
            </div>`).join("")}
        </details>` : ""}
    </div>`;
}

function renderQualitySummary(round) {
  if (!round || !round.quality_state || round.quality_state === "unclassified") {
    return "";
  }
  return `
    <div class="card" style="margin:0 0 14px">
      <h3>Quality</h3>
      <div class="row" style="gap:8px;flex-wrap:wrap">
        <span class="status-pill ${round.quality_state === "passed" ? "qa" : "failed"}">quality: ${htmlEscape(round.quality_state)}</span>
        ${round.quality_checked_at ? `<span class="muted small">${fmtTime(round.quality_checked_at)}</span>` : ""}
      </div>
      ${round.quality_message ? `<p style="margin-bottom:6px">${htmlEscape(round.quality_message)}</p>` : ""}
      ${round.quality_details ? `<details><summary>Details</summary><pre>${htmlEscape(round.quality_details)}</pre></details>` : ""}
    </div>`;
}

function renderNote(n) {
  const firstLine = (n.body || "").split("\n", 1)[0];
  const preview = firstLine.length > 80
    ? firstLine.slice(0, 77) + "…"
    : firstLine;
  const meta = [n.author, n.created ? fmtTime(n.created) : ""].filter(Boolean).join(" · ");
  return `
    <details class="note">
      <summary>
        <span class="note-preview">${htmlEscape(preview || "(empty)")}</span>
        ${meta ? `<span class="muted small note-meta">${htmlEscape(meta)}</span>` : ""}
      </summary>
      <div class="note-body">${htmlEscape(n.body || "").replace(/\n/g, "<br>")}</div>
      <div class="actions" style="margin-top:6px">
        <button class="secondary" data-note-edit="${htmlEscape(n.id)}">Edit</button>
        <button class="danger" data-note-delete="${htmlEscape(n.id)}">Delete</button>
      </div>
    </details>`;
}

function renderRoundForm(
  kind,
  prefill,
  { draft = null, disabled = false, formId = "round-form" } = {},
) {
  const actual = draft?.actual ?? prefill?.actual ?? "";
  const target = draft?.target ?? prefill?.target ?? "";
  const reporter = state.lastReporter || "";
  if (!reporter) return renderPickReporterNotice();
  const submitLabel = kind === "submit" ? "Submit new round" : "Save changes";
  const readonly = disabled ? "readonly" : "";
  const buttonDisabled = disabled ? "disabled" : "";
  return `
    <form id="${htmlEscape(formId)}" data-kind="${kind}">
      <div class="muted small" style="margin-bottom:8px">
        Submitting as <strong class="js-reporter-name">${htmlEscape(reporter)}</strong>
        — change in the top-right reporter selector.
      </div>
      ${disabled ? `
        <p class="muted small">
          This Gap is no longer editable. Unsaved text is preserved here so you can copy it.
        </p>` : ""}
      <div class="form-row">
        <label>Actual (current behavior)</label>
        <textarea name="actual" placeholder="What's happening today?" ${readonly}>${htmlEscape(actual)}</textarea>
      </div>
      <div class="form-row">
        <label>Target (desired behavior)</label>
        <textarea name="target" placeholder="What should be happening?" ${readonly}>${htmlEscape(target)}</textarea>
      </div>
      <div class="actions">
        <button type="submit" ${buttonDisabled}>${submitLabel}</button>
      </div>
    </form>
  `;
}

function renderPickReporterNotice() {
  return `
    <p class="muted">
      Pick a reporter in the top-right selector to enable this form.
    </p>
  `;
}

function bindRoundFormSubmit(gap) {
  const form = $("#round-form");
  if (!form) return;
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const reporter = state.lastReporter || "";
    if (!reporter) return toast("Pick a reporter in the top-right selector", "error");
    const fd = new FormData(form);
    const actual = (fd.get("actual") || "").toString().trim();
    const target = (fd.get("target") || "").toString().trim();
    if (!actual && !target) return toast("Provide actual or target", "error");
    const kind = form.dataset.kind;
    try {
      if (kind === "submit") {
        await api("POST", `/api/gaps/${gap.id}/rounds`, { reporter, actual, target });
        toast("New round submitted", "info");
      } else {
        await api("PATCH", `/api/gaps/${gap.id}/rounds/latest`, { reporter, actual, target });
        toast("Round updated", "info");
      }
      _gapRoundFormDraft = null;
      await loadGapDetail(gap.id);
    } catch (err) {
      await showActionError(err);
    }
  });
}

function computeFailureBanner(gap, latest) {
  if (gap.status === "failed") {
    const lastLog = latest?.latest_log;
    const errLog = latest?.latest_error_log;
    const workflowLog = latest?.latest_workflow_log;
    const fallbackLog = lastLog?.severity && lastLog.severity !== "info" ? lastLog : null;
    return {
      severity: "error",
      message: errLog?.message || workflowLog?.message || fallbackLog?.message || "Gap failed",
      actionsHtml: "",
    };
  }
  if (gap.status === "review") {
    // Was there a recent error? Then we treat it as a stuck-review state.
    const errLog = latest?.latest_error_log;
    if (errLog) {
      return {
        severity: "error",
        message: errLog.message || "Review needs attention",
        actionsHtml: "",
      };
    }
  }
  return null;
}

function computeGovernanceBanner(gap, latest) {
  if (!latest || !latest.rule_state || latest.rule_state === "unclassified") {
    return null;
  }
  const passed = latest.rule_state === "passed"
    && latest.product_state === "pass"
    && latest.constitution_state === "pass";
  if (passed) return null;
  return {
    severity: gap.status === "backlog" ? "warn" : "error",
    message: latest.governance_message || "Governance review requires changes before implementation.",
  };
}

function bindFailureBannerActions(_gap) {
  // No banner-level actions: Verify / Open Chat / Reopen / Rename / Cancel /
  // Delete all live in the unified action menu at the top of the page.
}
