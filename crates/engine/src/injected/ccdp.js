// Built by `cargo xtask build-js` from src/*.ts; do not edit. sources: 06fd02fde0d0c453
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
    "colorwell",
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
    color: "colorwell"
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
        const html2 = el;
        if (html2.isContentEditable && !(el.parentElement && el.parentElement.isContentEditable)) return "textbox";
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
  function openModal(doc) {
    return doc.querySelector("dialog:modal");
  }
  function outsideModal(el, modal) {
    return !!modal && !modal.contains(el) && !el.contains(modal);
  }
  function hiddenForAria(el) {
    const style = styleOf(el);
    if (style.visibility === "hidden" || style.visibility === "collapse") return true;
    if (outsideModal(el, openModal(el.ownerDocument))) return true;
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
        const value2 = el.value;
        if (role === "textbox" || role === "searchbox" || role === "spinbutton" || role === "slider") return " " + value2 + " ";
        if (role === "button") return " " + (value2 || "") + " ";
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
      const text2 = norm(byIds(el, labelledby).map((l) => textOf(l, null, 0)).join(" "));
      if (text2) return text2;
    }
    const label = norm(attr(el, "aria-label"));
    if (label) return label;
    const t = tag(el);
    if (t === "input" || t === "textarea" || t === "select" || t === "meter" || t === "progress" || t === "output") {
      const type = (attr(el, "type") || "").toLowerCase();
      const value2 = el.value;
      if (t === "input" && ["button", "submit", "reset"].includes(type)) {
        return norm(value2) || (type === "submit" ? "Submit" : type === "reset" ? "Reset" : "");
      }
      if (t === "input" && type === "image") return norm(attr(el, "alt")) || "Submit";
      const labels = el.labels ? Array.from(el.labels) : [];
      const text2 = norm(labels.map((l) => textOf(l, el, 0)).join(" "));
      if (text2) return text2;
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
      let text2 = "";
      for (const child of childrenOf(el)) text2 += textOf(child, null, 1, role === "treeitem");
      text2 = norm(text2);
      if (text2) return text2;
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
  function split(text2, sep) {
    const parts = [];
    let depth = 0;
    let quote = "";
    let current = "";
    for (let i = 0; i < text2.length; i++) {
      const c = text2[i];
      if (quote) {
        current += c;
        if (c === "\\") current += text2[++i] ?? "";
        else if (c === quote) quote = "";
        continue;
      }
      if (c === '"' || c === "'") quote = c;
      else if (c === "[" || c === "(") depth++;
      else if (c === "]" || c === ")") depth--;
      else if (c === "\\") {
        current += c + (text2[++i] ?? "");
        continue;
      }
      const isSep = depth === 0 && (sep === " " ? /\s/.test(c) : c === sep);
      if (!isSep) {
        current += c;
        continue;
      }
      if (sep === " ") {
        let j = i;
        while (j + 1 < text2.length && /\s/.test(text2[j + 1])) j++;
        const next = text2[j + 1] ?? "";
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
      return (text2) => re.test(norm(text2));
    }
    const want = norm(m.value);
    if (m.mode === "exact") return (text2) => norm(text2) === want;
    const lower = want.toLowerCase();
    return (text2) => norm(text2).toLowerCase().includes(lower);
  }
  var Texts = class {
    cache = /* @__PURE__ */ new Map();
    of(el) {
      let text2 = this.cache.get(el);
      if (text2 !== void 0) return text2;
      text2 = this.compute(el);
      this.cache.set(el, text2);
      return text2;
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
      const value2 = el.getAttribute(name);
      return value2 !== null && test(value2);
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
    const name = cut(shownName(el, role) || (INTERACTIVE.has(role) ? "" : norm(el.textContent)), 60);
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

  // src/read.ts
  function tidy(text2) {
    return text2.replace(/ /g, " ").split(/\r?\n/).map((line) => line.replace(/[ \t\f\v\r]+/g, " ").trim()).join("\n").replace(/\n{3,}/g, "\n\n").trim();
  }
  function text(node) {
    const el = node instanceof Document ? node.body || node.documentElement : node;
    if (!el) return "";
    if (!(el instanceof HTMLElement)) return tidy(el.textContent || "");
    if (!visible(el)) return "";
    const around = /* @__PURE__ */ new Set();
    for (const d of descendants(el)) {
      if (!d.shadowRoot) continue;
      for (let a = d; a && !around.has(a); a = a === el ? null : parentOf(a)) around.add(a);
    }
    if (!around.size) return tidy(el.innerText);
    let out = "";
    const walk2 = (parent) => {
      for (const child of childrenOf(parent)) {
        if (child.nodeType === Node.TEXT_NODE) {
          out += child.data.replace(/\s+/g, " ");
          continue;
        }
        if (!(child instanceof HTMLElement) || SKIP.has(tag(child))) continue;
        const style = getComputedStyle(child);
        if (style.display === "none") continue;
        const block = style.display !== "contents" && !style.display.startsWith("inline");
        if (block) out += "\n";
        if (tag(child) === "br") out += "\n";
        else if (around.has(child)) walk2(child);
        else out += child.innerText;
        if (block) out += "\n";
      }
    };
    walk2(el);
    return tidy(out);
  }
  var VOID = /* @__PURE__ */ new Set([
    "area",
    "base",
    "br",
    "col",
    "embed",
    "hr",
    "img",
    "input",
    "link",
    "meta",
    "param",
    "source",
    "track",
    "wbr"
  ]);
  var CUT = 200;
  var LINE = 120;
  var cut2 = (s, n) => s.length > n ? `${s.slice(0, n)}… (${s.length} chars)` : s;
  var escapeText = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  var escapeAttr = (s) => s.replace(/&/g, "&amp;").replace(/"/g, "&quot;");
  function openTag(el) {
    let out = `<${tag(el)}`;
    for (const a of Array.from(el.attributes)) {
      out += a.value === "" ? ` ${a.name}` : ` ${a.name}="${escapeAttr(cut2(a.value, CUT))}"`;
    }
    return out + ">";
  }
  function html(node, outer) {
    const out = [];
    if (node instanceof Document) {
      if (node.doctype) out.push(`<!doctype ${node.doctype.name}>`);
      if (node.documentElement) pretty(node.documentElement, 0, out);
    } else if (outer) {
      pretty(node, 0, out);
    } else {
      for (const child of contentOf(node)) pretty(child, 0, out);
    }
    return out.join("\n");
  }
  function contentOf(el) {
    const kids = [];
    if (el.shadowRoot) kids.push(el.shadowRoot);
    const own = tag(el) === "template" ? el.content.childNodes : el.childNodes;
    kids.push(...Array.from(own));
    return kids.filter((n) => !(n.nodeType === Node.TEXT_NODE && !n.data.trim()));
  }
  function pretty(node, depth, out) {
    const indent2 = "  ".repeat(depth);
    if (node instanceof ShadowRoot) {
      out.push(`${indent2}#shadow-root (${node.mode})`);
      for (const child of Array.from(node.childNodes)) {
        if (child.nodeType === Node.TEXT_NODE && !child.data.trim()) continue;
        pretty(child, depth + 1, out);
      }
      return;
    }
    if (node.nodeType === Node.TEXT_NODE) {
      const collapsed = norm(node.data);
      if (collapsed) out.push(indent2 + escapeText(collapsed));
      return;
    }
    if (node.nodeType === Node.COMMENT_NODE) {
      out.push(`${indent2}<!-- ${cut2(norm(node.data), CUT)} -->`);
      return;
    }
    if (!(node instanceof Element)) return;
    const name = tag(node);
    const open = openTag(node);
    if (VOID.has(name)) {
      out.push(indent2 + open);
      return;
    }
    const close = `</${name}>`;
    if (name === "script" || name === "style") {
      const body = (node.textContent || "").trim();
      out.push(indent2 + open + (body ? cut2(body, CUT) : "") + close);
      return;
    }
    if (name === "pre" || name === "textarea") {
      out.push(indent2 + open + escapeText(node.textContent || "") + close);
      return;
    }
    const kids = contentOf(node);
    const line = compact(node, LINE - indent2.length);
    if (line !== null) {
      out.push(indent2 + line);
      return;
    }
    out.push(indent2 + open);
    for (const child of kids) pretty(child, depth + 1, out);
    out.push(indent2 + close);
  }
  function compact(el, room) {
    const name = tag(el);
    if (name === "script" || name === "style" || name === "pre" || name === "textarea" || el.shadowRoot) return null;
    let line = openTag(el);
    if (VOID.has(name)) return line.length <= room ? line : null;
    const own = name === "template" ? el.content.childNodes : el.childNodes;
    const kids = Array.from(own);
    for (let i = 0; i < kids.length && line.length <= room; i++) {
      const child = kids[i];
      if (child.nodeType === Node.TEXT_NODE) {
        let t = child.data.replace(/\s+/g, " ");
        if (i === 0) t = t.trimStart();
        if (i === kids.length - 1) t = t.trimEnd();
        line += escapeText(t);
      } else if (child instanceof Element) {
        const inner = compact(child, room - line.length);
        if (inner === null) return null;
        line += inner;
      } else {
        return null;
      }
    }
    line += `</${name}>`;
    return line.length <= room ? line : null;
  }
  function value(el) {
    if (el instanceof HTMLInputElement) {
      if (el.type === "file") {
        return { value: Array.from(el.files || []).map((f) => f.name), kind: "file" };
      }
      return { value: el.value, kind: el.type === "password" ? "password" : "input" };
    }
    if (el instanceof HTMLTextAreaElement) return { value: el.value, kind: "textarea" };
    if (el instanceof HTMLSelectElement) {
      const chosen = Array.from(el.selectedOptions).map((o) => o.value);
      return { value: el.multiple ? chosen : chosen[0] ?? "", kind: "select" };
    }
    if (el instanceof HTMLOutputElement) return { value: el.value, kind: "output" };
    if (el instanceof HTMLProgressElement || el instanceof HTMLMeterElement) {
      return { value: String(el.value), kind: tag(el) };
    }
    if (el instanceof HTMLElement && el.isContentEditable) {
      return { value: tidy(el.innerText), kind: "contenteditable" };
    }
    const now = el.getAttribute("aria-valuetext") ?? el.getAttribute("aria-valuenow");
    if (now !== null) return { value: now, kind: "aria" };
    return { error: `it is a <${tag(el)}>, not a field` };
  }
  var DROP = /* @__PURE__ */ new Set([
    "script",
    "style",
    "noscript",
    "template",
    "head",
    "svg",
    "canvas",
    "iframe",
    "frame",
    "object",
    "embed",
    "video",
    "audio",
    "map",
    "input",
    "select",
    "textarea",
    "button",
    "dialog",
    "datalist",
    "option",
    "label"
  ]);
  var LIST_START = /^(?:- |\d+\. )/;
  var FURNITURE = /* @__PURE__ */ new Set(["nav", "aside", "footer"]);
  var FURNITURE_ROLES = /* @__PURE__ */ new Set(["navigation", "complementary", "contentinfo", "search"]);
  function markdown(el) {
    const readable = el instanceof Document;
    const root = el instanceof Document ? mainContent() : el;
    const md = new Markdown(readable);
    const blocks = [];
    md.blocks(root, blocks);
    return { markdown: blocks.join("\n\n").replace(/\n{3,}/g, "\n\n").trim(), source: describeEl(root) };
  }
  function describeEl(el) {
    let out = tag(el);
    if (el.id) out += `#${el.id}`;
    else if (typeof el.className === "string" && el.className.trim()) out += `.${el.className.trim().split(/\s+/)[0]}`;
    return out;
  }
  function mainContent() {
    const body = document.body || document.documentElement;
    const one2 = (selector) => {
      const found = Array.from(document.querySelectorAll(selector)).filter(visible);
      return found.length === 1 ? found[0] : null;
    };
    const main = one2("main, [role=main]") || one2("article");
    if (main) return main;
    const scores = /* @__PURE__ */ new Map();
    for (const p of Array.from(body.querySelectorAll("p, pre, blockquote, td, li"))) {
      const t = norm(p.textContent);
      if (t.length < 25 || !visible(p)) continue;
      const score = 1 + t.split(",").length + Math.min(Math.floor(t.length / 100), 3);
      const parent = p.parentElement;
      if (!parent) continue;
      scores.set(parent, (scores.get(parent) || 0) + score);
      const grand = parent.parentElement;
      if (grand) scores.set(grand, (scores.get(grand) || 0) + score / 2);
    }
    let best = null;
    let top = 0;
    for (const [candidate, score] of scores) {
      if (score > top) {
        best = candidate;
        top = score;
      }
    }
    return best && top >= 5 ? best : body;
  }
  var Markdown = class {
    /** `readable`: leave page furniture (navigation, sidebars, footers)
     * out. */
    constructor(readable) {
      this.readable = readable;
    }
    readable;
    base = location.href;
    display(el) {
      return el.ownerDocument.defaultView.getComputedStyle(el).display;
    }
    shown(el) {
      if (DROP.has(tag(el))) return false;
      if (el.getAttribute("aria-hidden") === "true") return false;
      return this.display(el) !== "none";
    }
    furniture(el) {
      if (!this.readable) return false;
      const role = el.getAttribute("role");
      return FURNITURE.has(tag(el)) || role !== null && FURNITURE_ROLES.has(role);
    }
    isBlock(el) {
      const display = this.display(el);
      return display !== "contents" && !display.startsWith("inline") && display !== "none";
    }
    /** Appends `el`'s content as Markdown blocks to `out`. */
    blocks(el, out) {
      let run = "";
      const flush = () => {
        const paragraph = tidyInline(run);
        if (paragraph) out.push(paragraph);
        run = "";
      };
      for (const child of childrenOf(el)) {
        if (child.nodeType === Node.TEXT_NODE) {
          run += collapse(child.data);
          continue;
        }
        if (!(child instanceof Element) || !this.shown(child) || this.furniture(child)) continue;
        if (this.display(child) === "contents") {
          flush();
          this.blocks(child, out);
          continue;
        }
        if (!this.isBlock(child) && tag(child) !== "table") {
          run += this.inline(child);
          continue;
        }
        flush();
        this.block(child, out);
      }
      flush();
    }
    block(el, out) {
      const name = tag(el);
      const heading = /^h([1-6])$/.exec(name);
      if (heading) {
        const t = tidyInline(this.inlineChildren(el)).replace(/\n+/g, " ");
        if (t) out.push(`${"#".repeat(Number(heading[1]))} ${t}`);
        return;
      }
      switch (name) {
        case "hr":
          out.push("---");
          return;
        case "br":
          return;
        case "img": {
          const img = this.inline(el);
          if (img) out.push(img);
          return;
        }
        case "pre": {
          const code = el.querySelector("code");
          const lang = /(?:^|\s)(?:language|lang)-(\S+)/.exec((code || el).className || "");
          const body = (el.innerText || el.textContent || "").replace(/\n+$/, "");
          out.push("```" + (lang ? lang[1] : "") + "\n" + body + "\n```");
          return;
        }
        case "ul":
        case "ol":
          this.list(el, name === "ol", out);
          return;
        case "blockquote": {
          const inner = [];
          this.blocks(el, inner);
          const quoted = inner.join("\n\n").split("\n").map((l) => l ? `> ${l}` : ">").join("\n");
          if (inner.length) out.push(quoted);
          return;
        }
        case "table":
          this.table(el, out);
          return;
        case "dt": {
          const t = tidyInline(this.inlineChildren(el));
          if (t) out.push(`**${t}**`);
          return;
        }
        case "dd": {
          const inner = [];
          this.blocks(el, inner);
          if (inner.length) out.push(indent(inner.join("\n\n"), ": ", "  "));
          return;
        }
        default:
          this.blocks(el, out);
      }
    }
    list(el, ordered, out) {
      const items = [];
      let n = ordered ? Number(el.getAttribute("start") || 1) : 0;
      for (const child of childrenOf(el)) {
        if (!(child instanceof Element) || !this.shown(child)) continue;
        const inner = [];
        if (tag(child) === "li") this.blocks(child, inner);
        else this.block(child, inner);
        const body = inner.reduce((all, b) => !all ? b : all + (LIST_START.test(b) ? "\n" : "\n\n") + b, "");
        if (!body) continue;
        const marker = ordered ? `${n++}. ` : "- ";
        items.push(indent(body, marker, " ".repeat(marker.length)));
      }
      if (items.length) out.push(items.join(items.some((i) => i.includes("\n\n")) ? "\n\n" : "\n"));
    }
    table(table, out) {
      const rows = Array.from(table.rows).filter((r) => this.shown(r));
      if (!rows.length) return;
      const cells = rows.map((r) => Array.from(r.cells).map((c) => tidyInline(this.inlineChildren(c)).replace(/\n+/g, " ").replace(/\|/g, "\\|")));
      const width = Math.max(...cells.map((r) => r.length));
      if (width === 0) return;
      const line = (r) => `| ${Array.from({ length: width }, (_, i) => r[i] ?? "").join(" | ")} |`;
      const header = cells[0];
      const lines = [line(header), `|${" --- |".repeat(width)}`, ...cells.slice(1).map(line)];
      const caption = table.caption ? tidyInline(this.inlineChildren(table.caption)) : "";
      if (caption) out.push(`**${caption}**`);
      out.push(lines.join("\n"));
    }
    inlineChildren(el) {
      let out = "";
      for (const child of childrenOf(el)) out += this.inline(child);
      return out;
    }
    /** `node` as inline Markdown; a block inside inline content becomes
     * a line break. */
    inline(node) {
      if (node.nodeType === Node.TEXT_NODE) return collapse(node.data);
      if (!(node instanceof Element) || !this.shown(node) || this.furniture(node)) return "";
      const name = tag(node);
      if (name === "br") return "\n";
      if (name === "img") {
        const alt = norm(node.getAttribute("alt"));
        const src = node.getAttribute("src") || "";
        if (!alt && (!src || src.startsWith("data:"))) return "";
        return `![${alt}](${src.startsWith("data:") ? "" : this.href(src)})`;
      }
      const inner = this.inlineChildren(node);
      const wrapped = this.isBlock(node) ? `
${inner}
` : inner;
      const t = norm(inner);
      if (!t) return wrapped.includes("![") ? wrapped : " ";
      switch (name) {
        case "a": {
          const href = node.getAttribute("href");
          if (!href || /^javascript:/i.test(href)) return wrapped;
          return edge(inner, `[${tidyInline(inner).replace(/\n+/g, " ")}](${this.href(href)})`);
        }
        case "strong":
        case "b":
          return edge(inner, `**${t}**`);
        case "em":
        case "i":
          return edge(inner, `_${t}_`);
        case "code":
        case "kbd":
        case "samp":
          return edge(inner, t.includes("`") ? `\`\` ${t} \`\`` : `\`${t}\``);
        case "del":
        case "s":
          return edge(inner, `~~${t}~~`);
        default:
          return wrapped;
      }
    }
    /** A link as the snapshot shows it: relative (path, query, fragment)
     * when on the page's origin. */
    href(raw) {
      let url;
      try {
        url = new URL(raw, this.base);
      } catch {
        return raw;
      }
      const here = new URL(this.base);
      if (url.origin === here.origin && /^(https?|file):$/.test(url.protocol)) {
        return url.pathname + url.search + url.hash;
      }
      return url.href;
    }
  };
  var collapse = (s) => s.replace(/[ \t\n\r\f]+/g, " ");
  function edge(raw, marked) {
    return (/^\s/.test(raw) ? " " : "") + marked + (/\s$/.test(raw) ? " " : "");
  }
  function tidyInline(s) {
    return s.replace(/ /g, " ").split("\n").map((l) => l.replace(/[ \t\f\v\r]+/g, " ").trim()).filter((l, i, all) => l || i > 0 && all[i - 1]).join("\n").trim();
  }
  function indent(body, first, rest) {
    return body.split("\n").map((l, i) => i === 0 ? first + l : l ? rest + l : l).join("\n");
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
    const modal = openModal(document);
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
      if (hidden(el, style) || outsideModal(el, modal)) return;
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
      const value2 = valueOf(el, role);
      if (value2 != null && value2 !== "") n.v = String(value2);
      const lv = level(el, role);
      if (lv) n.l = lv;
      const href = el.href;
      if (role === "link" && href) {
        const svg = href;
        n.h = String(svg.baseVal !== void 0 ? svg.baseVal : href);
      }
      if (tag(el) === "input") {
        const type = el.type;
        if (type === "password") n.w = 1;
        else if (type === "file") n.w = 2;
      }
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
  var api = { version: 1, query, one, describe, walk, visible, text, tidy, html, value, markdown };
  if (!globalThis.__ccdp) globalThis.__ccdp = api;
})();
