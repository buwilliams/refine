// ---- DOM morphing -----------------------------------------------------------
//
// Screens render HTML strings and swap them in with `innerHTML`. That destroys
// and rebuilds every node, which drops focus, caret position, scroll offset, and
// every attached listener — so any background refresh (SSE, polling) interrupts
// whatever the user was editing mid-keystroke.
//
// Morphing updates the existing tree in place instead: nodes that did not change
// keep their identity, so their listeners and the user's focus survive. Only the
// parts that actually differ are touched.
//
// Idiomorph still syncs form values onto the nodes it keeps, which would replace
// an in-progress edit with the server's value. `shouldPreserveDuringMorph`
// decides which controls a redraw is not allowed to touch.

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

function shouldPreserveDuringMorph(el, activeEl) {
  if (!el || el.nodeType !== 1) return false;
  if (!isMorphEditableControl(el)) return false;
  // Never yank the caret out of the control the user is typing in, even if they
  // have not changed it yet — a value swap under the cursor is just as jarring.
  if (activeEl && el === activeEl) return true;
  return isUserEditedControl(el);
}

// Morph `root`'s children to `html`.
//
// Returns `{ structural }` — whether the morph added, removed, or replaced any
// node. When nothing structural changed, every surviving node still carries the
// listeners it was bound with, so callers must not re-bind (that would attach a
// second copy of every handler). When it did, new nodes need binding.
function morphChildren(root, html) {
  let structural = false;
  const activeEl = document.activeElement;
  Idiomorph.morph(root, html, {
    morphStyle: "innerHTML",
    callbacks: {
      beforeNodeMorphed: (fromEl) => !shouldPreserveDuringMorph(fromEl, activeEl),
      afterNodeAdded: () => { structural = true; },
      beforeNodeRemoved: () => { structural = true; return true; },
    },
  });
  return { structural };
}

// Focus, caret, and scroll offset of whatever the user is in, so a redraw that
// does have to replace nodes can put them back where they were.
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
  if (!el || el.disabled || el.readOnly || !isMorphEditableControl(el)) return;
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
