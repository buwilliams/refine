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
    };
  `, context);
  return context.morphTest;
}

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
