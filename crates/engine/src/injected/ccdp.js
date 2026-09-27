// Built by `cargo xtask build-js` from src/*.ts; do not edit. sources: 2b178705108557c0
"use strict";
(() => {
  // src/dom.ts
  var SKIP = /* @__PURE__ */ new Set(["script", "style", "template", "noscript", "head", "meta", "link", "title"]);
  var norm = (s) => String(s == null ? "" : s).replace(/\s+/g, " ").trim();
  var tag = (el) => el.localName;
  var attr = (el, name) => el.getAttribute(name);
  function childrenOf(el) {
    if (el.shadowRoot) return Array.from(el.shadowRoot.childNodes);
    if (el instanceof HTMLSlotElement) {
      const assigned = el.assignedNodes({ flatten: true });
      return assigned.length ? assigned : Array.from(el.childNodes);
    }
    if (el instanceof HTMLDetailsElement && !el.open) {
      return Array.from(el.children).filter((c) => tag(c) === "summary").slice(0, 1);
    }
    return Array.from(el.childNodes);
  }
  function descendants(scope) {
    const out = [];
    const visit = (node) => {
      if (out.length > 5e5) return;
      if (node instanceof Element) {
        if (node.shadowRoot) visit(node.shadowRoot);
      }
      for (let child = node.firstChild; child; child = child.nextSibling) {
        if (child.nodeType === Node.ELEMENT_NODE) {
          out.push(child);
          visit(child);
        }
      }
    };
    visit(scope);
    return out;
  }
  function parentOf(node) {
    if (node.parentElement) return node.parentElement;
    const parent = node.parentNode;
    if (parent instanceof ShadowRoot) return parent.host;
    return null;
  }
  function visible(el) {
    const box = el.getBoundingClientRect();
    if (box.width <= 0 || box.height <= 0) {
      const style = el.ownerDocument.defaultView?.getComputedStyle(el);
      if (style?.display === "contents") return childrenOf(el).some((c) => c instanceof Element && visible(c));
      return false;
    }
    return el.checkVisibility ? el.checkVisibility({ visibilityProperty: true }) : true;
  }
  function inOrder(els, root) {
    const unique = Array.from(new Set(els));
    if (unique.length < 2) return unique;
    const order = /* @__PURE__ */ new Map();
    descendants(root).forEach((el, i) => order.set(el, i));
    if (root instanceof Element) order.set(root, -1);
    return unique.sort((a, b) => (order.get(a) ?? 1e9) - (order.get(b) ?? 1e9));
  }

  // src/aria.ts
  var INTERACTIVE = /* @__PURE__ */ new Set([
    "button",
    "link",
    "textbox",
    "searchbox",
    "checkbox",
    "radio",
    "switch",
    "combobox",
    "listbox",
    "option",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "tab",
    "slider",
    "spinbutton",
    "treeitem",
    "iframe"
  ]);
  var FROM_CONTENT = /* @__PURE__ */ new Set([
    "button",
    "link",
    "heading",
    "tab",
    "option",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "treeitem",
    "checkbox",
    "radio",
    "switch",
    "cell",
    "gridcell",
    "columnheader",
    "rowheader",
    "tooltip",
    "row"
  ]);
  var KNOWN = /* @__PURE__ */ new Set([
    ...INTERACTIVE,
    ...FROM_CONTENT,
    "alert",
    "alertdialog",
    "application",
    "article",
    "banner",
    "blockquote",
    "caption",
    "code",
    "complementary",
    "contentinfo",
    "definition",
    "deletion",
    "dialog",
    "directory",
    "document",
    "emphasis",
    "feed",
    "figure",
    "form",
    "generic",
    "grid",
    "group",
    "img",
    "image",
    "insertion",
    "list",
    "listitem",
    "log",
    "main",
    "marquee",
    "math",
    "menu",
    "menubar",
    "meter",
    "navigation",
    "none",
    "note",
    "paragraph",
    "presentation",
    "progressbar",
    "radiogroup",
    "region",
    "rowgroup",
    "scrollbar",
    "search",
    "separator",
    "status",
    "strong",
    "subscript",
    "superscript",
    "table",
    "tablist",
    "tabpanel",
    "term",
    "time",
    "timer",
    "toolbar",
    "tree",
    "treegrid"
  ]);
  var INPUT = {
    button: "button",
    submit: "button",
    reset: "button",
    image: "button",
    file: "button",
    checkbox: "checkbox",
    radio: "radio",
    range: "slider",
    number: "spinbutton",
    search: "searchbox",
    color: "button"
  };
  var ariaTrue = (el, name) => attr(el, name) === "true";
  var scoped = (el) => el.closest("article, aside, main, nav, section");
  var styleOf = (el) => el.ownerDocument.defaultView.getComputedStyle(el);
  function implicitRole(el) {
    const t = tag(el);
    switch (t) {
      case "a":
      case "area":
        return el.hasAttribute("href") ? "link" : "generic";
      case "article":
        return "article";
      case "aside":
        return "complementary";
      case "button":
        return "button";
      case "datalist":
        return "listbox";
      case "dd":
        return "definition";
      case "dl":
        return "descriptionlist";
      case "details":
        return "group";
      case "dfn":
      case "dt":
        return "term";
      case "dialog":
        return "dialog";
      case "fieldset":
      case "optgroup":
        return "group";
      case "figure":
        return "figure";
      case "figcaption":
        return "figcaption";
      case "footer":
        return scoped(el.parentElement || el) ? "generic" : "contentinfo";
      case "header":
        return scoped(el.parentElement || el) ? "generic" : "banner";
      case "form":
        return "form";
      case "h1":
      case "h2":
      case "h3":
      case "h4":
      case "h5":
      case "h6":
        return "heading";
      case "hr":
        return "separator";
      case "iframe":
      case "frame":
        return "iframe";
      case "img":
        return attr(el, "alt") === "" && !el.hasAttribute("title") ? "presentation" : "img";
      case "input": {
        const type = (attr(el, "type") || "text").toLowerCase();
        if (type === "hidden") return null;
        if (el.hasAttribute("list") && !INPUT[type]) return "combobox";
        return INPUT[type] || "textbox";
      }
      case "label":
        return "label";
      case "legend":
        return "legend";
      case "li":
        return "listitem";
      case "main":
        return "main";
      case "menu":
      case "ol":
      case "ul":
        return "list";
      case "meter":
        return "meter";
      case "nav":
        return "navigation";
      case "option":
        return "option";
      case "output":
        return "status";
      case "p":
        return "paragraph";
      case "progress":
        return "progressbar";
      case "search":
        return "search";
      case "section":
        return el.hasAttribute("aria-label") || el.hasAttribute("aria-labelledby") ? "region" : "generic";
      case "select": {
        const select = el;
        return select.multiple || select.size > 1 ? "listbox" : "combobox";
      }
      case "summary":
        return "button";
      case "table":
        return "table";
      case "tbody":
      case "thead":
      case "tfoot":
        return "rowgroup";
      case "td":
        return el.closest("table[role=grid], [role=grid]") ? "gridcell" : "cell";
      case "th":
        return attr(el, "scope") === "row" ? "rowheader" : "columnheader";
      case "textarea":
        return "textbox";
      case "tr":
        return "row";
      case "caption":
        return "caption";
      case "blockquote":
        return "blockquote";
      case "code":
        return "code";
      case "em":
        return "emphasis";
      case "strong":
        return "strong";
      case "time":
        return "time";
      case "svg":
        return "img";
      case "math":
        return "math";
      case "html":
      case "body":
        return "none";
      default: {
        const html = el;
        if (html.isContentEditable && !(el.parentElement && el.parentElement.isContentEditable)) return "textbox";
        return "generic";
      }
    }
  }
  function roleOf(el) {
    const explicit = (attr(el, "role") || "").trim().toLowerCase().split(/\s+/).find((r) => KNOWN.has(r));
    if (explicit === "presentation" || explicit === "none") {
      return el.tabIndex >= 0 && el.hasAttribute("tabindex") ? implicitRole(el) : "none";
    }
    if (explicit === "image") return "img";
    return explicit || implicitRole(el);
  }
  function hidden(el, style) {
    return style.display === "none" || ariaTrue(el, "aria-hidden") || el.hasAttribute("inert");
  }
  function hiddenForAria(el) {
    const style = styleOf(el);
    if (style.visibility === "hidden" || style.visibility === "collapse") return true;
    for (let e = el; e; e = parentOf(e)) {
      if (ariaTrue(e, "aria-hidden") || e.hasAttribute("inert")) return true;
      if (styleOf(e).display === "none") return true;
    }
    return false;
  }
  function textOf(node, skip, depth, treeitem) {
    if (depth > 30) return "";
    if (node.nodeType === Node.TEXT_NODE) return node.data;
    if (node.nodeType !== Node.ELEMENT_NODE && node.nodeType !== Node.DOCUMENT_FRAGMENT_NODE) return "";
    if (node.nodeType === Node.ELEMENT_NODE) {
      const el = node;
      if (el === skip || SKIP.has(tag(el))) return "";
      if (treeitem && (attr(el, "role") === "group" || attr(el, "role") === "treeitem")) return "";
      const style = styleOf(el);
      if (hidden(el, style) || style.visibility === "hidden") return "";
      const label = attr(el, "aria-label");
      if (label && label.trim()) return " " + label + " ";
      const t = tag(el);
      if (t === "img" || t === "input" && attr(el, "type") === "image") return " " + (attr(el, "alt") || "") + " ";
      if (t === "input" || t === "textarea") {
        const role = roleOf(el);
        const value = el.value;
        if (role === "textbox" || role === "searchbox" || role === "spinbutton" || role === "slider") return " " + value + " ";
        if (role === "button") return " " + (value || "") + " ";
        return "";
      }
      if (t === "select") {
        const opt = el.selectedOptions[0];
        return opt ? " " + opt.text + " " : "";
      }
      if (t === "br") return " ";
      if (t === "svg") return " " + svgTitle(el) + " ";
      const block = style.display !== "inline" && style.display !== "contents";
      let out2 = "";
      for (const child of childrenOf(el)) out2 += textOf(child, skip, depth + 1, treeitem);
      return block ? " " + out2 + " " : out2;
    }
    let out = "";
    for (const child of Array.from(node.childNodes)) out += textOf(child, skip, depth + 1, treeitem);
    return out;
  }
  function svgTitle(el) {
    const title = Array.from(el.children).find((c) => tag(c) === "title");
    return title ? title.textContent || "" : "";
  }
  function byIds(el, ids) {
    const root = el.getRootNode();
    return ids.split(/\s+/).map((id) => root.getElementById ? root.getElementById(id) : el.ownerDocument.getElementById(id)).filter((e) => !!e);
  }
  function labelsOf(el) {
    const out = [];
    const labelledby = attr(el, "aria-labelledby");
    if (labelledby) out.push(norm(byIds(el, labelledby).map((l) => textOf(l, null, 0)).join(" ")));
    const label = attr(el, "aria-label");
    if (label) out.push(norm(label));
    const labels = el.labels;
    if (labels) for (const l of Array.from(labels)) out.push(norm(textOf(l, el, 0)));
    return out.filter(Boolean);
  }
  function nameOf(el, role) {
    const labelledby = attr(el, "aria-labelledby");
    if (labelledby) {
      const text = norm(byIds(el, labelledby).map((l) => textOf(l, null, 0)).join(" "));
      if (text) return text;
    }
    const label = norm(attr(el, "aria-label"));
    if (label) return label;
    const t = tag(el);
    if (t === "input" || t === "textarea" || t === "select" || t === "meter" || t === "progress" || t === "output") {
      const type = (attr(el, "type") || "").toLowerCase();
      const value = el.value;
      if (t === "input" && ["button", "submit", "reset"].includes(type)) {
        return norm(value) || (type === "submit" ? "Submit" : type === "reset" ? "Reset" : "");
      }
      if (t === "input" && type === "image") return norm(attr(el, "alt")) || "Submit";
      const labels = el.labels ? Array.from(el.labels) : [];
      const text = norm(labels.map((l) => textOf(l, el, 0)).join(" "));
      if (text) return text;
      return norm(attr(el, "title")) || norm(attr(el, "placeholder"));
    }
    if (t === "img" || t === "area") return norm(attr(el, "alt")) || norm(attr(el, "title"));
    if (t === "svg") return norm(svgTitle(el)) || norm(attr(el, "title"));
    if (t === "fieldset") {
      const legend = Array.from(el.children).find((c) => tag(c) === "legend");
      if (legend) return norm(textOf(legend, null, 0));
    }
    if (t === "table") {
      const caption = el.caption;
      if (caption) return norm(textOf(caption, null, 0));
    }
    if (t === "iframe") return norm(attr(el, "title"));
    if (FROM_CONTENT.has(role) && role !== "row") {
      let text = "";
      for (const child of childrenOf(el)) text += textOf(child, null, 1, role === "treeitem");
      text = norm(text);
      if (text) return text;
    }
    return norm(attr(el, "title"));
  }
  function shownName(el, role) {
    if (role === "none" || role === "generic" || role === "paragraph") return norm(attr(el, "aria-label"));
    return nameOf(el, role);
  }
  function valueOf(el, role) {
    const now = attr(el, "aria-valuetext") || attr(el, "aria-valuenow");
    const t = tag(el);
    if (t === "input" || t === "textarea") return el.value;
    if (t === "select") {
      const opt = el.selectedOptions[0];
      return opt ? opt.text : "";
    }
    if (role === "textbox" && el.isContentEditable) return el.innerText;
    return now;
  }
  function activeElement() {
    let a = document.activeElement;
    while (a && a.shadowRoot && a.shadowRoot.activeElement) a = a.shadowRoot.activeElement;
    return a;
  }
  function statesOf(el, role, active) {
    const s = {};
    const t = tag(el);
    const input = el;
    if (t === "input" && (input.type === "checkbox" || input.type === "radio")) {
      s.c = input.indeterminate ? "mixed" : input.checked ? "true" : "false";
    } else if (["checkbox", "radio", "switch", "menuitemcheckbox", "menuitemradio"].includes(role)) {
      const c = attr(el, "aria-checked");
      s.c = c === "true" || c === "mixed" ? c : "false";
    }
    if (el.hasAttribute("aria-expanded")) s.x = ariaTrue(el, "aria-expanded");
    else if (t === "summary" && el.parentElement && tag(el.parentElement) === "details") s.x = el.parentElement.open;
    else if (t === "select" && role === "combobox") s.x = false;
    if (t === "option" ? el.selected : ariaTrue(el, "aria-selected")) s.s = 1;
    if (el.matches && el.matches(":disabled") || el.closest('[aria-disabled="true"]')) s.d = 1;
    if (input.readOnly || ariaTrue(el, "aria-readonly")) s.r = 1;
    if (input.required || ariaTrue(el, "aria-required")) s.q = 1;
    const invalid = attr(el, "aria-invalid");
    if (invalid && invalid !== "false") s.i = 1;
    if (el === active && el !== document.body) s.f = 1;
    if (ariaTrue(el, "aria-pressed")) s.p = 1;
    if (attr(el, "aria-pressed") === "mixed") s.pm = 1;
    return s;
  }
  function level(el, role) {
    if (role !== "heading") return void 0;
    const aria = parseInt(attr(el, "aria-level") || "", 10);
    if (aria > 0) return aria;
    const m = /^h([1-6])$/.exec(tag(el));
    return m ? Number(m[1]) : 2;
  }

  // src/errors.ts
  var LocatorError = class extends Error {
    constructor(message) {
      super(message);
      this.name = "LocatorError";
    }
  };

  // src/css.ts
  function split(text, sep) {
    const parts = [];
    let depth = 0;
    let quote = "";
    let current = "";
    for (let i = 0; i < text.length; i++) {
      const c = text[i];
      if (quote) {
        current += c;
        if (c === "\\") current += text[++i] ?? "";
        else if (c === quote) quote = "";
        continue;
      }
      if (c === '"' || c === "'") quote = c;
      else if (c === "[" || c === "(") depth++;
      else if (c === "]" || c === ")") depth--;
      else if (c === "\\") {
        current += c + (text[++i] ?? "");
        continue;
      }
      const isSep = depth === 0 && (sep === " " ? /\s/.test(c) : c === sep);
      if (!isSep) {
        current += c;
        continue;
      }
      if (sep === " ") {
        let j = i;
        while (j + 1 < text.length && /\s/.test(text[j + 1])) j++;
        const next = text[j + 1] ?? "";
        const prev = current.trimEnd().slice(-1);
        i = j;
        if (!current.trim() || ">+~".includes(next) || ">+~".includes(prev) || next === "") {
          current += " ";
          continue;
        }
      }
      parts.push(current.trim());
      current = "";
    }
    parts.push(current.trim());
    return parts.filter(Boolean);
  }
  function shadowRoots(scope) {
    const out = [];
    const visit = (root) => {
      if (root instanceof Element && root.shadowRoot) {
        out.push(root.shadowRoot);
        visit(root.shadowRoot);
      }
      for (const el of Array.from(root.querySelectorAll("*"))) {
        if (el.shadowRoot) {
          out.push(el.shadowRoot);
          visit(el.shadowRoot);
        }
      }
    };
    visit(scope);
    return out;
  }
  function ancestorsMatch(el, parts) {
    let j = parts.length - 1;
    for (let a = parentOf(el); a && j >= 0; a = parentOf(a)) {
      if (a.matches(parts[j])) j--;
    }
    return j < 0;
  }
  function css(selector, scope) {
    let light;
    try {
      light = Array.from(scope.querySelectorAll(selector));
    } catch (e) {
      throw new LocatorError(`${selector} is not a valid CSS selector`);
    }
    const roots = shadowRoots(scope);
    if (!roots.length) return light;
    const out = new Set(light);
    for (const alternative of split(selector, ",")) {
      const parts = split(alternative, " ");
      const last = parts.pop();
      if (!last) continue;
      for (const root of roots) {
        for (const el of Array.from(root.querySelectorAll(last))) {
          if (ancestorsMatch(el, parts)) out.add(el);
        }
      }
    }
    return Array.from(out);
  }

  // src/text.ts
  function matcher(m) {
    if (m.mode === "regex") {
      let re;
      try {
        re = new RegExp(m.value, (m.flags || "").replace(/[gy]/g, ""));
      } catch (e) {
        throw new LocatorError(`/${m.value}/${m.flags || ""} is not a valid regex: ${e.message}`);
      }
      return (text) => re.test(norm(text));
    }
    const want = norm(m.value);
    if (m.mode === "exact") return (text) => norm(text) === want;
    const lower = want.toLowerCase();
    return (text) => norm(text).toLowerCase().includes(lower);
  }
  var Texts = class {
    cache = /* @__PURE__ */ new Map();
    of(el) {
      let text = this.cache.get(el);
      if (text !== void 0) return text;
      text = this.compute(el);
      this.cache.set(el, text);
      return text;
    }
    compute(el) {
      if (SKIP.has(tag(el))) return "";
      if (tag(el) === "input") {
        const input = el;
        return ["button", "submit", "reset"].includes(input.type) ? input.value : "";
      }
      let out = "";
      for (const child of childrenOf(el)) {
        if (child.nodeType === Node.TEXT_NODE) out += child.data;
        else if (child instanceof Element && shown(child)) out += tag(child) === "br" ? "\n" : this.of(child);
      }
      return out;
    }
  };
  function shown(el) {
    const style = el.ownerDocument.defaultView.getComputedStyle(el);
    return style.display !== "none" && style.visibility !== "hidden" && style.visibility !== "collapse";
  }

  // src/locator.ts
  function xpath(expression, scope) {
    const expr = scope instanceof Document || !expression.startsWith("/") ? expression : "." + expression;
    let found;
    try {
      found = document.evaluate(expr, scope, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);
    } catch (e) {
      throw new LocatorError(`${expression} is not a valid XPath expression`);
    }
    const out = [];
    for (let i = 0; i < found.snapshotLength; i++) {
      const node = found.snapshotItem(i);
      if (node instanceof Element) out.push(node);
    }
    return out;
  }
  function byText(m, scope, texts) {
    const test = matcher(m);
    const matches = (el) => test(texts.of(el));
    const out = [];
    for (const el of descendants(scope)) {
      if (!matches(el) || !visible(el)) continue;
      const kids = Array.from((el.shadowRoot || el).children);
      if (kids.some((c) => matches(c) && visible(c))) continue;
      out.push(el);
    }
    return out;
  }
  function byRole(step, scope) {
    const name = step.name ? matcher(step.name) : null;
    const active = activeElement();
    const out = [];
    for (const el of descendants(scope)) {
      const role = roleOf(el);
      if (role !== step.role) continue;
      if (!step.includeHidden && hiddenForAria(el)) continue;
      if (step.checked !== void 0 || step.disabled !== void 0 || step.expanded !== void 0 || step.pressed !== void 0 || step.selected !== void 0) {
        const s = statesOf(el, role, active);
        if (step.checked !== void 0 && (s.c || "false") !== step.checked) continue;
        if (step.disabled !== void 0 && !!s.d !== step.disabled) continue;
        if (step.expanded !== void 0 && s.x !== step.expanded) continue;
        if (step.selected !== void 0 && !!s.s !== step.selected) continue;
        if (step.pressed !== void 0 && (s.pm ? "mixed" : s.p ? "true" : "false") !== step.pressed) continue;
      }
      if (step.level !== void 0 && level(el, role) !== step.level) continue;
      if (name && !name(nameOf(el, role))) continue;
      out.push(el);
    }
    return out;
  }
  function byAttribute(name, m, scope) {
    const test = matcher(m);
    return descendants(scope).filter((el) => {
      const value = el.getAttribute(name);
      return value !== null && test(value);
    });
  }
  function find(step, scope, texts) {
    switch (step.kind) {
      case "css":
        return css(step.value, scope);
      case "xpath":
        return xpath(step.value, scope);
      case "text":
        return byText(step.text, scope, texts);
      case "role":
        return byRole(step, scope);
      case "label": {
        const test = matcher(step.text);
        return descendants(scope).filter((el) => labelsOf(el).some(test));
      }
      case "placeholder":
        return byAttribute("placeholder", step.text, scope);
      case "alt":
        return byAttribute("alt", step.text, scope);
      case "title":
        return byAttribute("title", step.text, scope);
      case "testid":
        return descendants(scope).filter((el) => el.getAttribute(step.attribute) === step.value);
      default:
        return [];
    }
  }
  function query(steps, root) {
    if (root && !root.isConnected) return [];
    const top = root || document;
    const texts = new Texts();
    let current = [top];
    for (const step of steps) {
      const els = current.filter((s) => s instanceof Element);
      switch (step.kind) {
        case "nth": {
          const i = step.index < 0 ? els.length + step.index : step.index;
          current = i >= 0 && i < els.length ? [els[i]] : [];
          break;
        }
        case "visible":
          current = els.filter((el) => visible(el) === step.visible);
          break;
        case "has-text": {
          const test = matcher(step.text);
          current = els.filter((el) => test(texts.of(el)));
          break;
        }
        default: {
          const found = [];
          for (const scope of current) found.push(...find(step, scope, texts));
          current = current.length > 1 ? inOrder(found, top) : found;
        }
      }
    }
    return current.filter((s) => s instanceof Element);
  }
  var CONTEXT_ROLES = /* @__PURE__ */ new Set([
    "row",
    "listitem",
    "article",
    "dialog",
    "alertdialog",
    "form",
    "region",
    "navigation",
    "group",
    "tabpanel",
    "menu",
    "listbox",
    "radiogroup",
    "tablist",
    "toolbar",
    "banner",
    "contentinfo",
    "complementary",
    "main",
    "search",
    "table",
    "list"
  ]);
  var cut = (s, n) => s.length > n ? s.slice(0, n - 1) + "…" : s;
  function describe(el) {
    const role = roleOf(el) || "generic";
    const name = cut(shownName(el, role) || (role === "generic" ? norm(el.textContent) : ""), 60);
    const out = { role, name, tag: tag(el), visible: visible(el) };
    for (let a = parentOf(el); a; a = parentOf(a)) {
      const r = roleOf(a);
      if (!r || !CONTEXT_ROLES.has(r)) continue;
      const n = cut(r === "row" || r === "listitem" ? norm(textOf(a, null, 0)) : nameOf(a, r), 40);
      if (!n) continue;
      out.context = `${r} "${n.replace(/"/g, '\\"')}"`;
      break;
    }
    return out;
  }
  var LISTED = 5;
  function one(steps, root) {
    const found = query(steps, root);
    if (found.length === 1) return found[0];
    const els = found.slice(0, LISTED);
    return { json: JSON.stringify({ count: found.length, candidates: els.map(describe) }), els };
  }

  // src/walker.ts
  function scrolls(el, style) {
    if (tag(el) === "html" || tag(el) === "body") return false;
    const over = (v) => v === "auto" || v === "scroll" || v === "overlay";
    return over(style.overflowY) && el.scrollHeight - el.clientHeight > 1 || over(style.overflowX) && el.scrollWidth - el.clientWidth > 1;
  }
  function walk(scope) {
    const view = document.defaultView;
    const nodes = [];
    const els = [];
    const active = activeElement();
    function visit(node, parent, parentStyle, invisible, mute) {
      if (nodes.length > 2e5) return;
      if (node.nodeType === Node.TEXT_NODE) {
        const data = node.data;
        if (!invisible && !mute && data) nodes.push({ p: parent, t: 1, n: data });
        return;
      }
      if (node.nodeType !== Node.ELEMENT_NODE) return;
      const el = node;
      if (SKIP.has(tag(el))) return;
      if (tag(el) === "br") {
        if (!invisible && !mute) nodes.push({ p: parent, t: 1, n: "\n" });
        return;
      }
      const style = view.getComputedStyle(el);
      if (hidden(el, style)) return;
      const role = roleOf(el);
      if (role === null) return;
      const vis = style.visibility !== "hidden" && style.visibility !== "collapse";
      const index = nodes.length;
      if (!vis) {
        for (const child of childrenOf(el)) visit(child, parent, style, true, mute);
        return;
      }
      const n = { p: parent, r: role };
      const name = shownName(el, role);
      if (name) n.n = name;
      const value = valueOf(el, role);
      if (value != null && value !== "") n.v = String(value);
      const lv = level(el, role);
      if (lv) n.l = lv;
      const href = el.href;
      if (role === "link" && href) {
        const svg = href;
        n.h = String(svg.baseVal !== void 0 ? svg.baseVal : href);
      }
      if (tag(el) === "input" && el.type === "password") n.w = 1;
      const s = statesOf(el, role, active);
      delete s.pm;
      if (Object.keys(s).length) n.s = s;
      if (style.display === "inline" || style.display === "contents") n.i = 1;
      const tabindex = parseInt(attr(el, "tabindex") || "", 10);
      const pointerRoot = style.cursor === "pointer" && !(parentStyle && parentStyle.cursor === "pointer");
      if (tag(el) !== "html" && tag(el) !== "body" && (el.hasAttribute("onclick") || tabindex >= 0 || pointerRoot)) n.c = 1;
      if (scrolls(el, style)) n.x = 1;
      const long = el.childElementCount > 30 || Array.from(el.children).some((c) => c.childElementCount > 30);
      if (INTERACTIVE.has(role) || n.c || n.x || el === scope || role === "textbox" || long) {
        n.e = els.length;
        els.push(el);
      }
      nodes.push(n);
      if (role === "iframe" || tag(el) === "svg" || role === "img") return;
      const muted = mute || tag(el) === "label" && !!el.control;
      for (const child of childrenOf(el)) visit(child, index, style, false, muted);
    }
    if (document.documentElement) visit(document.documentElement, -1, null, false, false);
    return {
      json: JSON.stringify({ title: document.title, url: location.href, nodes }),
      els
    };
  }

  // src/index.ts
  var api = { version: 1, query, one, describe, walk };
  if (!globalThis.__ccdp) globalThis.__ccdp = api;
})();
