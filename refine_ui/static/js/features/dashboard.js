// ---- Dashboard --------------------------------------------------------------

async function renderDashboard() {
  // First paint only: lay out the outer chrome and a `Loading…`
  // placeholder. SSE-triggered refreshes route through `refreshDashboard`
  // below so the screen doesn't flicker back to `Loading…` between
  // events. If `#dash` is already in the DOM (e.g. the route handler
  // was re-invoked by hashchange while the screen is already up), skip
  // the wipe — drawing the new data over the old DOM is silent.
  if (!document.getElementById("dash")) {
    $("#main").innerHTML = `<h2>Dashboard</h2><div id="dash"><p class="muted">Loading…</p></div>`;
  }
  await refreshDashboard();
}

async function refreshDashboard() {
  // Silent refresh — fetch + redraw in place, no `Loading…` flash. Used
  // by both the route handler (after the first-paint scaffold above) and
  // every SSE handler that wants the dashboard to track live state.
  if (state.currentRoute !== "dashboard") return;
  try {
    const reporter = state.lastReporter || "";
    const [d, reviews] = await Promise.all([
      api("GET", "/api/dashboard"),
      reporter
        ? api("GET", "/api/gaps?status=review&reporter=" + encodeURIComponent(reporter) + "&limit=200")
        : Promise.resolve({ gaps: [] }),
    ]);
    state.dashboard = d;
    drawDashboard(d, { reviewsForReporter: reviews.gaps || [], reporter });
  } catch (e) {
    const dash = document.getElementById("dash");
    if (dash) dash.innerHTML = `<p class="muted">Failed to load: ${htmlEscape(e.message)}</p>`;
  }
}

function drawDashboard(d, opts = {}) {
  const reviewsForReporter = opts.reviewsForReporter || [];
  const reviewReporter = opts.reporter || "";
  // Global banners
  const banners = (d.needs_attention || []).filter((x) => x.kind === "banner")
    .map((x) => ({
      severity: x.severity || "error",
      message: x.message,
      action: /Refine cannot reach/i.test(x.message) ? {
        label: "Re-check auth",
        onClick: async () => {
          try {
            await api("POST", "/api/settings/recheck-auth");
            toast("Pre-flight re-run requested", "info");
            await refreshDashboard();
          } catch (e) {
            toast(e.message, "error");
          }
        },
      } : null,
    }));
  renderBanners(banners);

  const needsAttention = (d.needs_attention || []).filter((x) => x.kind === "filter");
  const counts = d.counts || {};
  const orderedStatuses = ["backlog", "todo", "in-progress", "ready-merge", "review", "done", "failed", "cancelled"];
  const dash = $("#dash");
  // Guard against late-arriving SSE refreshes after the user navigated
  // away — the container is gone, so just bail silently.
  if (!dash) return;
  dash.innerHTML = `
    ${needsAttention.length ? `
      <section class="card">
        <h3>Needs attention</h3>
        <div class="actions">
          ${needsAttention.map((x) => `
            <a href="#/gaps?status=${encodeURIComponent(x.filter?.status || "")}" class="btn">
              ${htmlEscape(x.message)}
            </a>`).join("")}
        </div>
      </section>` : ""}

    <section class="card-grid">
      ${orderedStatuses.map((s) => `
        <a class="card" href="#/gaps?status=${s}" style="text-decoration:none;color:inherit"
           title="${counts[s] || 0} ${s} gap${(counts[s] || 0) === 1 ? "" : "s"}">
          <div class="muted small">${s}</div>
          <div style="font-size:28px;font-weight:600;margin-top:4px">${fmtCount(counts[s] || 0)}</div>
        </a>`).join("")}
    </section>

    <section class="card">
      <h3>Reporter stats</h3>
      ${(d.reporter_stats || []).length === 0
        ? `<p class="muted">No reporter activity yet.</p>`
        : `<table class="table">
            <thead><tr>
              <th>Reporter</th>
              <th>Active</th>
              <th>Done</th>
              <th>Reported</th>
              <th>Done / Reported</th>
            </tr></thead>
            <tbody>
              ${d.reporter_stats.map((s) => `
                <tr class="reporter-stats-row"
                    data-reporter="${htmlEscape(s.reporter)}"
                    title="See Gaps reported by ${htmlEscape(s.reporter)}">
                  <td>${htmlEscape(s.reporter)}</td>
                  <td>${fmtCount(s.active)}</td>
                  <td>${fmtCount(s.done)}</td>
                  <td>${fmtCount(s.reported)}</td>
                  <td>${s.completion_rate.toFixed(1)}%</td>
                </tr>`).join("")}
            </tbody>
          </table>`}
    </section>

    ${reviewReporter ? `
    <section class="card" id="reviews-for-reporter-card">
      <div class="card-head-row">
        <h3>
          Awaiting your review
          <span class="muted small">— ${htmlEscape(reviewReporter)}</span>
        </h3>
        ${reviewsForReporter.length === 0 ? "" : `
          <button id="rev-bulk-verify" disabled>Verify selected</button>`}
      </div>
      ${reviewsForReporter.length === 0
        ? `<p class="muted">Nothing in <code>review</code> assigned to you right now.</p>`
        : `<table class="table">
            <thead><tr>
              <th class="gap-select-col">
                <input type="checkbox" id="rev-select-all"
                       aria-label="Select all reviews">
              </th>
              <th>Gap</th>
              <th>Updated</th>
              <th class="actions-col" style="white-space:nowrap"></th>
            </tr></thead>
            <tbody>
              ${reviewsForReporter.map((g) => `
                <tr data-rev-row="${g.id}">
                  <td class="gap-select-col"><input type="checkbox" class="rev-row-check" data-rev-id="${g.id}"></td>
                  <td>
                    <a href="#/gaps/${g.id}" title="${htmlEscape(g.id)}">
                      ${htmlEscape(g.name)}
                    </a>
                  </td>
                  <td class="muted small">${fmtTime(g.updated)}</td>
                  <td class="actions" style="white-space:nowrap">
                    <button data-rev-verify="${g.id}">Verify →</button>
                    <button class="secondary" data-rev-add-round="${g.id}"
                            data-rev-name="${htmlEscape(g.name)}">Add round</button>
                  </td>
                </tr>`).join("")}
            </tbody>
          </table>`}
    </section>` : ""}

    <section class="card">
      <h3>Currently running</h3>
      <div class="card-scroll">
        ${(d.running || []).length === 0
          ? `<p class="muted">No agent subprocesses running.</p>`
          : (() => {
              // Anchor seconds-at-fetch + a single `data-anchor-ms`
              // timestamp so the tick can compute a live value
              // without re-fetching: shown = base + (now - anchor) / 1000.
              const anchorMs = Date.now();
              return `<table class="table"><thead><tr><th>Gap</th><th>Elapsed</th><th>Idle</th><th>PID</th></tr></thead><tbody>
                ${d.running.map((r) => `<tr onclick="location.hash='#/gaps/${r.gap_id}'">
                  <td><code>${r.gap_id.slice(0,8)}…</code></td>
                  <td class="js-elapsed-tick"
                      data-base="${r.elapsed_seconds}"
                      data-anchor-ms="${anchorMs}">${fmtElapsed(r.elapsed_seconds)}</td>
                  <td class="js-idle-tick"
                      data-base="${r.idle_seconds}"
                      data-anchor-ms="${anchorMs}">${fmtElapsed(r.idle_seconds)}</td>
                  <td class="muted small">${r.pid}</td>
                </tr>`).join("")}
                </tbody></table>`;
            })()}
      </div>
    </section>
  `;
  // Click any reporter row → deep-link into the Gaps list filtered by
  // that reporter. We use data-reporter + a delegated listener so the
  // name can contain spaces/quotes without HTML-escaping hazards.
  $$(".reporter-stats-row").forEach((row) => {
    row.addEventListener("click", () => {
      location.hash = gapsHash({ reporter: row.dataset.reporter });
    });
  });

  wireReviewsForReporter(reviewsForReporter);
}

function wireReviewsForReporter(reviews) {
  if (!reviews || !reviews.length) return;
  const card = document.getElementById("reviews-for-reporter-card");
  if (!card) return;
  const checks = () => $$(".rev-row-check", card);
  const selected = () => checks().filter((c) => c.checked).map((c) => c.dataset.revId);
  const syncBulkButton = () => {
    const btn = $("#rev-bulk-verify", card);
    if (!btn) return;
    const n = selected().length;
    btn.disabled = n === 0;
    btn.textContent = n === 0 ? "Verify selected" : `Verify selected (${n})`;
  };
  const selectAll = $("#rev-select-all", card);
  selectAll?.addEventListener("change", () => {
    checks().forEach((c) => { c.checked = selectAll.checked; });
    syncBulkButton();
  });
  checks().forEach((c) => c.addEventListener("change", () => {
    if (!c.checked && selectAll) selectAll.checked = false;
    syncBulkButton();
  }));

  $$("[data-rev-verify]", card).forEach((btn) => {
    btn.addEventListener("click", async () => {
      const id = btn.dataset.revVerify;
      await withButtonBusy(btn, "Verifying…", async () => {
        try {
          const r = await api("POST", `/api/gaps/${id}/verify`);
          if (r.ok) toast(r.message || "Verified", "info");
          else toast(r.message || "Verify did not complete", "error");
        } catch (e) { toast(e.message, "error"); }
        await refreshDashboard();
      });
    });
  });

  $$("[data-rev-add-round]", card).forEach((btn) => {
    btn.addEventListener("click", () => {
      openAddRoundModal({
        gapId: btn.dataset.revAddRound,
        gapName: btn.dataset.revName || "",
      });
    });
  });

  $("#rev-bulk-verify", card)?.addEventListener("click", async () => {
    const ids = selected();
    if (!ids.length) return;
    const ok = await modalConfirm(
      `Verify ${ids.length} gap${ids.length === 1 ? "" : "s"}?`,
      { title: "Bulk verify", okLabel: "Verify all" },
    );
    if (!ok) return;
    const btn = $("#rev-bulk-verify", card);
    await withButtonBusy(btn, `Verifying 0/${ids.length}…`, async () => {
      let done = 0, failed = 0;
      for (const id of ids) {
        btn.textContent = `Verifying ${done + 1}/${ids.length}…`;
        try {
          const r = await api("POST", `/api/gaps/${id}/verify`);
          if (!r.ok) failed++;
        } catch (_e) { failed++; }
        done++;
      }
      const msg = failed
        ? `Verified ${done - failed} of ${ids.length} — ${failed} did not complete`
        : `Verified ${done} gap${done === 1 ? "" : "s"}`;
      toast(msg, failed ? "error" : "info");
      await refreshDashboard();
    });
  });

  syncBulkButton();
}

function openAddRoundModal({ gapId, gapName }) {
  const reporter = state.lastReporter || "";
  if (!reporter) {
    toast("Pick a reporter in the top-right selector first", "error");
    return;
  }
  const root = document.createElement("div");
  root.className = "modal-backdrop";
  root.innerHTML = `
    <div class="modal" role="dialog" aria-modal="true"
         aria-labelledby="add-round-title" style="max-width:560px">
      <div class="modal-title" id="add-round-title">
        Add round — ${htmlEscape(gapName || gapId)}
      </div>
      <div class="modal-body">
        <div class="muted small" style="margin-bottom:8px">
          Submitting as <strong>${htmlEscape(reporter)}</strong>
          — change in the top-right reporter selector.
        </div>
        <form id="add-round-form">
          <div class="form-row">
            <label>Actual (current behavior)</label>
            <textarea name="actual" placeholder="What's still happening?"></textarea>
          </div>
          <div class="form-row">
            <label>Target (desired behavior)</label>
            <textarea name="target" placeholder="What should be happening?"></textarea>
          </div>
        </form>
      </div>
      <div class="modal-actions">
        <button class="secondary" data-cancel>Cancel</button>
        <button data-ok>Submit new round</button>
      </div>
    </div>`;
  document.body.appendChild(root);
  let closed = false;
  const close = () => {
    if (closed) return;
    closed = true;
    document.removeEventListener("keydown", onKey, true);
    root.remove();
  };
  const onKey = (e) => { if (e.key === "Escape") close(); };
  document.addEventListener("keydown", onKey, true);
  root.addEventListener("click", (e) => { if (e.target === root) close(); });
  root.querySelector("[data-cancel]").addEventListener("click", close);
  const submit = async () => {
    const form = root.querySelector("#add-round-form");
    const fd = new FormData(form);
    const actual = (fd.get("actual") || "").toString().trim();
    const target = (fd.get("target") || "").toString().trim();
    if (!actual && !target) return toast("Provide actual or target", "error");
    const okBtn = root.querySelector("[data-ok]");
    await withButtonBusy(okBtn, "Submitting…", async () => {
      try {
        await api("POST", `/api/gaps/${gapId}/rounds`,
                  { reporter, actual, target });
        toast("New round submitted", "info");
        close();
        await refreshDashboard();
      } catch (err) { toast(err.message, "error"); }
    });
  };
  root.querySelector("[data-ok]").addEventListener("click", submit);
  root.querySelector("#add-round-form").addEventListener("submit", (e) => {
    e.preventDefault(); submit();
  });
  root.querySelector("textarea[name='actual']")?.focus();
}

function renderActivityList(entries) {
  if (!entries.length) return `<p class="muted">No activity yet.</p>`;
  return entries.map((e) => `
    <div class="log-entry ${e.severity || 'info'}">
      <div>${htmlEscape(e.message)}</div>
      <div class="meta">
        ${fmtTime(e.datetime)} · ${htmlEscape(e.category || '')}
        ${e.actor ? ' · ' + htmlEscape(e.actor) : ''}
        ${e.gap_id ? ` · <a href="#/gaps/${e.gap_id}">Gap ${e.gap_id.slice(0,8)}…</a>` : ''}
      </div>
      ${e.details ? `<details><summary class="diff-show-details">Show details</summary><pre>${htmlEscape(e.details)}</pre></details>` : ''}
    </div>`).join("");
}
