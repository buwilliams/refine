// ---- Init -------------------------------------------------------------------

// Increment the live "Elapsed" / "Idle" cells once per second so the
// dashboard and System runtime feel responsive even between SSE refreshes.
// Cells without `.js-elapsed-tick` / `.js-idle-tick` no-op cheaply.
function tickRunningCells() {
  const now = Date.now();
  document.querySelectorAll(".js-elapsed-tick, .js-idle-tick").forEach((el) => {
    const base = parseInt(el.dataset.base, 10);
    const anchor = parseInt(el.dataset.anchorMs, 10);
    if (Number.isNaN(base) || Number.isNaN(anchor)) return;
    const seconds = base + Math.floor((now - anchor) / 1000);
    const next = fmtElapsed(seconds);
    if (el.textContent !== next) el.textContent = next;
  });
}

async function init() {
  const attached = await ensureProjectAttached();
  if (!attached && state.project?.attached !== false) return;
  if (attached) {
    try {
      await refreshReporters();
    } catch (e) {
      // not fatal — likely fresh install with no reporters yet
    }
  }
  initToolbar();
  if (typeof initGuide === "function") initGuide();
  if (typeof initCommandPalette === "function") initCommandPalette();
  initTargetAppToggle();
  if (attached) {
    initSSE();
  } else {
    enterNoProjectMode(state.project, { openGuidePanel: true });
  }
  setInterval(tickRunningCells, 1000);
  if (typeof recoverImportSessionOnLoad === "function") {
    recoverImportSessionOnLoad();
  }
  navigate();
}

init();
