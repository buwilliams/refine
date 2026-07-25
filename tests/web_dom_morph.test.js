const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

// `dom-morph.js` is all function declarations, so it loads into a bare context.
// The predicates under test read only element properties, which lets real
// browser semantics be described with plain objects.
function morphRuntime() {
  const context = vm.createContext({ document: { activeElement: null } });
  const source = fs.readFileSync(
    path.join(__dirname, "../src/surfaces/web/static/js/dom-morph.js"),
    "utf8",
  );
  vm.runInContext(source, context);
  vm.runInContext(`
    globalThis.morphTest = {
      isUserEdited: isUserEditedControl,
      shouldPreserve: shouldPreserveDuringMorph,
      isEditable: isMorphEditableControl,
      bindOnce,
      preserve: preserveDuringMorph,
      release: releaseAfterMorph,
      isPreserved: isPreservedDuringMorph,
    };
  `, context);
  return context.morphTest;
}

// Stands in for a DOM element: records the listeners actually attached.
const listenerSpy = () => ({
  nodeType: 1,
  tagName: "BUTTON",
  attached: [],
  addEventListener(event, handler) {
    this.attached.push({ event, handler });
  },
});

const input = (props = {}) => ({
  nodeType: 1, tagName: "INPUT", type: "text", value: "", defaultValue: "", ...props,
});
const checkbox = (props = {}) => ({
  nodeType: 1, tagName: "INPUT", type: "checkbox", checked: false, defaultChecked: false, ...props,
});
const textarea = (props = {}) => ({
  nodeType: 1, tagName: "TEXTAREA", value: "", defaultValue: "", ...props,
});
const select = (value, options) => ({
  nodeType: 1, tagName: "SELECT", value, options,
});

test("a control matching its rendered value is not treated as edited", () => {
  const { isUserEdited } = morphRuntime();

  assert.equal(isUserEdited(input({ value: "4", defaultValue: "4" })), false);
  assert.equal(isUserEdited(checkbox({ checked: true, defaultChecked: true })), false);
  assert.equal(isUserEdited(textarea({ value: "notes", defaultValue: "notes" })), false);
  assert.equal(
    isUserEdited(select("qa", [
      { value: "default", defaultSelected: false },
      { value: "qa", defaultSelected: true },
    ])),
    false,
  );
});

test("a control the user changed is treated as edited", () => {
  const { isUserEdited } = morphRuntime();

  assert.equal(isUserEdited(input({ value: "12", defaultValue: "4" })), true);
  assert.equal(isUserEdited(checkbox({ checked: true, defaultChecked: false })), true);
  assert.equal(isUserEdited(checkbox({ checked: false, defaultChecked: true })), true);
  assert.equal(isUserEdited(textarea({ value: "mine", defaultValue: "server" })), true);
  assert.equal(
    isUserEdited(select("qa", [
      { value: "default", defaultSelected: true },
      { value: "qa", defaultSelected: false },
    ])),
    true,
  );
});

// A save round-trip re-renders the control with the saved value as its
// attribute, which is what clears the edited state — there is no flag to reset.
test("an edited control goes clean once its saved value is rendered back", () => {
  const { isUserEdited } = morphRuntime();

  const field = input({ value: "12", defaultValue: "4" });
  assert.equal(isUserEdited(field), true);
  field.defaultValue = "12";
  assert.equal(isUserEdited(field), false);
});

test("file and command inputs are never withheld from a redraw", () => {
  const { isUserEdited } = morphRuntime();

  assert.equal(isUserEdited(input({ type: "file", value: "a.txt", defaultValue: "" })), false);
  assert.equal(isUserEdited(input({ type: "button", value: "Go", defaultValue: "" })), false);
  assert.equal(isUserEdited(input({ type: "submit", value: "Save", defaultValue: "" })), false);
});

test("a redraw preserves unsaved edits and the focused control", () => {
  const { shouldPreserve } = morphRuntime();

  const edited = input({ value: "12", defaultValue: "4" });
  const clean = input({ value: "4", defaultValue: "4" });

  // Unsaved edit is protected even when the user has moved on to another field.
  assert.equal(shouldPreserve(edited, clean), true);
  // The control the caret sits in is protected even before it is edited, so a
  // server value cannot swap out from under the cursor.
  assert.equal(shouldPreserve(clean, clean), true);
  // Everything else takes the server's value, which is what lets unrelated
  // fields in the same tab keep updating live.
  assert.equal(shouldPreserve(clean, edited), false);
  assert.equal(shouldPreserve(clean, null), false);
});

test("only form controls are withheld from a redraw", () => {
  const { shouldPreserve, isEditable } = morphRuntime();

  const div = { nodeType: 1, tagName: "DIV" };
  const button = { nodeType: 1, tagName: "BUTTON" };
  const textNode = { nodeType: 3, tagName: undefined, value: "x", defaultValue: "y" };

  assert.equal(isEditable(div), false);
  assert.equal(isEditable(button), false);
  assert.equal(isEditable(input()), true);
  assert.equal(isEditable(textarea()), true);
  assert.equal(isEditable(select("", [])), true);

  // A focused non-control must not block its own subtree from updating.
  assert.equal(shouldPreserve(div, div), false);
  assert.equal(shouldPreserve(button, button), false);
  assert.equal(shouldPreserve(textNode, null), false);
  assert.equal(shouldPreserve(null, null), false);
});

// Screens re-run their bind step after every redraw. Nodes the morph kept must
// not collect a second copy of a handler, or one click would fire the command
// once per refresh that had happened since the screen was opened.
test("a surviving element is bound once no matter how many redraws bind it", () => {
  const { bindOnce } = morphRuntime();
  const button = listenerSpy();

  bindOnce(button, "click", () => {});
  bindOnce(button, "click", () => {});
  bindOnce(button, "click", () => {});

  assert.equal(button.attached.length, 1);
});

test("each event on an element binds separately", () => {
  const { bindOnce } = morphRuntime();
  const control = listenerSpy();

  bindOnce(control, "mousedown", () => {});
  bindOnce(control, "click", () => {});
  bindOnce(control, "keydown", () => {});
  bindOnce(control, "click", () => {});

  assert.deepEqual(control.attached.map((entry) => entry.event), [
    "mousedown",
    "click",
    "keydown",
  ]);
});

// Two distinct handlers for the same event on one element need distinct keys.
test("an explicit key allows a second handler for the same event", () => {
  const { bindOnce } = morphRuntime();
  const control = listenerSpy();

  bindOnce(control, "change", () => {}, "autosave");
  bindOnce(control, "change", () => {}, "preview");
  bindOnce(control, "change", () => {}, "autosave");

  assert.equal(control.attached.length, 2);
});

// Nodes the morph introduced have no binding history, so they must get bound.
test("a newly rendered element is bound even after its predecessor was", () => {
  const { bindOnce } = morphRuntime();
  const before = listenerSpy();
  const after = listenerSpy();

  bindOnce(before, "click", () => {});
  bindOnce(after, "click", () => {});

  assert.equal(before.attached.length, 1);
  assert.equal(after.attached.length, 1);
});

test("binding a missing element is a no-op", () => {
  const { bindOnce } = morphRuntime();

  assert.doesNotThrow(() => bindOnce(null, "click", () => {}));
  assert.doesNotThrow(() => bindOnce(undefined, "click", () => {}));
});

// An inline editor's open state lives only in the DOM: the rendered HTML always
// describes the closed field, so a refresh would collapse it mid-edit. The mark
// claims the whole subtree, which is why it is checked before the control tests.
test("a subtree claimed by an open editor is withheld from a redraw", () => {
  const { shouldPreserve, preserve, release, isPreserved } = morphRuntime();
  const field = { nodeType: 1, tagName: "DIV", dataset: {} };

  assert.equal(isPreserved(field), false);
  assert.equal(shouldPreserve(field, null), false);

  preserve(field);
  assert.equal(isPreserved(field), true);
  // A plain wrapper, not a form control, and not focused — the mark alone holds it.
  assert.equal(shouldPreserve(field, null), true);

  release(field);
  assert.equal(isPreserved(field), false);
  assert.equal(shouldPreserve(field, null), false);
});

test("the preserve helpers tolerate a missing element", () => {
  const { preserve, release, isPreserved } = morphRuntime();

  assert.doesNotThrow(() => preserve(null));
  assert.doesNotThrow(() => release(null));
  assert.equal(isPreserved(null), false);
  assert.equal(isPreserved({ nodeType: 1, tagName: "DIV" }), false);
});
