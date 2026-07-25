// ---- Redraw-in-place rendering ----------------------------------------------
//
// THE PATTERN. Every screen that redraws after its first paint uses exactly two
// calls, and nothing else should assign `innerHTML` on a refresh path:
//
//   async function refreshThing() {
//     const data = await api("GET", "/api/thing");
//     renderInto($("#thing"), thingHtml(data), () => {
//       $$(".thing-row").forEach((row) => bindOnce(row, "click", onRowClick));
//     });
//   }
//
// `renderInto` morphs the new HTML onto the existing tree instead of replacing
// it. Nodes that did not change keep their identity, so the user's focus, caret,
// scroll offset, and every attached listener survive a background refresh.
//
// `bindOnce` makes the bind step safe to run after every render: nodes that
// survived the morph are skipped, and only new nodes get listeners. That is what
// removes the need to strip listeners by replacing nodes first — the old
// approach, which is precisely what interrupted the user mid-edit.
//
// Two rules the callbacks must follow:
//
//   1. Read live state inside the handler; do not close over data from the
//      render that bound it. A surviving node keeps its original handler, so a
//      captured `data` goes stale on the next refresh. Read `state.*` or the
//      element's own `dataset` instead.
//   2. Route-entry paints may still assign `innerHTML` — replacing the whole
//      screen on navigation is correct. It is only refreshes that must morph.

// Which controls a redraw must leave alone: the one the user is currently in,
// and any whose value they have changed but not yet saved.
//
// "Not yet saved" is derived, not tracked. A render emits the server's value as
// the element's attribute, which the DOM exposes as its default value, so a live
// value that differs from the default is necessarily a local edit. Once a save
// round-trips, the next render emits the new value as the attribute and the
// control is clean again — no dirty flags to set, clear, or leak.
function isUserEditedControl(el) {
  const tag = el?.tagName;
  if (tag === "INPUT") {
    const type = String(el.type || "").toLowerCase();
    if (type === "checkbox" || type === "radio") return el.checked !== el.defaultChecked;
    if (type === "file" || type === "button" || type === "submit") return false;
    return el.value !== el.defaultValue;
  }
  if (tag === "TEXTAREA") return el.value !== el.defaultValue;
  if (tag === "SELECT") {
    const rendered = Array.from(el.options || []).find((opt) => opt.defaultSelected);
    const renderedValue = rendered ? rendered.value : (el.options?.[0]?.value ?? "");
    return el.value !== renderedValue;
  }
  return false;
}

function isMorphEditableControl(el) {
  return ["INPUT", "TEXTAREA", "SELECT"].includes(el?.tagName);
}

// Local UI state the server knows nothing about: a field switched from preview to
// edit, an inline editor opened. The rendered HTML always describes the closed
// state, so a redraw would collapse it. Claim the subtree while it is open.
//
// Marked with a `data-` attribute rather than a WeakMap because the guard skips
// the marked element before the morph reaches it, so the attribute is never
// synced away — and because the mark applies to the whole subtree, not one node.
function preserveDuringMorph(el) {
  if (el) el.dataset.morphPreserve = "1";
}

function releaseAfterMorph(el) {
  if (el) delete el.dataset.morphPreserve;
}

function isPreservedDuringMorph(el) {
  return el?.dataset?.morphPreserve === "1";
}

function shouldPreserveDuringMorph(el, activeEl) {
  if (!el || el.nodeType !== 1) return false;
  // Checked before the control tests below: this claims a whole subtree, and the
  // element carrying it is usually a wrapper rather than the control itself.
  if (isPreservedDuringMorph(el)) return true;
  if (!isMorphEditableControl(el)) return false;
  // Never yank the caret out of the control the user is typing in, even if they
  // have not changed it yet — a value swap under the cursor is just as jarring.
  if (activeEl && el === activeEl) return true;
  return isUserEditedControl(el);
}

// Listeners already attached, tracked off-DOM. A `data-` marker would not work:
// the morph syncs attributes against the incoming HTML, which does not carry the
// marker, so it would be stripped and the element would be bound twice.
const BOUND_LISTENERS = new WeakMap();

// Attach `handler` to `el` once, no matter how many redraws call this.
//
// `key` distinguishes multiple listeners for the same event on one element;
// it defaults to the event name, which is what nearly every caller wants.
function bindOnce(el, event, handler, key = event) {
  if (!el) return;
  let bound = BOUND_LISTENERS.get(el);
  if (!bound) {
    bound = new Set();
    BOUND_LISTENERS.set(el, bound);
  }
  if (bound.has(key)) return;
  bound.add(key);
  el.addEventListener(event, handler);
}

function morphChildren(root, html) {
  const activeEl = document.activeElement;
  Idiomorph.morph(root, html, {
    morphStyle: "innerHTML",
    callbacks: {
      beforeNodeMorphed: (fromEl) => !shouldPreserveDuringMorph(fromEl, activeEl),
    },
  });
}

// Redraw `root`'s contents from `html`, then re-run `bind`.
//
// `bind` must use `bindOnce` (or otherwise be idempotent) because it runs on
// every redraw. It runs unconditionally so that nodes the morph introduced are
// always bound.
function renderInto(root, html, bind) {
  if (!root) return;
  // Idiomorph keeps nodes in place, but a changed tag or id still forces a
  // replacement, which would drop focus. Cheap safety net for that case.
  const focus = captureMorphFocus(root);
  morphChildren(root, html);
  if (typeof bind === "function") bind();
  restoreMorphFocus(root, focus);
}

function captureMorphFocus(root) {
  const el = document.activeElement;
  if (!el || !root?.contains(el) || !isMorphEditableControl(el)) return null;
  const controls = $$("input, select, textarea", root);
  return {
    id: el.id || "",
    name: el.getAttribute("name") || "",
    testId: el.getAttribute("data-testid") || "",
    index: controls.indexOf(el),
    selectionStart: typeof el.selectionStart === "number" ? el.selectionStart : null,
    selectionEnd: typeof el.selectionEnd === "number" ? el.selectionEnd : null,
    scrollTop: el.scrollTop || 0,
  };
}

function restoreMorphFocus(root, snapshot) {
  if (!root || !snapshot) return;
  const byIndex = () => {
    if (snapshot.index < 0) return null;
    return $$("input, select, textarea", root)[snapshot.index] || null;
  };
  const el = (snapshot.id && root.querySelector(`#${CSS.escape(snapshot.id)}`))
    || (snapshot.testId
      && root.querySelector(`[data-testid="${CSS.escape(snapshot.testId)}"]`))
    || (snapshot.name && root.querySelector(`[name="${CSS.escape(snapshot.name)}"]`))
    || byIndex();
  if (!el || el === document.activeElement) return;
  if (el.disabled || el.readOnly || !isMorphEditableControl(el)) return;
  el.focus({ preventScroll: true });
  if (typeof el.setSelectionRange === "function"
    && snapshot.selectionStart !== null && snapshot.selectionEnd !== null) {
    try {
      el.setSelectionRange(snapshot.selectionStart, snapshot.selectionEnd);
    } catch {
      // Inputs whose type forbids selection (number, email) throw; focus is enough.
    }
  }
  if (snapshot.scrollTop) el.scrollTop = snapshot.scrollTop;
}
