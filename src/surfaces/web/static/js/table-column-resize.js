// ---- Bounded table-column resizing -----------------------------------------

// Column widths are surface preferences rather than durable application state.
// Keep them in memory for redraws and sessionStorage for reloads in the same tab.
// A new tab or browser session intentionally starts from the documented default.
const TABLE_COLUMN_WIDTHS = new Map();

function clampTableColumnWidth(value, config) {
  const parsed = Number(value);
  const fallback = Number(config.defaultWidth);
  const width = Number.isFinite(parsed) ? parsed : fallback;
  return Math.max(config.minWidth, Math.min(config.maxWidth, Math.round(width)));
}

function readTableColumnWidth(config) {
  if (TABLE_COLUMN_WIDTHS.has(config.storageKey)) {
    return TABLE_COLUMN_WIDTHS.get(config.storageKey);
  }
  let stored = null;
  try {
    stored = sessionStorage.getItem(config.storageKey);
  } catch {}
  const width = clampTableColumnWidth(stored === null ? config.defaultWidth : stored, config);
  TABLE_COLUMN_WIDTHS.set(config.storageKey, width);
  return width;
}

function saveTableColumnWidth(config, width) {
  const next = clampTableColumnWidth(width, config);
  TABLE_COLUMN_WIDTHS.set(config.storageKey, next);
  try {
    sessionStorage.setItem(config.storageKey, String(next));
  } catch {}
  return next;
}

function renderTableColumnResizeHandle(config, width = readTableColumnWidth(config)) {
  const current = clampTableColumnWidth(width, config);
  return `<span class="table-column-resize-handle"
                data-table-column-resize="${config.key}"
                data-testid="${config.testId}"
                role="separator" tabindex="0"
                aria-orientation="vertical"
                aria-label="Resize ${config.label} column"
                aria-valuemin="${config.minWidth}"
                aria-valuemax="${config.maxWidth}"
                aria-valuenow="${current}"
                title="Drag to resize ${config.label}; use Left and Right Arrow keys"></span>`;
}

function applyTableColumnWidth(root, config, width, { persist = false } = {}) {
  const next = persist
    ? saveTableColumnWidth(config, width)
    : clampTableColumnWidth(width, config);
  TABLE_COLUMN_WIDTHS.set(config.storageKey, next);
  const table = root.querySelector(config.tableSelector);
  const handle = root.querySelector(`[data-table-column-resize="${config.key}"]`);
  table?.style.setProperty(config.cssProperty, `${next}px`);
  handle?.setAttribute("aria-valuenow", String(next));
  return next;
}

function bindTableColumnResize(root, config) {
  const handle = root.querySelector(`[data-table-column-resize="${config.key}"]`);
  if (!handle) return;

  bindOnce(handle, "click", (event) => event.stopPropagation());
  bindOnce(handle, "dblclick", (event) => {
    event.preventDefault();
    event.stopPropagation();
    applyTableColumnWidth(root, config, config.defaultWidth, { persist: true });
  });
  bindOnce(handle, "keydown", (event) => {
    const current = readTableColumnWidth(config);
    let next = null;
    if (event.key === "ArrowLeft") next = current - config.step;
    if (event.key === "ArrowRight") next = current + config.step;
    if (event.key === "Home") next = config.minWidth;
    if (event.key === "End") next = config.maxWidth;
    if (next === null) return;
    event.preventDefault();
    event.stopPropagation();
    applyTableColumnWidth(root, config, next, { persist: true });
  });
  bindOnce(handle, "pointerdown", (event) => {
    if (event.button !== undefined && event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    const startX = event.clientX;
    const startWidth = readTableColumnWidth(config);
    const pointerId = event.pointerId;
    root.classList.add("resizing-column");

    function onMove(moveEvent) {
      if (moveEvent.pointerId !== pointerId) return;
      applyTableColumnWidth(root, config, startWidth + moveEvent.clientX - startX);
    }
    function onUp(upEvent) {
      if (upEvent.pointerId !== pointerId) return;
      document.removeEventListener("pointermove", onMove);
      document.removeEventListener("pointerup", onUp);
      document.removeEventListener("pointercancel", onUp);
      root.classList.remove("resizing-column");
      root.dataset.columnResizeSuppressSort = "1";
      setTimeout(() => delete root.dataset.columnResizeSuppressSort, 0);
      applyTableColumnWidth(root, config, readTableColumnWidth(config), { persist: true });
    }

    // Keep the gesture alive if a Goals refresh morphs the header mid-drag.
    document.addEventListener("pointermove", onMove);
    document.addEventListener("pointerup", onUp);
    document.addEventListener("pointercancel", onUp);
  });
}
