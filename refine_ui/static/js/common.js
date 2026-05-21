// refine — vanilla JS single-page app. No build step, no framework.

const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));

const state = {
  reporters: [],
  lastReporter: localStorage.getItem("refine_last_reporter") || "",
  project: null,
  dashboard: null,
  needsAttentionBanners: [],
  currentRoute: null,
  currentGap: null,
  // The hash that's "underneath" the Gap detail modal. Updated whenever
  // navigate() runs for a route other than gaps_detail. Used to restore
  // the URL when the modal is dismissed so the page the user came from
  // is what they land back on.
  underlayHash: "#/",
  // Provider-scoped feature flag matrix from `/api/features`. Refreshed
  // on app start and after any System save. UI helpers below read
  // `state.features` to gate Chat / Import affordances.
  features: null,
};

const WORKFLOW_STATUSES = [
  "backlog",
  "todo",
  "in-progress",
  "ready-merge",
  "awaiting-rebuild",
  "review",
  "done",
  "failed",
  "cancelled",
];
const STATUS_FILTER_OPTIONS = ["", ...WORKFLOW_STATUSES];

function updateActiveInstanceLabel() {
  const el = document.getElementById("active-instance-label");
  if (!el) return;
  const project = state.project || {};
  const active = project.active_instance || null;
  const activeId = project.active_instance_id || "";
  const label = active?.display_name || active?.name || activeId || "none";
  el.textContent = project.attached === false ? "none" : label;
  el.title = el.textContent;
}

async function refreshFeatures() {
  try {
    state.features = await api("GET", "/api/features");
  } catch { /* keep prior value; gates default to permissive */ }
  // Re-render whatever surfaces depend on the matrix.
  applyFeatureGates();
  // Chat dock is always in the DOM — its body content branches on
  // featureEnabled("chat"), so a redraw is needed when the matrix
  // (or the active provider) changes.
  if (typeof drawChatDock === "function") drawChatDock();
  if (state.currentRoute === "settings") refreshSettings();
  if (state.currentRoute === "gaps_detail" && state.currentGap) {
    loadGapDetail(state.currentGap);
  }
}

function featureEnabled(featureKey) {
  // Default-permissive: if we haven't loaded the matrix yet (first
  // paint racing against /api/features), don't block UI interactions
  // — the server is still the source of truth and will reject any
  // gated action with a clear error.
  const f = state.features;
  if (!f) return true;
  const cell = f.matrix?.[`${f.current_provider}.${featureKey}`];
  return cell ? !!cell.enabled : true;
}

function applyFeatureGates() {
  // Top-bar Import button: hide entirely when LLM extraction isn't
  // supported for this provider. Hiding vs. graying-out is
  // intentional — the action has no fallback, so the affordance
  // shouldn't tease the user.
  const importBtn = document.getElementById("btn-import");
  if (importBtn) {
    importBtn.style.display = featureEnabled("import_gaps") ? "" : "none";
  }
  // Chat dock toggle: keep visible (the dock is part of the layout)
  // but mark disabled when chat isn't supported. The dock itself
  // shows an inline "disabled" notice when expanded.
  const chatDock = document.getElementById("chat-dock");
  if (chatDock) {
    chatDock.dataset.disabled = featureEnabled("chat") ? "0" : "1";
  }
}

// ---- API helpers ------------------------------------------------------------

async function api(method, path, body) {
  const opts = { method, headers: {} };
  if (body !== undefined) {
    opts.headers["Content-Type"] = "application/json";
    opts.body = JSON.stringify(body);
  }
  const res = await fetch(path, opts);
  let data = null;
  try { data = await res.json(); } catch {}
  if (!res.ok) {
    const msg = data?.error?.message || res.statusText || "Request failed";
    const err = new Error(msg);
    err.status = res.status;
    err.details = data?.error?.details;
    err.code = data?.error?.code;
    throw err;
  }
  return data;
}

async function waitForBackgroundJob(jobOrId, {
  intervalMs = 750,
  timeoutMs = 10 * 60 * 1000,
} = {}) {
  const jobId = typeof jobOrId === "string" ? jobOrId : jobOrId?.id;
  if (!jobId) throw new Error("Background job id missing");
  const started = Date.now();
  while (true) {
    const snap = await api("GET", `/api/jobs/${jobId}`);
    const job = snap.job || {};
    if (job.status === "complete") return job.result || {};
    if (job.status === "failed") {
      const err = new Error(job.error?.message || "Background job failed");
      err.details = job.error?.details;
      throw err;
    }
    if (Date.now() - started > timeoutMs) {
      throw new Error("Background job timed out");
    }
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  }
}

async function resolveBackgroundJobResponse(response, message = "") {
  if (!response?.job) return response;
  if (message) toast(message, "info");
  const result = await waitForBackgroundJob(response.job);
  if (result.http_status && result.http_status >= 400) {
    throw new Error(result.error?.message || "Background job failed");
  }
  return result;
}

function toast(message, kind = "info") {
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.textContent = message;
  document.body.appendChild(el);
  setTimeout(() => el.remove(), 4000);
}

// ---- Project attach/setup ---------------------------------------------------

async function ensureProjectAttached() {
  const snap = await refreshProjectStatus();
  if (!snap) return false;
  if (snap.attached) {
    const schema = snap.schema || {};
    if (schema.compatible !== false) {
      await syncProjectUpdates({ silent: true });
      return true;
    }
    if (schema.migration_required) {
      const ok = await modalConfirm(
        "This app uses an older Refine schema. Migrate .refine state and open it?",
        { title: "Migrate app", okLabel: "Migrate and open" },
      );
      if (!ok) {
        $("#main").innerHTML = `
          <h2>Project migration required</h2>
          <p class="muted">This app was not loaded because its .refine state needs migration.</p>`;
        return false;
      }
      try {
        const result = await api("POST", "/api/project/attach", {
          path: snap.client_repo,
          migrate: true,
        });
        await applyProjectAttachResult(result);
        return true;
      } catch (e) {
        toast(e.details || e.message || "Migration failed", "error");
        return false;
      }
    }
    $("#main").innerHTML = `
      <h2>Unsupported project schema</h2>
      <p class="muted">This app was not loaded because its .refine state was written by a newer Refine version.</p>`;
    return false;
  }
  const result = await openAddAppModal({
    message: snap.message || "Add an existing app path or a new directory to create and initialize.",
  });
  return !!result;
}

async function refreshProjectStatus() {
  let snap = null;
  try {
    snap = await api("GET", "/api/project/status");
  } catch (e) {
    toast(e.message || "Could not check project status", "error");
    return null;
  }
  state.project = snap;
  updateActiveInstanceLabel();
  return snap;
}

async function syncProjectUpdates({ silent = false } = {}) {
  try {
    const result = await api("POST", "/api/project/sync", {});
    await refreshProjectStatus();
    if (!silent) toast(result.message || "Project updates synced", "info");
    return result;
  } catch (e) {
    const message = e.details || e.message || "Could not sync latest project updates";
    toast(message, silent ? "warn" : "error");
    if (!silent) throw e;
    return null;
  }
}

function openProjectAttachModal({
  message = "",
  title = "Choose project",
  okLabel = "Attach project",
  defaultPath = "",
  reloadOnSuccess = true,
} = {}) {
  return new Promise((resolve) => {
    const root = document.createElement("div");
    root.className = "modal-backdrop project-setup-backdrop";
    root.innerHTML = `
      <div class="modal project-setup-modal" role="dialog" aria-modal="true" aria-labelledby="project-setup-title">
        <form id="project-setup-form">
          <div class="modal-title" id="project-setup-title">${htmlEscape(title)}</div>
          <div class="modal-body">
            <p class="muted">${htmlEscape(message)}</p>
            <label for="project-setup-path">Project path</label>
            <input id="project-setup-path" name="path" type="text" class="modal-input"
                   placeholder="/path/to/app" autocomplete="off" required
                   value="${htmlEscape(defaultPath)}">
            <p class="muted small">
              If the directory does not exist, refine will create it, run git init,
              and add the .refine configuration.
            </p>
            <div class="form-error" id="project-setup-error" style="display:none"></div>
          </div>
          <div class="modal-actions">
            <button class="secondary" type="button" id="project-setup-cancel">Cancel</button>
            <button type="submit" id="project-setup-submit">${htmlEscape(okLabel)}</button>
          </div>
        </form>
      </div>`;
    document.body.appendChild(root);

    const form = root.querySelector("#project-setup-form");
    const input = root.querySelector("#project-setup-path");
    const error = root.querySelector("#project-setup-error");
    const button = root.querySelector("#project-setup-submit");
    root.querySelector("#project-setup-cancel").addEventListener("click", () => {
      root.remove();
      resolve(null);
    });
    input.focus();
    input.select();

    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      const path = input.value.trim();
      if (!path) return;
      error.style.display = "none";
      button.disabled = true;
      button.textContent = "Attaching...";
      try {
        const result = await api("POST", "/api/project/attach", { path });
        if (reloadOnSuccess) {
          state.project = result;
          showProjectAttachToast(result);
          window.location.reload();
        } else {
          await applyProjectAttachResult(result);
          root.remove();
        }
        resolve(result);
      } catch (err) {
        if (err.status === 409 && /migration required/i.test(err.message || "")) {
          const migrate = await modalConfirm(
            "This app uses an older Refine schema. Migrate .refine state and open it?",
            { title: "Migrate app", okLabel: "Migrate and open" },
          );
          if (migrate) {
            try {
              const result = await api("POST", "/api/project/attach", { path, migrate: true });
              if (reloadOnSuccess) {
                state.project = result;
                showProjectAttachToast(result);
                window.location.reload();
              } else {
                await applyProjectAttachResult(result);
                root.remove();
              }
              resolve(result);
              return;
            } catch (migrateErr) {
              err = migrateErr;
            }
          }
        }
        error.textContent = err.details || err.message || "Could not attach project";
        error.style.display = "";
        button.disabled = false;
        button.textContent = okLabel;
      }
    });
  });
}

function openAddAppModal(options = {}) {
  return openProjectAttachModal({
    message: "Add an existing app path or a new directory to create and initialize.",
    title: "Add app",
    okLabel: "Add and switch",
    reloadOnSuccess: false,
    ...options,
  });
}

function showProjectAttachToast(result) {
  if (result.runner && result.runner.started === false && result.runner.message) {
    toast(result.runner.message, "warn");
  } else {
    toast("Project attached", "success");
  }
}

async function applyProjectAttachResult(result) {
  state.project = result;
  updateActiveInstanceLabel();
  state.dashboard = null;
  state.currentGap = null;
  state.underlayHash = "#/system/application";
  if (typeof gapsExcludedIds !== "undefined") gapsExcludedIds.clear();
  showProjectAttachToast(result);
  resetChatForProjectSwitch();
  initSSE();
  await syncProjectUpdates({ silent: true });
  await refreshFeatures();
  await refreshInstanceScopedState({ selectReporterFallback: true });
  await refreshTargetAppToggle();
  if (location.hash !== "#/system/application") {
    location.hash = "#/system/application";
  } else if (state.currentRoute === "settings") {
    await refreshSettings();
  } else {
    navigate();
  }
}

// ---- Modals (replace native prompt / confirm) -------------------------------
//
// Both return a Promise. modalPrompt resolves to the entered string or null
// if cancelled. modalConfirm resolves to a boolean.
//
// Keyboard: Enter submits, Escape cancels. Clicking the backdrop cancels.

function _openModal(buildBody, onResolveDefault, focusSel) {
  return new Promise((resolve) => {
    const root = document.createElement("div");
    root.className = "modal-backdrop";
    const body = buildBody();
    root.innerHTML = `<div class="modal" role="dialog" aria-modal="true">${body}</div>`;
    document.body.appendChild(root);

    let resolved = false;
    function close(value) {
      if (resolved) return;
      resolved = true;
      document.removeEventListener("keydown", onKey, true);
      root.remove();
      resolve(value);
    }
    function onKey(e) {
      if (e.key === "Escape") {
        e.preventDefault();
        close(onResolveDefault.cancel);
      } else if (e.key === "Enter") {
        // Allow Enter inside a textarea to insert newlines (none today, but safe).
        if (e.target && e.target.tagName === "TEXTAREA") return;
        e.preventDefault();
        const okBtn = root.querySelector("[data-ok]");
        if (okBtn) okBtn.click();
      }
    }
    document.addEventListener("keydown", onKey, true);
    root.addEventListener("click", (e) => {
      if (e.target === root) close(onResolveDefault.cancel);
    });

    root.querySelector("[data-cancel]")?.addEventListener("click", () =>
      close(onResolveDefault.cancel));
    root.querySelector("[data-ok]").addEventListener("click", () => {
      const input = root.querySelector(".modal-input");
      close(input ? input.value : onResolveDefault.ok);
    });

    const focus = root.querySelector(focusSel);
    if (focus) {
      focus.focus();
      if (focus.tagName === "INPUT") focus.select();
    }
  });
}

function modalPrompt(label, defaultValue = "", {
  title = null, okLabel = "OK", cancelLabel = "Cancel",
} = {}) {
  const body = () => `
    ${title ? `<div class="modal-title">${htmlEscape(title)}</div>` : ""}
    <div class="modal-body">
      <label>${htmlEscape(label)}</label>
      <input type="text" class="modal-input" value="${htmlEscape(defaultValue)}">
    </div>
    <div class="modal-actions">
      <button class="secondary" data-cancel>${htmlEscape(cancelLabel)}</button>
      <button data-ok>${htmlEscape(okLabel)}</button>
    </div>`;
  return _openModal(body, { cancel: null, ok: "" }, ".modal-input");
}

function modalConfirm(message, {
  title = null, okLabel = "OK", cancelLabel = "Cancel", danger = false,
} = {}) {
  const body = () => `
    ${title ? `<div class="modal-title">${htmlEscape(title)}</div>` : ""}
    <div class="modal-body">${htmlEscape(message)}</div>
    <div class="modal-actions">
      <button class="secondary" data-cancel>${htmlEscape(cancelLabel)}</button>
      <button ${danger ? 'class="danger"' : ""} data-ok>${htmlEscape(okLabel)}</button>
    </div>`;
  return _openModal(body, { cancel: false, ok: true }, "[data-ok]");
}

function modalAlert(message, {
  title = "Action not allowed", okLabel = "OK",
} = {}) {
  const body = () => `
    ${title ? `<div class="modal-title">${htmlEscape(title)}</div>` : ""}
    <div class="modal-body">${htmlEscape(message)}</div>
    <div class="modal-actions">
      <button data-ok>${htmlEscape(okLabel)}</button>
    </div>`;
  return _openModal(body, { cancel: null, ok: true }, "[data-ok]");
}

function isInstanceOwnershipError(err) {
  return err?.code === "instance_ownership"
    || (err?.status === 409 && /owned by another instance/i.test(err?.message || ""));
}

async function showActionError(err, fallbackPrefix = "") {
  if (isInstanceOwnershipError(err)) {
    await modalAlert(err.message || "This action is not allowed because the Gap is owned by another instance.");
    return;
  }
  const message = err?.message || "Request failed";
  toast(fallbackPrefix ? `${fallbackPrefix}: ${message}` : message, "error");
}

function fmtTime(iso) {
  if (!iso) return "";
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}

function fmtElapsed(seconds) {
  seconds = Math.max(0, Math.floor(seconds));
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  return `${(seconds / 3600).toFixed(1)}h`;
}

// Abbreviate counter values so dashboard cards stay legible at scale:
// 999 → "999", 1300 → "1.3K", 1_300_000 → "1.3M", 1_300_000_000 → "1.3B".
// Drops a trailing ".0" (so 1000 reads as "1K", not "1.0K"). Promotes
// to the next tier when 1-decimal rounding would push the value over
// 999 at the current tier (e.g. 999_999 reads as "1M", not "1000K").
function fmtCount(n) {
  n = Number(n) || 0;
  if (n < 1000) return String(n);
  const tiers = [
    { div: 1_000_000_000, suffix: "B" },
    { div: 1_000_000,     suffix: "M" },
    { div: 1_000,         suffix: "K" },
  ];
  for (let i = 0; i < tiers.length; i++) {
    const t = tiers[i];
    if (n < t.div) continue;
    const rounded = Math.round((n / t.div) * 10) / 10;
    if (rounded >= 1000 && i > 0) {
      const up = tiers[i - 1];
      return (n / up.div).toFixed(1).replace(/\.0$/, "") + up.suffix;
    }
    return rounded.toFixed(1).replace(/\.0$/, "") + t.suffix;
  }
  return String(n);
}

function renderPaginationControls(idPrefix, pageMeta = {}, itemCount = 0,
                                  noun = "entry") {
  const limit = Math.max(1, parseInt(pageMeta.limit || itemCount || 1, 10));
  const offset = Math.max(0, parseInt(pageMeta.offset || 0, 10));
  const page = Math.floor(offset / limit) + 1;
  const hasPrev = offset > 0;
  const hasNext = !!pageMeta.has_more;
  if (!hasPrev && !hasNext) return "";
  const start = itemCount ? offset + 1 : offset;
  const end = offset + itemCount;
  const label = itemCount
    ? `${start}-${end} ${noun}${itemCount === 1 ? "" : "s"}`
    : `Page ${page}`;
  return `
    <div class="pagination" id="${htmlEscape(idPrefix)}-pagination">
      <span class="muted small">${htmlEscape(label)}</span>
      <span class="spacer"></span>
      <button class="secondary small" data-page="${page - 1}" ${hasPrev ? "" : "disabled"}>Previous</button>
      <span class="muted small">Page ${page}</span>
      <button class="secondary small" data-page="${page + 1}" ${hasNext ? "" : "disabled"}>Next</button>
    </div>`;
}

function bindPaginationControls(root, idPrefix, onPage) {
  $$(`#${idPrefix}-pagination [data-page]`, root).forEach((btn) => {
    btn.addEventListener("click", () => onPage(parseInt(btn.dataset.page, 10)));
  });
}

function htmlEscape(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  }[c]));
}

// ---- Minimal Markdown → HTML ------------------------------------------------
//
// Used to render chat transcripts. Inputs come from the selected agent CLI's
// stream-json `assistant.content[].text` blocks (text only — never raw HTML),
// plus the user-echoed `> message` lines we synthesize locally. We html-escape
// every text fragment before substitution and only emit a small whitelist of
// inline tags, so even if agent text contained literal HTML we'd render it
// as literal text.
//
// Block-level: code fences (```), headings (#…######), unordered (- / *) and
// ordered (1.) lists, blockquotes (>), paragraphs separated by blank lines.
// Inline: **bold**, *italic*, `code`, [text](http(s)://...).
function mdToHtml(text) {
  if (!text) return "";
  const lines = String(text).replace(/\r\n/g, "\n").split("\n");
  const out = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    const fence = line.match(/^```\s*([^\s`]*)\s*$/);
    if (fence) {
      const lang = fence[1] || "";
      const code = [];
      i++;
      while (i < lines.length && !/^```\s*$/.test(lines[i])) {
        code.push(lines[i]);
        i++;
      }
      if (i < lines.length) i++;
      const cls = lang ? ` class="lang-${htmlEscape(lang)}"` : "";
      out.push(`<pre><code${cls}>${htmlEscape(code.join("\n"))}</code></pre>`);
      continue;
    }
    if (!line.trim()) { i++; continue; }
    const heading = line.match(/^(#{1,6})\s+(.+)$/);
    if (heading) {
      const lvl = heading[1].length;
      out.push(`<h${lvl}>${mdInline(heading[2])}</h${lvl}>`);
      i++;
      continue;
    }
    if (/^\s*>\s?/.test(line)) {
      const quoted = [];
      while (i < lines.length && /^\s*>\s?/.test(lines[i])) {
        quoted.push(lines[i].replace(/^\s*>\s?/, ""));
        i++;
      }
      // Recurse so nested formatting inside the quote works.
      out.push(`<blockquote>${mdToHtml(quoted.join("\n"))}</blockquote>`);
      continue;
    }
    if (/^\s*[-*]\s+/.test(line)) {
      const items = [];
      while (i < lines.length && /^\s*[-*]\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^\s*[-*]\s+/, ""));
        i++;
      }
      out.push(`<ul>${items.map(it => `<li>${mdInline(it)}</li>`).join("")}</ul>`);
      continue;
    }
    if (/^\s*\d+\.\s+/.test(line)) {
      const items = [];
      while (i < lines.length && /^\s*\d+\.\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^\s*\d+\.\s+/, ""));
        i++;
      }
      out.push(`<ol>${items.map(it => `<li>${mdInline(it)}</li>`).join("")}</ol>`);
      continue;
    }
    // Paragraph: gather until a blank line or a recognized block opener.
    const para = [];
    while (i < lines.length) {
      const ln = lines[i];
      if (!ln.trim()) break;
      if (/^```/.test(ln) || /^(#{1,6})\s+/.test(ln) ||
          /^\s*[-*]\s+/.test(ln) || /^\s*\d+\.\s+/.test(ln) ||
          /^\s*>\s?/.test(ln)) break;
      para.push(ln);
      i++;
    }
    if (para.length) {
      out.push(`<p>${mdInline(para.join("\n"))}</p>`);
    }
  }
  return out.join("\n");
}

function mdInline(text) {
  let s = htmlEscape(text);
  // Inline code first so its contents aren't mangled by later passes.
  s = s.replace(/`([^`\n]+)`/g, (_, c) => `<code>${c}</code>`);
  // Bold before italic so ** isn't greedily matched as italic.
  s = s.replace(/\*\*([^*\n]+)\*\*/g, "<strong>$1</strong>");
  s = s.replace(/__([^_\n]+)__/g, "<strong>$1</strong>");
  s = s.replace(/\*([^*\n]+)\*/g, "<em>$1</em>");
  s = s.replace(/(^|[^\w])_([^_\n]+)_(?![\w])/g, "$1<em>$2</em>");
  // Links: only http(s)/mailto pass through; anything else stays literal.
  s = s.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (m, txt, url) => {
    if (!/^(https?:|mailto:)/i.test(url)) return m;
    return `<a href="${url}" target="_blank" rel="noopener noreferrer">${txt}</a>`;
  });
  // Convert remaining hard newlines inside a paragraph to <br>.
  s = s.replace(/\n/g, "<br>");
  return s;
}

// ---- reporter dropdown ------------------------------------------------------

async function refreshReporters({ selectFallback = false } = {}) {
  const data = await api("GET", "/api/reporters");
  state.reporters = data.reporters || [];
  reconcileLastReporter({ selectFallback });
  populateAllReporterDropdowns();
}

async function refreshInstanceScopedState({ selectReporterFallback = false } = {}) {
  if (typeof resetChatForProjectSwitch === "function") resetChatForProjectSwitch();
  state.reporters = [];
  state.dashboard = null;
  state.currentGap = null;
  setLastReporter("");
  populateAllReporterDropdowns();
  await refreshReporters({ selectFallback: selectReporterFallback });
}

function reconcileLastReporter({ selectFallback = false } = {}) {
  const names = state.reporters.map((r) => r.name).filter(Boolean);
  if (state.lastReporter && names.includes(state.lastReporter)) return;
  const next = selectFallback ? (names[0] || "") : "";
  setLastReporter(next);
}

function populateAllReporterDropdowns() {
  const all = $$("[data-reporter-select]");
  // include the global reporter selector in the topbar
  const globalSel = $("#global-reporter");
  if (globalSel) all.push(globalSel);
  for (const sel of all) {
    const current = sel.value || sel.dataset.value || state.lastReporter;
    sel.innerHTML = "";
    const optBlank = document.createElement("option");
    optBlank.value = "";
    optBlank.textContent = "— pick reporter —";
    sel.appendChild(optBlank);
    for (const r of state.reporters) {
      const opt = document.createElement("option");
      opt.value = r.name;
      opt.textContent = r.name;
      sel.appendChild(opt);
    }
    const optAdd = document.createElement("option");
    optAdd.value = "__add__";
    optAdd.textContent = "+ Add new reporter…";
    sel.appendChild(optAdd);
    // Restore selection if still valid
    const stillValid = state.reporters.some((r) => r.name === current);
    sel.value = stillValid ? current : "";
  }
}

async function handleReporterAdd(sel) {
  const name = await modalPrompt("Name for the new reporter:",
                                  "", { title: "Add reporter" });
  if (!name || !name.trim()) {
    sel.value = state.lastReporter || "";
    return null;
  }
  try {
    const { reporter } = await api("POST", "/api/reporters", { name });
    await refreshReporters();
    sel.value = reporter.name;
    setLastReporter(reporter.name);
    return reporter.name;
  } catch (e) {
    toast(`Could not add reporter: ${e.message}`, "error");
    sel.value = state.lastReporter || "";
    return null;
  }
}

function setLastReporter(name) {
  const wasEmpty = !state.lastReporter;
  state.lastReporter = name;
  if (name) localStorage.setItem("refine_last_reporter", name);
  else localStorage.removeItem("refine_last_reporter");
  const g = $("#global-reporter");
  if (g) g.value = name;
  // Keep any in-page "Submitting as X" indicator in sync without re-rendering
  // the form (which would lose the user's typed-but-unsubmitted text).
  for (const el of $$(".js-reporter-name")) el.textContent = name;
  // If the user just picked their first reporter, re-render views gated on a
  // selected reporter so the form replaces the "pick a reporter" notice.
  if (wasEmpty && name) {
    const r = state.currentRoute;
    if (r === "gaps_new" || r === "gaps_import" || r === "gaps_detail") {
      navigate();
    }
  }
  // Dashboard's "Awaiting your review" section is reporter-scoped — refresh
  // it whenever the selection changes so the list re-targets immediately.
  if (state.currentRoute === "dashboard") refreshDashboard();
}

// react to "+ Add new reporter" selection on any dropdown
document.addEventListener("change", async (e) => {
  if (e.target.matches("[data-reporter-select], #global-reporter")) {
    if (e.target.value === "__add__") {
      const newName = await handleReporterAdd(e.target);
      if (newName) e.target.dispatchEvent(new Event("change-after-add"));
    } else if (e.target.value) {
      setLastReporter(e.target.value);
    }
  }
});

// "+ New Gap" and "Import…" in the topbar open modals in place rather than
// navigating to dedicated screens. The hrefs are kept for deep-linking /
// accessibility; click handlers intercept so the user's current view stays
// underneath.
document.addEventListener("click", (e) => {
  if (e.target.closest("#btn-new-gap")) {
    e.preventDefault();
    openNewGapModal();
  } else if (e.target.closest("#btn-import")) {
    e.preventDefault();
    openImportModal();
  }
});

// ---- Banners ----------------------------------------------------------------

function renderBanners(items) {
  const root = $("#banners");
  root.innerHTML = "";
  for (const item of items) {
    const tpl = $("#t-banner").content.cloneNode(true);
    const div = tpl.querySelector(".banner");
    div.classList.add(item.severity || "error");
    tpl.querySelector(".banner-msg").textContent = item.message;
    if (item.action) {
      const btn = document.createElement("button");
      btn.textContent = item.action.label;
      btn.onclick = item.action.onClick;
      tpl.querySelector(".banner-actions").appendChild(btn);
    }
    root.appendChild(tpl);
  }
}

// ---- SSE --------------------------------------------------------------------

let sseSource = null;

function initSSE() {
  if (sseSource) sseSource.close();
  sseSource = new EventSource("/api/sse");
  sseSource.addEventListener("activity_added", (e) => {
    // Refresh dashboard activity if visible; refresh current gap if relevant.
    // Route through the silent `refresh*` paths — not `render*` — so the
    // screen doesn't blink back to `Loading…` on every event.
    if (state.currentRoute === "dashboard") refreshDashboard();
    if (state.currentRoute === "logs") loadLogs();
    if (state.currentRoute === "changes") loadChanges();
    if (state.currentRoute === "gaps_detail" && state.currentGap) {
      try {
        const data = JSON.parse(e.data);
        if (!data.gap_id || data.gap_id === state.currentGap) {
          if (typeof invalidateGapRoundLogs === "function") invalidateGapRoundLogs(state.currentGap);
          loadGapDetail(state.currentGap);
        }
      } catch {}
    }
  });
  sseSource.addEventListener("status_change", () => {
    if (state.currentRoute === "dashboard") refreshDashboard();
    // Refresh only the table on background updates so an in-progress
    // keystroke in the search box isn't interrupted by a full re-render.
    if (state.currentRoute === "gaps") refreshGapsTable();
    if (state.currentRoute === "logs") loadLogs();
    if (state.currentRoute === "settings" &&
        document.querySelector('[data-tab-pane="runtime"].active')) {
      refreshSettings();
    }
    // Changes screen: the Merge agent can land a new merge commit;
    // a cancellation flips an existing row's Undo button state.
    if (state.currentRoute === "changes") loadChanges();
    if (state.currentRoute === "gaps_detail" && state.currentGap) {
      loadGapDetail(state.currentGap);
    }
  });
  sseSource.addEventListener("target_app_state", () => {
    refreshTargetAppToggle();
  });
  sseSource.addEventListener("target_app_health", () => {
    refreshTargetAppToggle();
  });
  sseSource.addEventListener("project_updated", async () => {
    await refreshProjectStatus();
    await refreshFeatures();
    await refreshReporters();
    if (state.currentRoute === "dashboard") refreshDashboard();
    if (state.currentRoute === "gaps") refreshGapsTable();
    if (state.currentRoute === "logs") loadLogs();
    if (state.currentRoute === "settings") refreshSettings();
    if (state.currentRoute === "changes") loadChanges();
    if (state.currentRoute === "gaps_detail" && state.currentGap) {
      loadGapDetail(state.currentGap);
    }
  });
  sseSource.addEventListener("round_log_added", (e) => {
    // Subprocess flushed new stdout to the active round's logs[]. If the user
    // is viewing that gap's detail, refresh so the new lines appear live.
    if (state.currentRoute !== "gaps_detail" || !state.currentGap) return;
    try {
      const data = JSON.parse(e.data);
      if (data.gap_id === state.currentGap) {
        if (typeof invalidateGapRoundLogs === "function") invalidateGapRoundLogs(state.currentGap);
        loadGapDetail(state.currentGap);
      }
    } catch {}
  });
  sseSource.onerror = () => {
    // Browser will auto-reconnect.
  };
}
