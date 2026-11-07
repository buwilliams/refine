// ---- System / Runtime / Governance compile gate ----------------------------
//
// Opt-in Governance-phase guard that runs a mechanical compile check before
// the LLM Governance review. The setting value is the shell command the gate
// runs; the recommended value is a path to a committed script. When the
// configured value looks like a path, this section lets the operator view and
// edit the script inline rather than ssh into the box.

const compileGateState = {
  path: null,
  loaded: null,
  content: "",
  editing: false,
  dirty: false,
  size: 0,
  executable: false,
  loading: false,
  saving: false,
  error: "",
};

function renderCompileGateSection(s) {
  const path = String(s.governance_compile_command ?? "").trim();
  const timeout = String(s.governance_compile_timeout_seconds ?? "600").trim() || "600";
  return `
    <section class="settings-section" data-testid="settings-compile-gate">
      <h3>Governance compile gate</h3>
      <p class="muted small" style="margin-top:0">
        Opt-in guard that runs a mechanical compile check before the Governance
        review. A non-zero exit short-circuits the Goal to Failed with the build
        log attached to the recovery round. Point this at a <strong>committed
        script</strong> under the app repo. Inline shell works but doesn't
        version-control cleanly.
      </p>
      ${renderSettingsEditableField({
        id: "s-governance-compile-command",
        label: "Compile gate script",
        guideItemId: "governance-compile-command",
        description: "repo-relative path to a committed script (e.g. ./scripts/compile-check.sh). Leave blank to disable the gate.",
        valueLabel: path || "disabled",
        emptyLabel: "disabled",
        control: `<input type="text" id="s-governance-compile-command"
                         data-testid="governance-compile-command"
                         placeholder="./scripts/compile-check.sh"
                         value="${htmlEscape(path)}">`,
      })}
      ${renderSettingsEditableField({
        id: "s-governance-compile-timeout",
        label: "Compile gate timeout (s)",
        guideItemId: "governance-compile-timeout",
        description: "maximum wall-clock seconds the gate may run before being killed.",
        valueLabel: timeout,
        control: `<input type="number" id="s-governance-compile-timeout"
                         data-testid="governance-compile-timeout"
                         min="1" value="${htmlEscape(timeout)}">`,
      })}
      <div class="compile-gate-editor" data-testid="compile-gate-editor">
        <div class="actions" style="margin-top:12px;align-items:center;gap:8px">
          <strong>Script contents</strong>
          <span class="spacer"></span>
          <span class="muted small" data-compile-gate-status></span>
          <button type="button" class="secondary" data-compile-gate-load data-testid="compile-gate-load">Load file</button>
          <button type="button" class="secondary" data-compile-gate-edit data-testid="compile-gate-edit" hidden>Edit</button>
          <button type="button" data-compile-gate-save data-testid="compile-gate-save" hidden>Save</button>
          <button type="button" class="secondary" data-compile-gate-cancel data-testid="compile-gate-cancel" hidden>Cancel</button>
        </div>
        <p class="muted small" data-compile-gate-help>Click <strong>Load file</strong> to view or edit the file at the path above. Only available when the setting is a path (not an inline command).</p>
        <pre class="compile-gate-preview" data-compile-gate-preview hidden
             style="max-height:360px;overflow:auto;background:var(--panel, #111);border:1px solid var(--line, #333);border-radius:4px;padding:10px;font-family:ui-monospace,monospace;font-size:12px;white-space:pre"></pre>
        <textarea data-compile-gate-textarea data-testid="compile-gate-textarea" rows="14" hidden spellcheck="false"
                  style="width:100%;font-family:ui-monospace,monospace;font-size:12px"></textarea>
      </div>
    </section>`;
}

function compileGateNormalizePath(raw) {
  const trimmed = String(raw || "").trim();
  if (!trimmed) return "";
  if (/[\n|&;<>$`()]/.test(trimmed)) return "";
  const firstToken = trimmed.split(/\s+/)[0];
  if (!firstToken) return "";
  if (!firstToken.includes("/")) return "";
  return firstToken.replace(/^\.\//, "").replace(/^\/+/, "");
}

function compileGatePathCandidate() {
  const el = document.getElementById("s-governance-compile-command");
  return el ? compileGateNormalizePath(el.value || "") : "";
}

function compileGateStatusText() {
  if (compileGateState.loading) return "loading…";
  if (compileGateState.saving) return "saving…";
  if (compileGateState.error) return compileGateState.error;
  if (compileGateState.loaded === "exists") {
    const exec = compileGateState.executable ? " · executable" : " · not executable";
    return `loaded · ${compileGateState.size} bytes${exec}`;
  }
  if (compileGateState.loaded === "missing") return "no file at this path yet";
  return "";
}

function drawCompileGateEditor(section) {
  if (!section) return;
  const status = section.querySelector("[data-compile-gate-status]");
  const help = section.querySelector("[data-compile-gate-help]");
  const preview = section.querySelector("[data-compile-gate-preview]");
  const textarea = section.querySelector("[data-compile-gate-textarea]");
  const loadBtn = section.querySelector("[data-compile-gate-load]");
  const editBtn = section.querySelector("[data-compile-gate-edit]");
  const saveBtn = section.querySelector("[data-compile-gate-save]");
  const cancelBtn = section.querySelector("[data-compile-gate-cancel]");
  if (!status || !preview || !textarea || !loadBtn || !editBtn || !saveBtn || !cancelBtn || !help) return;
  const hasPath = !!compileGatePathCandidate();
  loadBtn.disabled = !hasPath || compileGateState.loading || compileGateState.saving;
  status.textContent = compileGateStatusText();
  const hide = (el) => { el.hidden = true; };
  const show = (el) => { el.hidden = false; };
  if (!hasPath) {
    hide(preview); hide(textarea); hide(editBtn); hide(saveBtn); hide(cancelBtn);
    show(help);
    return;
  }
  if (compileGateState.editing) {
    hide(preview); show(textarea);
    textarea.value = compileGateState.content;
    hide(editBtn); show(saveBtn); show(cancelBtn);
    saveBtn.disabled = compileGateState.saving;
    hide(help);
    return;
  }
  if (compileGateState.loaded === "exists") {
    show(preview); hide(textarea);
    preview.textContent = compileGateState.content;
    show(editBtn); editBtn.textContent = "Edit";
    hide(saveBtn); hide(cancelBtn); hide(help);
    return;
  }
  if (compileGateState.loaded === "missing") {
    hide(preview); hide(textarea);
    show(editBtn); editBtn.textContent = "Create";
    hide(saveBtn); hide(cancelBtn); hide(help);
    return;
  }
  hide(preview); hide(textarea);
  hide(editBtn); hide(saveBtn); hide(cancelBtn);
  show(help);
}

async function compileGateLoad(section) {
  const path = compileGatePathCandidate();
  if (!path) return;
  compileGateState.loading = true;
  compileGateState.error = "";
  drawCompileGateEditor(section);
  try {
    const file = await api(
      "GET",
      `/api/files/read?path=${encodeURIComponent(path)}&offset=0&limit=262144`,
      undefined,
      { recordError: false },
    );
    compileGateState.path = path;
    compileGateState.loaded = "exists";
    compileGateState.size = Number(file.size || 0);
    if (file.kind !== "text") {
      compileGateState.content = "";
      compileGateState.executable = false;
      compileGateState.error = "File is not text; edit it from a terminal.";
    } else {
      compileGateState.content = String(file.content || "");
      // The read endpoint doesn't return the executable bit. Assume a
      // committed compile-gate script is executable; a save with
      // `executable: true` makes it so if it wasn't.
      compileGateState.executable = true;
    }
  } catch (e) {
    if (e?.status === 404) {
      compileGateState.path = path;
      compileGateState.loaded = "missing";
      compileGateState.content = "#!/bin/sh\nset -eu\n\n# Return non-zero to fail the gate and record the build log.\n";
      compileGateState.size = 0;
      compileGateState.executable = true;
    } else {
      compileGateState.error = e?.message || "Load failed";
    }
  } finally {
    compileGateState.loading = false;
    compileGateState.editing = false;
    compileGateState.dirty = false;
    drawCompileGateEditor(section);
  }
}

async function compileGateSave(section) {
  if (compileGateState.saving) return;
  const textarea = section.querySelector("[data-compile-gate-textarea]");
  const path = compileGateState.path || compileGatePathCandidate();
  if (!path || !textarea) return;
  compileGateState.content = textarea.value;
  compileGateState.saving = true;
  compileGateState.error = "";
  drawCompileGateEditor(section);
  try {
    const result = await api("POST", "/api/files/write", {
      path,
      content: compileGateState.content,
      executable: compileGateState.executable,
    });
    compileGateState.size = Number(result.size || compileGateState.content.length);
    compileGateState.executable = !!result.executable;
    compileGateState.loaded = "exists";
    compileGateState.editing = false;
    compileGateState.dirty = false;
    toast("Compile gate script saved", "info");
  } catch (e) {
    compileGateState.error = e?.message || "Save failed";
  } finally {
    compileGateState.saving = false;
    drawCompileGateEditor(section);
  }
}

function bindCompileGateSection(root) {
  const section = root?.querySelector('[data-testid="settings-compile-gate"]');
  if (!section) return;
  const pathInput = section.querySelector("#s-governance-compile-command");
  if (pathInput) {
    bindOnce(pathInput, "input", () => {
      const next = compileGateNormalizePath(pathInput.value);
      if (next !== compileGateState.path) {
        compileGateState.loaded = null;
        compileGateState.content = "";
        compileGateState.error = "";
        compileGateState.editing = false;
        compileGateState.path = null;
      }
      drawCompileGateEditor(section);
    }, "compile-gate-path");
  }
  bindOnce(section.querySelector("[data-compile-gate-load]"), "click",
    () => compileGateLoad(section), "compile-gate-load");
  bindOnce(section.querySelector("[data-compile-gate-edit]"), "click", () => {
    compileGateState.editing = true;
    compileGateState.error = "";
    drawCompileGateEditor(section);
    section.querySelector("[data-compile-gate-textarea]")?.focus();
  }, "compile-gate-edit");
  bindOnce(section.querySelector("[data-compile-gate-save]"), "click",
    () => compileGateSave(section), "compile-gate-save");
  bindOnce(section.querySelector("[data-compile-gate-cancel]"), "click", () => {
    compileGateState.editing = false;
    compileGateState.error = "";
    drawCompileGateEditor(section);
  }, "compile-gate-cancel");
  const textarea = section.querySelector("[data-compile-gate-textarea]");
  if (textarea) {
    bindOnce(textarea, "input", () => {
      compileGateState.content = textarea.value;
      compileGateState.dirty = true;
    }, "compile-gate-textarea");
  }
  drawCompileGateEditor(section);
}
