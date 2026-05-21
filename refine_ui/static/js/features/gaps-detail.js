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
const GAP_ROUND_LOG_LIMIT = 100;
const _gapRoundLogCache = new Map();

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

function roundLogCacheKey(gapId, roundIdx) {
  return `${gapId}:${roundIdx}`;
}

function roundLogPageState(gapId, roundIdx) {
  return _gapRoundLogCache.get(roundLogCacheKey(gapId, roundIdx)) || null;
}

function invalidateGapRoundLogCache(gapId) {
  for (const key of Array.from(_gapRoundLogCache.keys())) {
    if (key.startsWith(`${gapId}:`)) _gapRoundLogCache.delete(key);
  }
}

async function loadRoundLogsPage(gapId, roundIdx, page = 1) {
  const offset = (Math.max(1, page) - 1) * GAP_ROUND_LOG_LIMIT;
  const key = roundLogCacheKey(gapId, roundIdx);
  _gapRoundLogCache.set(key, {
    ...(roundLogPageState(gapId, roundIdx) || {}),
    loading: true,
    error: "",
    page: { limit: GAP_ROUND_LOG_LIMIT, offset, has_more: false },
  });
  drawGapDetail(state.currentGapData);
  try {
    const data = await api(
      "GET",
      `/api/gaps/${gapId}/rounds/${roundIdx}/logs?limit=${GAP_ROUND_LOG_LIMIT}&offset=${offset}`,
    );
    _gapRoundLogCache.set(key, {
      logs: data.logs || [],
      page: data.page || { limit: GAP_ROUND_LOG_LIMIT, offset, has_more: false },
      loading: false,
      error: "",
    });
  } catch (e) {
    _gapRoundLogCache.set(key, {
      logs: [],
      page: { limit: GAP_ROUND_LOG_LIMIT, offset, has_more: false },
      loading: false,
      error: e.message || "Could not load logs",
    });
  }
  if (state.currentRoute === "gaps_detail" && state.currentGap === gapId) {
    await loadGapDetail(gapId);
  }
}

// User-driven workflow transitions for a Gap. Each state declares its
// `back` and `forward` neighbors. System-owned states have no user buttons —
// `in-progress` (dispatcher owns), `ready-merge` (merger owns), and
// `awaiting-rebuild` (target-app rebuild owns) have no user buttons
// because they're system-driven phases the agent passes through
// automatically (todo → in-progress → ready-merge → awaiting-rebuild → review).
// Forward from `review` goes through the dedicated /verify endpoint for
// approval. No user action moves a Gap into `review`; successful rebuild does.
//
// failed / cancelled only expose a back arrow — there's no obvious
// forward target for them (they're terminal-ish in opposite directions
// from done). Use back to send the Gap back to todo and rerun.
const GAP_WORKFLOW = {
  backlog:      { forward: { label: "Todo →",     next: "todo"   } },
  todo:         { back:    { label: "← Backlog",  next: "backlog" } },
  // in-progress: no user buttons — dispatcher owns.
  // ready-merge: no user buttons — merger owns.
  // awaiting-rebuild: no user buttons — target-app rebuild owns.
  review:       { back:    { label: "← Todo",     next: "todo"   },
                  forward: { label: "Verify →",   next: "done", verify: true } },
  done:         { back:    { label: "← Review",   next: "review" } },
  failed:       { back:    { label: "← Todo",     next: "todo"   } },
  cancelled:    { back:    { label: "← Todo",     next: "todo"   } },
};

function drawGapDetail(gap) {
  if (!gap) return;
  state.currentGapData = gap;
  renderBanners([]);
  // Preserve the notes-card open state across re-renders of the same gap so
  // saving a note (or an SSE-driven refresh) doesn't snap it shut.
  const notesOpen = document.querySelector(
    `.notes-card[data-gap-id="${gap.id}"]`,
  )?.open ?? false;
  // Same idea for the per-round wrapper and its inner Logs disclosure.
  // SSE round_log_added events trigger a full drawGapDetail re-render
  // every time the agent emits a new event; without this, an expanded
  // round or expanded Logs collapses each time a new log arrives.
  const prevRoundOpen = {};
  const prevLogsOpen = {};
  document.querySelectorAll("details.round[data-round-idx]").forEach((el) => {
    prevRoundOpen[el.dataset.roundIdx] = el.open;
  });
  document.querySelectorAll('details[data-role="round-logs"][data-round-idx]').forEach((el) => {
    prevLogsOpen[el.dataset.roundIdx] = el.open;
  });
  const rounds = gap.rounds || [];
  // Merge gap-scoped activity into each round so users see lifecycle events
  // and runner errors alongside the round's own paged logs. Each activity entry
  // goes into the latest round whose `created` is at or before the entry's
  // datetime.
  attachActivityToRounds(rounds, gap.activity || [], gap.id);
  const latest = rounds[rounds.length - 1] || null;
  const failureBanner = computeFailureBanner(gap, latest);
  const governanceBanner = computeGovernanceBanner(gap, latest);

  const isLatestEditable = (gap.status === "backlog" ||
                            gap.status === "todo" ||
                            gap.status === "failed");
  const cancelEnabled = !["done", "cancelled"].includes(gap.status);
  // Chat is always available — the value is the Gap context the runner
  // primes into the provider session. The chat runs in the Gap's worktree
  // when one exists and falls back to the client repo when it doesn't.

  // Dynamic workflow buttons: each state shows the previous/next state
  // it can move to as back / forward buttons. The user-driven workflow
  // skips system-owned statuses. Forward from review goes through the existing
  // `verify` endpoint for approval; everything else is a bookkeeping status
  // update via PATCH /api/gaps/<id>.
  const workflow = GAP_WORKFLOW[gap.status] || {};
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
        <span class="status-pill ${gap.status}">${gap.status}</span>
        <span class="priority-pill priority-${gap.priority || "low"}">priority: ${gap.priority || "low"}</span>
      </div>
      <div class="actions" style="margin-bottom:10px">
        ${backBtn}
        ${forwardBtn}
        <button id="btn-chat" ${featureEnabled("chat") ? "" : "disabled title=\"Chat is disabled for the current AI provider — see System → Runtime\""}>Open Chat</button>
        <button class="warn" id="btn-rename">Rename</button>
        <button class="warn" id="btn-priority">Change Priority</button>
        <button class="warn" id="btn-cancel" ${cancelEnabled ? "" : "disabled"}>Cancel Gap</button>
        <button class="danger" id="btn-delete">Delete</button>
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

      <h3>Rounds (${rounds.length})</h3>
      ${rounds.length === 0 ? `<p class="muted">No rounds yet.</p>` :
        rounds.map((rnd, idx) => renderRound(
          rnd, idx,
          idx === rounds.length - 1,
          isLatestEditable && idx === rounds.length - 1,
          prevRoundOpen, prevLogsOpen, gap.id,
        )).join("")}

      ${(gap.status === "backlog" || gap.status === "todo" || gap.status === "failed") ? `
        <div class="card" style="margin-top:14px">
          <h3>Edit latest round</h3>
          ${renderRoundForm("edit", latest)}
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

  // Workflow back / forward buttons. Forward from `review` calls the
  // existing /verify endpoint for approval; every other arrow is a plain
  // status PATCH.
  const wireWorkflow = (btnId, target) => {
    if (!target) return;
    $(btnId)?.addEventListener("click", async () => {
      const btn = $(btnId);
      const busyLabel = target.verify ? "Verifying…" : `Moving to ${target.next}…`;
      await withButtonBusy(btn, busyLabel, async () => {
        try {
          if (target.verify) {
            const r = await api("POST", `/api/gaps/${gap.id}/verify`);
            if (r.ok) toast(r.message || "Verified", "info");
            else toast(r.message || "Verify did not complete", "error");
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
  $("#btn-rename")?.addEventListener("click", async () => {
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
  bindRoundLogControls(gap);
}

function bindRoundLogControls(gap) {
  document.querySelectorAll('details[data-role="round-logs"][data-round-idx]').forEach((el) => {
    const roundIdx = parseInt(el.dataset.roundIdx, 10);
    if (!Number.isFinite(roundIdx)) return;
    el.addEventListener("toggle", () => {
      if (!el.open) return;
      if (roundLogPageState(gap.id, roundIdx)) return;
      loadRoundLogsPage(gap.id, roundIdx, 1);
    });
    bindPaginationControls(el, `round-${roundIdx}-logs`, (page) => {
      loadRoundLogsPage(gap.id, roundIdx, page);
    });
    if (el.open && !roundLogPageState(gap.id, roundIdx)) {
      loadRoundLogsPage(gap.id, roundIdx, 1);
    }
  });
}

function attachActivityToRounds(rounds, activity, gapId = "") {
  // Reset any prior merge — we always recompute from cached round logs plus
  // gap activity.
  rounds.forEach((r, idx) => {
    const cached = roundLogPageState(gapId, idx);
    r._roundLogsPage = cached;
    r._mergedLogs = (cached?.logs || r.logs || []).slice();
    r._activityLogCount = 0;
  });
  if (!rounds.length) return;
  // Sort rounds ascending by `created`; bucket each activity entry into the
  // last round whose `created` ≤ entry.datetime.
  const bucket = (ts) => {
    let idx = 0;
    for (let i = 0; i < rounds.length; i++) {
      if ((rounds[i].created || "") <= ts) idx = i;
      else break;
    }
    return idx;
  };
  for (const a of activity) {
    const idx = bucket(a.datetime || "");
    rounds[idx]._mergedLogs.push(a);
    rounds[idx]._activityLogCount = (rounds[idx]._activityLogCount || 0) + 1;
  }
  // Sort each round's merged logs by datetime ascending.
  for (const r of rounds) {
    r._mergedLogs.sort((x, y) => (x.datetime || "").localeCompare(y.datetime || ""));
  }
}

function renderRound(rnd, idx, isLatest, editable,
                     prevRoundOpen = {}, prevLogsOpen = {}, gapId = "") {
  const logs = rnd._mergedLogs || rnd.logs || [];
  const pageState = rnd._roundLogsPage || null;
  const roundLogCount = parseInt(rnd.log_count || 0, 10) || 0;
  const activityCount = parseInt(rnd._activityLogCount || 0, 10) || 0;
  const displayLogCount = roundLogCount + activityCount;
  // Preserve the user's open/closed choice across re-renders. New rounds
  // (no prior entry in the snapshot) default to "open on latest" — the
  // historical behavior — and Logs default closed.
  const key = String(idx);
  const roundOpen = key in prevRoundOpen ? prevRoundOpen[key] : isLatest;
  const logsOpen = key in prevLogsOpen ? prevLogsOpen[key] : false;
  return `
    <details class="round" data-round-idx="${idx}" ${roundOpen ? "open" : ""}>
      <summary class="round-head">
        <strong>Round ${idx + 1}</strong>
        ${isLatest ? `<span class="status-pill review">latest</span>` : ""}
        ${isLatest && rnd.rule_state && rnd.rule_state !== "unclassified"
          ? `<span class="status-pill ${rnd.rule_state === "passed" ? "done" : "failed"}">governance: ${htmlEscape(rnd.rule_state)}</span>`
          : ""}
        <span class="spacer"></span>
        <span class="muted small">${htmlEscape(rnd.reporter || "(no reporter)")} · ${fmtTime(rnd.created)}</span>
      </summary>
      <div class="round-body">
        <dl class="pair">
          <dt>actual</dt><dd>${htmlEscape(rnd.actual || "").replace(/\n/g, "<br>")}</dd>
          <dt>target</dt><dd>${htmlEscape(rnd.target || "").replace(/\n/g, "<br>")}</dd>
        </dl>
        ${displayLogCount || logs.length ? `
          <details data-role="round-logs" data-gap-id="${htmlEscape(gapId)}"
                   data-round-idx="${idx}" ${logsOpen ? "open" : ""}>
            <summary>Logs (${displayLogCount || logs.length})</summary>
            ${renderRoundLogsBody(logs, pageState, idx)}
          </details>` : `<p class="muted small">No logs.</p>`}
      </div>
    </details>
  `;
}

function renderRoundLogsBody(logs, pageState, roundIdx) {
  if (pageState?.loading) {
    return `<p class="muted small">Loading…</p>`;
  }
  if (pageState?.error) {
    return `<p class="muted small">${htmlEscape(pageState.error)}</p>`;
  }
  const pageControls = pageState?.page
    ? renderPaginationControls(
      `round-${roundIdx}-logs`,
      pageState.page,
      pageState.logs?.length || 0,
      "entry",
    )
    : "";
  if (!logs.length) {
    return `<p class="muted small">Open to load logs.</p>${pageControls}`;
  }
  return `${logs.map((l) => renderLogEntry(l)).join("")}${pageControls}`;
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

function renderLogEntry(l) {
  return `
    <div class="log-entry ${l.severity || 'info'}">
      <div>${htmlEscape(l.message)}</div>
      <div class="meta">${fmtTime(l.datetime)} · ${htmlEscape(l.category || '')}${l.actor ? ' · ' + htmlEscape(l.actor) : ''}</div>
      ${l.details ? `<details><summary class="diff-show-details">Show details</summary><pre>${htmlEscape(l.details)}</pre></details>` : ''}
    </div>`;
}

function renderRoundForm(kind, prefill) {
  const actual = prefill?.actual || "";
  const target = prefill?.target || "";
  const reporter = state.lastReporter || "";
  if (!reporter) return renderPickReporterNotice();
  const submitLabel = kind === "submit" ? "Submit new round" : "Save changes";
  return `
    <form id="round-form" data-kind="${kind}">
      <div class="muted small" style="margin-bottom:8px">
        Submitting as <strong class="js-reporter-name">${htmlEscape(reporter)}</strong>
        — change in the top-right reporter selector.
      </div>
      <div class="form-row">
        <label>Actual (current behavior)</label>
        <textarea name="actual" placeholder="What's happening today?">${htmlEscape(actual)}</textarea>
      </div>
      <div class="form-row">
        <label>Target (desired behavior)</label>
        <textarea name="target" placeholder="What should be happening?">${htmlEscape(target)}</textarea>
      </div>
      <div class="actions">
        <button type="submit">${submitLabel}</button>
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
      await loadGapDetail(gap.id);
    } catch (err) {
      await showActionError(err);
    }
  });
}

function computeFailureBanner(gap, latest) {
  if (gap.status === "failed") {
    const lastLog = latest?.latest_log || (latest?.logs || []).slice(-1)[0];
    return {
      severity: "error",
      message: lastLog?.message || "Agent run failed",
      actionsHtml: "",
    };
  }
  if (gap.status === "review") {
    // Was the last log an error? Then we treat it as a stuck-review state.
    const errLog = latest?.latest_error_log
      || (latest?.logs || []).slice().reverse().find((l) => l.severity === "error");
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
