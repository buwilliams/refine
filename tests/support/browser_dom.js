"use strict";

const VOID_TAGS = new Set(["input", "br", "hr", "img", "meta", "link"]);

class BrowserEvent {
  constructor(type, init = {}) {
    this.type = type;
    this.bubbles = init.bubbles !== false;
    this.key = init.key || "";
    this.ctrlKey = !!init.ctrlKey;
    this.metaKey = !!init.metaKey;
    this.defaultPrevented = false;
    this.propagationStopped = false;
    this.target = null;
    this.currentTarget = null;
  }

  preventDefault() { this.defaultPrevented = true; }
  stopPropagation() { this.propagationStopped = true; }
}

class BrowserElement {
  constructor(tagName, ownerDocument) {
    this.tagName = tagName.toUpperCase();
    this.ownerDocument = ownerDocument;
    this.parentElement = null;
    this.children = [];
    this.attributes = new Map();
    this.listeners = new Map();
    this._ownText = "";
    this._value = undefined;
    this._initialValue = "";
    this.dataset = new Proxy({}, {
      get: (_, key) => this.getAttribute(`data-${camelToKebab(String(key))}`),
      set: (_, key, value) => {
        this.setAttribute(`data-${camelToKebab(String(key))}`, String(value));
        return true;
      },
    });
    this.classList = {
      toggle: (name, enabled) => {
        const classes = new Set(this.className.split(/\s+/).filter(Boolean));
        if (enabled) classes.add(name);
        else classes.delete(name);
        this.className = [...classes].join(" ");
      },
    };
  }

  setAttribute(name, value = "") { this.attributes.set(name, String(value)); }
  getAttribute(name) { return this.attributes.has(name) ? this.attributes.get(name) : null; }
  hasAttribute(name) { return this.attributes.has(name); }
  removeAttribute(name) { this.attributes.delete(name); }

  get id() { return this.getAttribute("id") || ""; }
  set id(value) { this.setAttribute("id", value); }
  get className() { return this.getAttribute("class") || ""; }
  set className(value) { this.setAttribute("class", value); }
  get hidden() { return this.hasAttribute("hidden"); }
  set hidden(value) { if (value) this.setAttribute("hidden", ""); else this.removeAttribute("hidden"); }
  get disabled() { return this.hasAttribute("disabled"); }
  set disabled(value) { if (value) this.setAttribute("disabled", ""); else this.removeAttribute("disabled"); }

  get value() {
    if (this.tagName === "SELECT") {
      if (this._value !== undefined) return this._value;
      return this.children.find((child) => child.hasAttribute("selected"))?.getAttribute("value")
        || this.children[0]?.getAttribute("value") || "";
    }
    return this._value === undefined ? (this.getAttribute("value") || "") : this._value;
  }

  set value(value) { this._value = String(value); }

  get textContent() {
    return this._ownText + this.children.map((child) => child.textContent).join("");
  }

  set textContent(value) {
    this.children = [];
    this._ownText = String(value);
  }

  get innerHTML() { return this._innerHTML || ""; }
  set innerHTML(markup) {
    this.children = [];
    this._ownText = "";
    this._innerHTML = String(markup);
    if (this.tagName === "SELECT") this._value = undefined;
    parseInto(this, String(markup), this.ownerDocument);
    initializeControls(this);
    if (this.tagName === "SELECT") {
      this._value = this.value;
      this._initialValue = this.value;
    }
  }

  get elements() {
    if (this.tagName !== "FORM") return undefined;
    return new Proxy({}, {
      get: (_, name) => this.querySelector(`[name="${String(name)}"]`),
    });
  }

  appendChild(child) {
    child.parentElement = this;
    this.children.push(child);
    return child;
  }

  remove() {
    if (!this.parentElement) return;
    this.parentElement.children = this.parentElement.children.filter((child) => child !== this);
    this.parentElement = null;
  }

  get isConnected() {
    let current = this;
    while (current) {
      if (current === this.ownerDocument.body) return true;
      current = current.parentElement;
    }
    return false;
  }

  addEventListener(type, listener) {
    if (!this.listeners.has(type)) this.listeners.set(type, []);
    this.listeners.get(type).push(listener);
  }

  dispatchEvent(event) {
    if (!(event instanceof BrowserEvent)) event = new BrowserEvent(event.type || event, event);
    if (!event.target) event.target = this;
    let current = this;
    do {
      event.currentTarget = current;
      for (const listener of current.listeners.get(event.type) || []) listener.call(current, event);
      current = event.bubbles && !event.propagationStopped ? current.parentElement : null;
    } while (current);
    return !event.defaultPrevented;
  }

  click() {
    if (!this.disabled) this.dispatchEvent(new BrowserEvent("click"));
  }

  requestSubmit() { this.dispatchEvent(new BrowserEvent("submit")); }

  reset() {
    for (const control of this.querySelectorAll("input, textarea, select")) {
      control._value = control._initialValue;
    }
  }

  focus() { this.ownerDocument.activeElement = this; }
  scrollIntoView() {}

  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }

  querySelectorAll(selector) {
    const selectors = selector.split(",").map((value) => value.trim());
    const found = [];
    const visit = (node) => {
      for (const child of node.children) {
        if (selectors.some((candidate) => matchesSelector(child, candidate))) found.push(child);
        visit(child);
      }
    };
    visit(this);
    return found;
  }
}

class BrowserDocument {
  constructor() {
    this.activeElement = null;
    this.body = new BrowserElement("body", this);
  }

  createElement(tagName) { return new BrowserElement(tagName, this); }
  querySelector(selector) { return this.body.querySelector(selector); }
}

function createBrowserDom(markup) {
  const document = new BrowserDocument();
  const root = document.createElement("div");
  document.body.appendChild(root);
  root.innerHTML = markup;
  return { document, root, BrowserEvent };
}

function parseInto(parent, markup, document) {
  const stack = [parent];
  const tokens = markup.match(/<!--[\s\S]*?-->|<[^>]+>|[^<]+/g) || [];
  for (const token of tokens) {
    if (token.startsWith("<!--")) continue;
    if (token.startsWith("</")) {
      if (stack.length > 1) stack.pop();
      continue;
    }
    if (!token.startsWith("<")) {
      stack[stack.length - 1]._ownText += decodeEntities(token);
      continue;
    }
    const match = token.match(/^<\s*([\w-]+)/);
    if (!match) continue;
    const element = document.createElement(match[1]);
    const attributeSource = token.slice(match[0].length, token.length - (token.endsWith("/>") ? 2 : 1));
    const attributePattern = /([^\s=/>]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g;
    for (const attribute of attributeSource.matchAll(attributePattern)) {
      element.setAttribute(attribute[1], decodeEntities(attribute[2] ?? attribute[3] ?? attribute[4] ?? ""));
    }
    stack[stack.length - 1].appendChild(element);
    if (!VOID_TAGS.has(match[1].toLowerCase()) && !token.endsWith("/>")) stack.push(element);
  }
}

function initializeControls(root) {
  for (const control of root.querySelectorAll("input, textarea, select")) {
    control._value = control.value;
    control._initialValue = control.value;
  }
}

function matchesSelector(element, selector) {
  const simple = selector.trim().split(/\s+/).pop();
  const tag = simple.match(/^[a-zA-Z][\w-]*/)?.[0];
  if (tag && element.tagName !== tag.toUpperCase()) return false;
  const id = simple.match(/#([\w-]+)/)?.[1];
  if (id && element.id !== id) return false;
  for (const className of [...simple.matchAll(/\.([\w-]+)/g)].map((match) => match[1])) {
    if (!element.className.split(/\s+/).includes(className)) return false;
  }
  for (const attribute of simple.matchAll(/\[([^\]=]+)(?:=(?:"([^"]*)"|'([^']*)'|([^\]]+)))?\]/g)) {
    if (!element.hasAttribute(attribute[1])) return false;
    const expected = attribute[2] ?? attribute[3] ?? attribute[4];
    if (expected !== undefined && element.getAttribute(attribute[1]) !== expected) return false;
  }
  return true;
}

function computedStyle(element, css, viewportWidth) {
  const style = {};
  collectCssRules(css, viewportWidth, null, (selector, declarations) => {
    if (selector.split(",").some((candidate) => matchesSelector(element, candidate.trim()))) {
      Object.assign(style, declarations);
    }
  });
  return style;
}

function collectCssRules(css, viewportWidth, inheritedMaxWidth, visit) {
  let cursor = 0;
  while (cursor < css.length) {
    const open = css.indexOf("{", cursor);
    if (open < 0) break;
    const header = css.slice(cursor, open).trim().replace(/^.*?\*\//s, "").trim();
    let depth = 1;
    let close = open + 1;
    while (close < css.length && depth > 0) {
      if (css[close] === "{") depth += 1;
      if (css[close] === "}") depth -= 1;
      close += 1;
    }
    const body = css.slice(open + 1, close - 1);
    if (header.startsWith("@media")) {
      const mediaMax = Number(header.match(/max-width:\s*(\d+)px/)?.[1] || Infinity);
      if (viewportWidth <= mediaMax && (inheritedMaxWidth === null || viewportWidth <= inheritedMaxWidth)) {
        collectCssRules(body, viewportWidth, mediaMax, visit);
      }
    } else if (!header.startsWith("@")) {
      const declarations = {};
      for (const declaration of body.split(";")) {
        const separator = declaration.indexOf(":");
        if (separator < 0) continue;
        declarations[declaration.slice(0, separator).trim()] = declaration.slice(separator + 1).trim();
      }
      visit(header, declarations);
    }
    cursor = close;
  }
}

function camelToKebab(value) { return value.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`); }
function decodeEntities(value) {
  return value.replaceAll("&quot;", '"').replaceAll("&lt;", "<").replaceAll("&gt;", ">").replaceAll("&amp;", "&");
}

module.exports = { BrowserEvent, computedStyle, createBrowserDom };
