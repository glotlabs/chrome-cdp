// Built by `cargo xtask build-js` from src/*.ts; do not edit. sources: cf255af9f0546daf
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
  function contains(outer, inner) {
    for (let n = inner; n; n = n instanceof Element ? parentOf(n) : n.parentNode) {
      if (n === outer) return true;
    }
    return false;
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
    return unique.sort((a2, b) => (order.get(a2) ?? 1e9) - (order.get(b) ?? 1e9));
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
    let a2 = document.activeElement;
    while (a2 && a2.shadowRoot && a2.shadowRoot.activeElement) a2 = a2.shadowRoot.activeElement;
    return a2;
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
    for (let a2 = parentOf(el); a2 && j >= 0; a2 = parentOf(a2)) {
      if (a2.matches(parts[j])) j--;
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
    const test2 = matcher(m);
    const matches = (el) => test2(texts.of(el));
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
    const test2 = matcher(m);
    return descendants(scope).filter((el) => {
      const value2 = el.getAttribute(name);
      return value2 !== null && test2(value2);
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
        const test2 = matcher(step.text);
        return descendants(scope).filter((el) => labelsOf(el).some(test2));
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
    return run(steps, root || document, new Texts());
  }
  function inScopes(steps, scopes, texts) {
    const out = /* @__PURE__ */ new Set();
    for (const scope of scopes) for (const el of run(steps, scope, texts)) out.add(el);
    return out;
  }
  function run(steps, top, texts) {
    let current = [top];
    let scopes = [top];
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
        case "has-text":
        case "has-not-text": {
          const test2 = matcher(step.text);
          const want = step.kind === "has-text";
          current = els.filter((el) => test2(texts.of(el)) === want);
          break;
        }
        case "has":
        case "has-not": {
          const want = step.kind === "has";
          current = els.filter((el) => run(step.steps, el, texts).length > 0 === want);
          break;
        }
        case "and": {
          const also = inScopes(step.steps, scopes, texts);
          current = els.filter((el) => also.has(el));
          break;
        }
        case "or":
          current = inOrder([...els, ...inScopes(step.steps, scopes, texts)], top);
          break;
        default: {
          const found = [];
          for (const scope of current) found.push(...find(step, scope, texts));
          scopes = current;
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
    for (let a2 = parentOf(el); a2; a2 = parentOf(a2)) {
      const r = roleOf(a2);
      if (!r || !CONTEXT_ROLES.has(r)) continue;
      const n = cut(r === "row" || r === "listitem" ? norm(textOf(a2, null, 0)) : nameOf(a2, r), 40);
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
    const out = {
      count: found.length,
      candidates: els.map(describe)
    };
    if (found.length === 0 && steps.length > 1 && steps[steps.length - 1].kind === "has") {
      out.beforeHas = query(steps.slice(0, -1), root).length;
    }
    return { json: JSON.stringify(out), els };
  }

  // src/read.ts
  function tidy(text2) {
    return text2.replace(/ /g, " ").split(/\r?\n/).map((line) => line.replace(/[ \t\f\v\r]+/g, " ").trim()).join("\n").replace(/\n{3,}/g, "\n\n").trim();
  }
  var frameMark = (i) => `\0${i}\0`;
  function ownerIndex(el, owners) {
    if (!owners.length) return -1;
    const i = owners.indexOf(el);
    return i >= 0 && visible(el) ? i : -1;
  }
  function text(node, owners = []) {
    const el = node instanceof Document ? node.body || node.documentElement : node;
    if (!el) return "";
    if (!(el instanceof HTMLElement)) return tidy(el.textContent || "");
    if (!visible(el)) return "";
    const around = /* @__PURE__ */ new Set();
    const mark = (d) => {
      for (let a2 = d; a2 && !around.has(a2); a2 = a2 === el ? null : parentOf(a2)) around.add(a2);
    };
    for (const d of descendants(el)) {
      if (d.shadowRoot || ownerIndex(d, owners) >= 0) mark(d);
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
        const frame2 = ownerIndex(child, owners);
        if (frame2 >= 0) {
          out += `
${frameMark(frame2)}
`;
          continue;
        }
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
    for (const a2 of Array.from(el.attributes)) {
      out += a2.value === "" ? ` ${a2.name}` : ` ${a2.name}="${escapeAttr(cut2(a2.value, CUT))}"`;
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
  function markdown(el, owners = []) {
    const readable = el instanceof Document;
    const root = el instanceof Document ? mainContent() : el;
    const md = new Markdown(readable, owners);
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
     * out. `owners`: the `<iframe>`s whose frames' content goes in. */
    constructor(readable, owners) {
      this.readable = readable;
      this.owners = owners;
    }
    readable;
    owners;
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
      let run2 = "";
      const flush = () => {
        const paragraph = tidyInline(run2);
        if (paragraph) out.push(paragraph);
        run2 = "";
      };
      for (const child of childrenOf(el)) {
        if (child.nodeType === Node.TEXT_NODE) {
          run2 += collapse(child.data);
          continue;
        }
        if (!(child instanceof Element)) continue;
        const frame2 = ownerIndex(child, this.owners);
        if (frame2 >= 0) {
          flush();
          out.push(frameMark(frame2));
          continue;
        }
        if (!this.shown(child) || this.furniture(child)) continue;
        if (this.display(child) === "contents") {
          flush();
          this.blocks(child, out);
          continue;
        }
        if (!this.isBlock(child) && tag(child) !== "table") {
          run2 += this.inline(child);
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
          const quoted2 = inner.join("\n\n").split("\n").map((l) => l ? `> ${l}` : ">").join("\n");
          if (inner.length) out.push(quoted2);
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
      if (!(node instanceof Element)) return "";
      const frame2 = ownerIndex(node, this.owners);
      if (frame2 >= 0) return `
${frameMark(frame2)}
`;
      if (!this.shown(node) || this.furniture(node)) return "";
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
    const over2 = (v) => v === "auto" || v === "scroll" || v === "overlay";
    return over2(style.overflowY) && el.scrollHeight - el.clientHeight > 1 || over2(style.overflowX) && el.scrollWidth - el.clientWidth > 1;
  }
  function walk(scope) {
    const view = document.defaultView;
    const nodes = [];
    const els = [];
    const active = activeElement();
    const modal = openModal(document);
    function visit(node, parent, parentStyle, invisible2, mute) {
      if (nodes.length > 2e5) return;
      if (node.nodeType === Node.TEXT_NODE) {
        const data = node.data;
        if (!invisible2 && !mute && data) nodes.push({ p: parent, t: 1, n: data });
        return;
      }
      if (node.nodeType !== Node.ELEMENT_NODE) return;
      const el = node;
      if (SKIP.has(tag(el))) return;
      if (tag(el) === "br") {
        if (!invisible2 && !mute) nodes.push({ p: parent, t: 1, n: "\n" });
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

  // src/actionable.ts
  function outline(el) {
    const id = el.getAttribute("id");
    if (id) return `<${tag(el)} id="${cut3(id, 30)}">`;
    const cls = norm2(el.getAttribute("class"));
    if (cls) return `<${tag(el)} class="${cut3(cls, 30)}">`;
    return `<${tag(el)}>`;
  }
  var norm2 = (s) => (s || "").replace(/\s+/g, " ").trim();
  var cut3 = (s, n) => s.length > n ? s.slice(0, n - 1) + "…" : s;
  function closest(el, test2) {
    for (let a2 = el; a2; a2 = parentOf(a2)) {
      if (test2(a2, getComputedStyle(a2))) return a2;
    }
    return null;
  }
  var on = (el, cause) => cause && cause !== el ? ` on ${outline(cause)}` : "";
  function causeOn(el, what, cause) {
    if (!cause || cause === el) return { why: `it has ${what}`, what };
    return { why: `it has ${what}${on(el, cause)}`, what: `${what} on {el}`, el: cause };
  }
  function setter(el, test2) {
    let top = el;
    for (let a2 = parentOf(el); a2 && test2(getComputedStyle(a2)); a2 = parentOf(a2)) top = a2;
    return top;
  }
  function invisibleCause(el) {
    if (!visible(el)) {
      const box = el.getBoundingClientRect();
      if (box.width > 0 && box.height > 0) {
        const hidden3 = (s) => s.visibility === "hidden" || s.visibility === "collapse";
        return causeOn(el, "visibility: hidden", setter(el, hidden3));
      }
      const none = closest(el, (_, s) => s.display === "none");
      if (none) return causeOn(el, "display: none", none);
      const skipped = closest(el, (a2, s) => a2 !== el && s.contentVisibility === "hidden");
      if (skipped) {
        return {
          why: `it is inside ${outline(skipped)}, which has content-visibility: hidden`,
          what: "content-visibility: hidden on {el}",
          el: skipped
        };
      }
      const what = `an empty box (${Math.round(box.width)}x${Math.round(box.height)})`;
      return { why: `its box is empty (${Math.round(box.width)}x${Math.round(box.height)})`, what };
    }
    const clear = closest(el, (_, s) => s.opacity === "0");
    if (clear) return causeOn(el, "opacity: 0", clear);
    return null;
  }
  function invisible(el) {
    return invisibleCause(el)?.why ?? null;
  }
  var CONTROLS = /* @__PURE__ */ new Set(["button", "input", "select", "textarea", "optgroup", "option", "fieldset"]);
  function disabledCause(el) {
    if (CONTROLS.has(tag(el)) && el.matches(":disabled")) {
      if (el.hasAttribute("disabled")) return { why: "it has the disabled attribute", what: "the disabled attribute" };
      const parent = closest(el, (a2) => a2 !== el && a2.matches("fieldset:disabled, select:disabled, optgroup:disabled"));
      if (parent) return { why: `it is inside a disabled ${outline(parent)}`, what: "inside the disabled {el}", el: parent };
      return { why: "it is disabled", what: "disabled" };
    }
    const aria = closest(el, (a2) => a2.getAttribute("aria-disabled") === "true");
    if (aria) return causeOn(el, 'aria-disabled="true"', aria);
    return null;
  }
  function disabled(el) {
    return disabledCause(el)?.why ?? null;
  }
  function inertCause(el) {
    const inert = closest(el, (a2) => a2.hasAttribute("inert"));
    if (inert) return causeOn(el, "inert", inert);
    const doc = el.ownerDocument;
    const modals = Array.from(doc.querySelectorAll("dialog")).filter((d) => d.matches(":modal"));
    if (modals.length && !modals.some((m) => contains(m, el))) {
      const modal = modals[modals.length - 1];
      return {
        why: `it is inert while the modal ${outline(modal)} is open`,
        what: "inert while the modal {el} is open",
        el: modal
      };
    }
    return null;
  }
  function pointerCause(el) {
    if (getComputedStyle(el).pointerEvents !== "none") return null;
    return causeOn(el, "pointer-events: none", setter(el, (s) => s.pointerEvents === "none"));
  }
  var NOT_TEXT = /* @__PURE__ */ new Set(["checkbox", "radio", "file", "button", "submit", "reset", "image", "hidden"]);
  function readOnly(el) {
    const name = tag(el);
    if (name === "input") {
      const type = el.type;
      if (NOT_TEXT.has(type)) return { why: `it is an <input type=${type}>, which takes no text`, final: true };
    } else if (name === "select") {
      return { why: "it is a <select>: choose its options with select", final: true };
    } else if (name !== "textarea" && !el.isContentEditable) {
      const role = roleOf(el);
      if (role !== "textbox" && role !== "searchbox" && role !== "combobox" && role !== "spinbutton") {
        return { why: "it is not an input, textarea or contenteditable element", final: true };
      }
    }
    if ((name === "input" || name === "textarea") && el.readOnly) {
      return { why: "it has the readonly attribute" };
    }
    if (el.getAttribute("aria-readonly") === "true") return { why: 'it has aria-readonly="true"' };
    return null;
  }
  var frame = () => new Promise((resolve) => {
    requestAnimationFrame(() => resolve());
    setTimeout(resolve, 100);
  });
  var rect = (el) => {
    const r = el.getBoundingClientRect();
    return [r.x, r.y, r.width, r.height];
  };
  async function checkActionable(el, needs) {
    if (!el.isConnected) return { check: "attached", why: "it was removed from the page" };
    const hidden3 = invisible(el);
    if (hidden3) return { check: "visible", why: hidden3 };
    await frame();
    const before2 = rect(el);
    await frame();
    const after = rect(el);
    if (!el.isConnected) return { check: "attached", why: "it was removed from the page" };
    if (before2.some((v, i) => v !== after[i])) {
      const moved = before2[0] !== after[0] || before2[1] !== after[1];
      return { check: "stable", why: moved ? "it is still moving" : "it is still changing size" };
    }
    if (needs.enabled || needs.editable) {
      const why = disabled(el) || inertCause(el)?.why;
      if (why) return { check: "enabled", why };
    }
    if (needs.editable) {
      const why = readOnly(el);
      if (why) return { check: "editable", ...why };
    }
    return null;
  }
  function named(el) {
    const out = describe(el);
    const cls = norm2(el.getAttribute("class")).split(" ")[0];
    if (cls) out.cls = cut3(cls, 30);
    return out;
  }
  var DISMISS = /^(accept|agree|allow|ok|okay|got it|close|dismiss|continue|no,? thanks|reject|decline|×|✕|x)\b/i;
  function covering(el, hit) {
    const start = hit instanceof Element ? hit : hit.parentElement;
    if (!start) return { json: "{}", els: [] };
    const doc = start.ownerDocument;
    let top = start;
    for (let a2 = parentOf(start); a2 && a2 !== doc.body && a2 !== doc.documentElement; a2 = parentOf(a2)) {
      if (el && contains(a2, el)) break;
      top = a2;
    }
    const inert = el !== null && contains(start, el);
    if (inert) top = start;
    let dismiss = null;
    if (!inert) {
      const buttons = Array.from(top.querySelectorAll("button, [role=button], a[href], input[type=button], input[type=submit]")).filter((b) => visible(b));
      dismiss = buttons.find((b) => DISMISS.test(describe(b).name)) || buttons[0] || null;
    }
    const out = {
      covering: named(top),
      dismiss: dismiss && named(dismiss),
      inert
    };
    if (inert && el) out.pointerEvents = getComputedStyle(el).pointerEvents;
    return { json: JSON.stringify(out), els: [top, dismiss] };
  }

  // src/pointer.ts
  function scrollable(el) {
    const s = getComputedStyle(el);
    const can = (overflow) => /^(auto|scroll|overlay)$/.test(overflow);
    return can(s.overflowY) && el.scrollHeight > el.clientHeight || can(s.overflowX) && el.scrollWidth > el.clientWidth;
  }
  function container(el) {
    const root = document.scrollingElement || document.documentElement;
    for (let a2 = el; a2; a2 = parentOf(a2)) {
      if (a2 === root || a2 === document.body) return null;
      if (scrollable(a2)) return a2;
    }
    return null;
  }
  function measure(c) {
    const s = c || document.scrollingElement || document.documentElement;
    const x = c ? c.scrollLeft : scrollX;
    const y = c ? c.scrollTop : scrollY;
    return {
      x: Math.round(x),
      y: Math.round(y),
      maxX: Math.max(0, Math.round(s.scrollWidth - s.clientWidth)),
      maxY: Math.max(0, Math.round(s.scrollHeight - s.clientHeight))
    };
  }
  function scrollState(el) {
    const c = container(el);
    return { json: JSON.stringify({ ...measure(c), page: !c }), els: [c] };
  }
  function scrollSettled(el, from, idle, ms) {
    const c = container(el);
    return new Promise((resolve) => {
      let last = measure(c);
      let moved = false;
      let same = 0;
      let frames = 0;
      const done = setTimeout(() => resolve(measure(c)), ms);
      const tick = () => {
        const now = measure(c);
        if (now.x !== from.x || now.y !== from.y) moved = true;
        same = now.x === last.x && now.y === last.y ? same + 1 : 0;
        last = now;
        frames += 1;
        if (moved && same >= 2 || !moved && frames >= idle) {
          clearTimeout(done);
          resolve(now);
        } else {
          requestAnimationFrame(tick);
        }
      };
      requestAnimationFrame(tick);
    });
  }
  function draggable(el) {
    for (let a2 = el; a2; a2 = parentOf(a2)) {
      if (a2.getAttribute("draggable") === "false") return false;
      if (a2 instanceof HTMLElement && a2.draggable) return true;
    }
    return false;
  }

  // src/form.ts
  function focused(el) {
    const doc = el ? el.ownerDocument || el : document;
    let f = doc.activeElement;
    while (f && f.shadowRoot && f.shadowRoot.activeElement) f = f.shadowRoot.activeElement;
    return f && f !== doc.body && f !== doc.documentElement ? f : null;
  }
  function hasFocus(el) {
    const f = focused(el);
    return !!f && (f === el || contains(el, f));
  }
  function caretToEnd(el) {
    if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) {
      try {
        const end = el.value.length;
        el.setSelectionRange(end, end);
      } catch {
      }
      return;
    }
    if (el.isContentEditable) {
      const range = el.ownerDocument.createRange();
      range.selectNodeContents(el);
      range.collapse(false);
      const selection = el.ownerDocument.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);
    }
  }
  var SET_VALUE = /* @__PURE__ */ new Set(["color", "date", "time", "datetime-local", "month", "range", "week"]);
  var TYPE_INTO = /* @__PURE__ */ new Set(["", "email", "number", "password", "search", "tel", "text", "url"]);
  var EXAMPLES = {
    color: "#ff0000",
    date: "2026-09-27",
    time: "13:45",
    "datetime-local": "2026-09-27T13:45",
    month: "2026-09",
    week: "2026-W39"
  };
  var fire = (el) => {
    el.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
  };
  function fillPrepare(el, value2) {
    if (el instanceof HTMLInputElement) {
      const type = el.type.toLowerCase();
      if (!TYPE_INTO.has(type) && !SET_VALUE.has(type)) {
        return { error: `it is an <input type=${type}>, which takes no text` };
      }
      if (type === "number" && value2.trim() !== "" && Number.isNaN(Number(value2.trim()))) {
        return { error: `it is an <input type=number>, and "${value2}" is not a number`, example: el.min || "42" };
      }
      if (SET_VALUE.has(type)) {
        const want = value2.trim();
        el.focus();
        el.value = want;
        if (el.value !== want) {
          const range2 = type === "range";
          const like = range2 ? `a number from ${el.min || "0"} to ${el.max || "100"}${el.step && el.step !== "1" ? ` in steps of ${el.step}` : ""}` : `a value like ${EXAMPLES[type]}`;
          const example = range2 ? el.min || "0" : EXAMPLES[type];
          return { error: `it is an <input type=${type}>, which takes ${like}, not "${value2}"`, example };
        }
        fire(el);
        return { set: true };
      }
      el.focus();
      el.select();
      return { set: false };
    }
    if (el instanceof HTMLSelectElement) {
      return { error: "it is a <select>, whose options select chooses", select: true };
    }
    if (el instanceof HTMLTextAreaElement) {
      el.focus();
      el.select();
      return { set: false };
    }
    if (!el.isContentEditable) {
      return { error: "it is not an input, textarea or contenteditable element" };
    }
    const range = el.ownerDocument.createRange();
    range.selectNodeContents(el);
    const selection = el.ownerDocument.getSelection();
    selection?.removeAllRanges();
    selection?.addRange(range);
    el.focus();
    return { set: false };
  }
  function fieldValue(el) {
    if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) {
      return { value: el.value, secret: el instanceof HTMLInputElement && el.type === "password" };
    }
    return { value: el.innerText.replace(/\n$/, ""), secret: false };
  }
  var option = (o) => ({ value: o.value, label: norm(o.label) });
  var LISTED2 = 20;
  function selectOptions(el, values) {
    if (!(el instanceof HTMLSelectElement)) {
      return JSON.stringify({ notSelect: true, role: roleOf(el) || "" });
    }
    const options = Array.from(el.options);
    const picked = [];
    for (const v of values) {
      const wanted = norm(v);
      const found = options.find((o) => o.value === v) || options.find((o) => norm(o.label) === wanted) || options.find((o) => norm(o.label).toLowerCase() === wanted.toLowerCase());
      if (!found) {
        const listed2 = options.slice(0, LISTED2).map((o) => ({ ...option(o), disabled: o.disabled || void 0 }));
        return JSON.stringify({ missing: v, options: listed2, more: Math.max(0, options.length - LISTED2) });
      }
      if (found.disabled || found.parentElement instanceof HTMLOptGroupElement && found.parentElement.disabled) {
        return JSON.stringify({ disabled: v });
      }
      if (!picked.includes(found)) picked.push(found);
    }
    if (!el.multiple && picked.length > 1) return JSON.stringify({ single: picked.length });
    for (const o of options) o.selected = picked.includes(o);
    fire(el);
    return JSON.stringify({ selected: picked.map(option) });
  }
  var CHECKABLE = /* @__PURE__ */ new Set(["checkbox", "radio", "switch", "menuitemcheckbox", "menuitemradio"]);
  function visuallyHidden(el) {
    if (invisible(el)) return true;
    const box = el.getBoundingClientRect();
    if (box.width <= 1 || box.height <= 1) return true;
    const s = getComputedStyle(el);
    return s.clip !== "auto" && s.clip !== "" || s.clipPath !== "none" && s.clipPath !== "";
  }
  function checkState(el) {
    let control = el;
    if (el instanceof HTMLLabelElement && el.control) control = el.control;
    let kind = null;
    let state = false;
    const role = roleOf(control) || "";
    if (control instanceof HTMLInputElement && (control.type === "checkbox" || control.type === "radio")) {
      kind = "native";
      state = control.indeterminate && control.type === "checkbox" ? "mixed" : control.checked;
    } else if (CHECKABLE.has(role)) {
      kind = "aria";
      const aria = control.getAttribute("aria-checked");
      state = aria === "mixed" ? "mixed" : aria === "true";
    }
    let click = control;
    if (el !== control) {
      click = el;
    } else if (kind === "native" && visuallyHidden(control)) {
      const labels = Array.from(control.labels || []);
      click = labels.find((l) => visible(l) && !invisible(l)) || control;
    }
    const radio = role === "radio" || role === "menuitemradio" || control.type === "radio";
    const candidate = describe(control);
    const disabled2 = control.matches(":disabled") || control.getAttribute("aria-disabled") === "true";
    const out = { kind, state, radio, candidate, disabled: disabled2, click: click && click !== control ? describe(click) : null };
    return { json: JSON.stringify(out), els: [control, click] };
  }
  function reaches(el, hit) {
    if (contains(el, hit)) return true;
    for (let a2 = hit instanceof Element ? hit : parentOf(hit); a2; a2 = parentOf(a2)) {
      if (a2 instanceof HTMLLabelElement) return a2.control === el;
    }
    return false;
  }

  // src/report.ts
  var before = null;
  var actedOn = [];
  var isFrame = (el) => el instanceof HTMLIFrameElement || el instanceof HTMLFrameElement;
  function formOf(el) {
    if (!el) return null;
    const owner2 = el.form;
    return owner2 instanceof HTMLFormElement ? owner2 : el.closest("form");
  }
  function focusMark(held = true) {
    const el = focused();
    const here = held && !isFrame(el);
    before = { el: here ? el : null, form: here ? formOf(el) : null, away: !here };
    actedOn = [];
    return held && isFrame(el) ? el : null;
  }
  function acted(el) {
    if (before) actedOn.push(el);
    return describe(el);
  }
  function focusMoved() {
    const mark = before || { el: null, form: null, away: true };
    const wasMarked = !!before;
    before = null;
    const el = focused();
    if (isFrame(el)) return el;
    if (!wasMarked) actedOn = [];
    if (!el || el === mark.el) return null;
    for (const a2 of actedOn) {
      if (a2 === el || contains(a2, el) || a2 instanceof HTMLLabelElement && a2.control === el) return null;
    }
    if (mark.away) return actedOn.some((a2) => formOf(a2) === formOf(el)) ? null : el;
    return formOf(el) === mark.form ? null : el;
  }

  // src/expect.ts
  var ArgError = class extends Error {
    constructor(message) {
      super(message);
      this.name = "ArgError";
    }
  };
  function test(p) {
    if (p.mode === "regex") {
      let re;
      try {
        re = new RegExp(p.value, (p.flags || "").replace(/[gy]/g, ""));
      } catch (e) {
        throw new ArgError(`/${p.value}/${p.flags || ""} is not a valid regex: ${e.message}`);
      }
      return (s) => re.test(s);
    }
    const want = norm(p.value);
    if (p.mode === "exact") return (s) => norm(s) === want;
    return (s) => norm(s).includes(want);
  }
  var SHOWN = 300;
  var cut4 = (s) => s.length > SHOWN ? s.slice(0, SHOWN - 1) + "…" : s;
  function textOf2(el) {
    return visible(el) ? norm(text(el)) : norm(el.textContent);
  }
  var CHECKABLE2 = /* @__PURE__ */ new Set(["checkbox", "radio", "switch", "menuitemcheckbox", "menuitemradio"]);
  function checked(el) {
    const control = el instanceof HTMLLabelElement && el.control ? el.control : el;
    if (control instanceof HTMLInputElement && (control.type === "checkbox" || control.type === "radio")) {
      return control.indeterminate && control.type === "checkbox" ? "mixed" : control.checked;
    }
    if (!CHECKABLE2.has(roleOf(control) || "")) return null;
    const aria = control.getAttribute("aria-checked");
    return aria === "mixed" ? "mixed" : aria === "true";
  }
  function inViewport(el) {
    return new Promise((resolve) => {
      const observer = new IntersectionObserver((entries) => {
        observer.disconnect();
        resolve(entries[entries.length - 1].intersectionRatio);
      });
      observer.observe(el);
      setTimeout(() => {
        observer.disconnect();
        const r = el.getBoundingClientRect();
        const view = el.ownerDocument.defaultView;
        const w = Math.max(0, Math.min(r.right, view.innerWidth) - Math.max(r.left, 0));
        const h = Math.max(0, Math.min(r.bottom, view.innerHeight) - Math.max(r.top, 0));
        resolve(r.width * r.height > 0 ? w * h / (r.width * r.height) : 0);
      }, 200);
    });
  }
  var canvas = null;
  function colour(s) {
    canvas = canvas || document.createElement("canvas").getContext("2d");
    if (!canvas) return null;
    canvas.fillStyle = "#010203";
    canvas.fillStyle = s;
    const first = canvas.fillStyle;
    canvas.fillStyle = "#040506";
    canvas.fillStyle = s;
    return first === canvas.fillStyle ? String(first) : null;
  }
  function cssMatches(actual, p) {
    if (p.mode === "regex") return test(p)(actual);
    if (norm(actual) === norm(p.value)) return true;
    const a2 = colour(actual);
    return a2 !== null && a2 === colour(p.value);
  }
  var ONE = /* @__PURE__ */ new Set([
    "visible",
    "enabled",
    "disabled",
    "editable",
    "checked",
    "unchecked",
    "focused",
    "text",
    "value",
    "values",
    "attr",
    "class",
    "css",
    "in-viewport",
    "hidden"
  ]);
  async function expectElements(spec, els) {
    const count = els.length;
    const m = spec.matcher;
    if (m === "attached") return { pass: count > 0, actual: count, count };
    if (m === "detached") return { pass: count === 0, actual: count, count };
    if (m === "count") {
      const c = spec.count || {};
      const pass = (c.eq == null || count === c.eq) && (c.min == null || count >= c.min) && (c.max == null || count <= c.max);
      return { pass, actual: count, count };
    }
    if (ONE.has(m) && count > 1) {
      return { pass: false, invalid: `${count} elements match`, count };
    }
    if (count === 0) {
      if (m === "visible") return { pass: false, state: "no element matches", count };
      if (m === "hidden") return { pass: true, state: "no element matches", count };
      return { pass: false, invalid: "no element matches", count };
    }
    const el = els[0];
    const one2 = (pass, state, actual) => ({ pass, state, actual, count });
    switch (m) {
      case "visible":
      case "hidden": {
        const shown3 = visible(el);
        const why = shown3 ? null : invisible(el);
        const state = shown3 ? "visible" : `hidden${why ? ` (${why})` : ""}`;
        return one2(m === "visible" ? shown3 : !shown3, state);
      }
      case "enabled":
      case "disabled": {
        const why = disabled(el);
        return one2(m === "enabled" ? !why : !!why, why ? `disabled (${why})` : "enabled");
      }
      case "editable": {
        const why = disabled(el) || readOnly(el)?.why || null;
        return one2(!why, why ? `not editable (${why})` : "editable");
      }
      case "checked":
      case "unchecked": {
        const state = checked(el);
        if (state === null) {
          return { pass: false, invalid: `it is ${roleOf(el) || "an element"}, not a checkbox, radio or switch`, count };
        }
        const words = state === "mixed" ? "mixed" : state ? "checked" : "unchecked";
        return one2(m === "checked" ? state === true : state === false, words);
      }
      case "focused": {
        const f = focused(el);
        if (f === el) return one2(true, "focused");
        if (!f) return one2(false, "not focused (nothing has focus)");
        const d = describe(f);
        return one2(false, `not focused (${d.role}${d.name ? ` "${d.name}"` : ""} has focus)`);
      }
      case "in-viewport": {
        const ratio = await inViewport(el);
        return one2(ratio > 0, ratio > 0 ? `in the viewport (${Math.round(ratio * 100)}% of it)` : "outside the viewport", ratio);
      }
      case "text": {
        const actual = textOf2(el);
        return one2(test(spec.pattern)(actual), "", cut4(actual));
      }
      case "value":
      case "values": {
        const read2 = value(el);
        if (read2.error) return { pass: false, invalid: read2.error, count };
        if (m === "value") {
          const actual2 = Array.isArray(read2.value) ? read2.value.join(", ") : read2.value || "";
          return one2(test(spec.pattern)(actual2), "", cut4(actual2));
        }
        const actual = Array.isArray(read2.value) ? read2.value : [read2.value || ""];
        const want = [...spec.values || []].sort();
        const have = [...actual].sort();
        return one2(want.length === have.length && want.every((v, i) => v === have[i]), "", actual);
      }
      case "attr": {
        const actual = el.getAttribute(spec.name);
        if (!spec.pattern) return one2(actual !== null, actual === null ? "no such attribute" : "", actual);
        return one2(actual !== null && test(spec.pattern)(actual), actual === null ? "no such attribute" : "", actual === null ? null : cut4(actual));
      }
      case "class": {
        const actual = norm(el.getAttribute("class"));
        if (spec.pattern) return one2(test(spec.pattern)(actual), "", cut4(actual));
        const want = norm(spec.name).split(" ");
        return one2(want.every((c) => el.classList.contains(c)), "", cut4(actual));
      }
      case "css": {
        const style = el.ownerDocument.defaultView.getComputedStyle(el);
        const actual = style.getPropertyValue(spec.name);
        return one2(cssMatches(actual, spec.pattern), "", cut4(actual));
      }
      default:
        throw new ArgError(`unknown matcher ${m}`);
    }
  }
  function expectPage(spec) {
    if (spec.matcher === "title") {
      const actual = document.title;
      return { pass: test(spec.pattern)(actual), actual: cut4(actual), count: 0 };
    }
    throw new ArgError(`unknown page check ${spec.matcher}`);
  }
  function expectNamed(els) {
    const shown3 = els.slice(0, 3);
    return { json: JSON.stringify(shown3.map(describe)), els: shown3 };
  }

  // src/ariaspec.ts
  var ROLES = /* @__PURE__ */ new Set([
    "alert",
    "alertdialog",
    "application",
    "article",
    "banner",
    "blockquote",
    "button",
    "caption",
    "cell",
    "checkbox",
    "code",
    "columnheader",
    "combobox",
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
    "gridcell",
    "group",
    "heading",
    "img",
    "insertion",
    "link",
    "list",
    "listbox",
    "listitem",
    "log",
    "main",
    "marquee",
    "math",
    "menu",
    "menubar",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "meter",
    "navigation",
    "none",
    "note",
    "option",
    "paragraph",
    "presentation",
    "progressbar",
    "radio",
    "radiogroup",
    "region",
    "row",
    "rowgroup",
    "rowheader",
    "scrollbar",
    "search",
    "searchbox",
    "separator",
    "slider",
    "spinbutton",
    "status",
    "strong",
    "subscript",
    "superscript",
    "switch",
    "tab",
    "table",
    "tablist",
    "tabpanel",
    "term",
    "textbox",
    "time",
    "timer",
    "toolbar",
    "tooltip",
    "tree",
    "treegrid",
    "treeitem",
    "image",
    "mark",
    "comment",
    "suggestion",
    "graphics-document",
    "graphics-object",
    "graphics-symbol",
    "doc-abstract",
    "doc-acknowledgments",
    "doc-afterword",
    "doc-appendix",
    "doc-backlink",
    "doc-biblioentry",
    "doc-bibliography",
    "doc-biblioref",
    "doc-chapter",
    "doc-colophon",
    "doc-conclusion",
    "doc-cover",
    "doc-credit",
    "doc-credits",
    "doc-dedication",
    "doc-endnote",
    "doc-endnotes",
    "doc-epigraph",
    "doc-epilogue",
    "doc-errata",
    "doc-example",
    "doc-footnote",
    "doc-foreword",
    "doc-glossary",
    "doc-glossref",
    "doc-index",
    "doc-introduction",
    "doc-noteref",
    "doc-notice",
    "doc-pagebreak",
    "doc-pagefooter",
    "doc-pageheader",
    "doc-pagelist",
    "doc-part",
    "doc-preface",
    "doc-prologue",
    "doc-pullquote",
    "doc-qna",
    "doc-subtitle",
    "doc-tip",
    "doc-toc"
  ]);
  var ATTRS = {
    "aria-activedescendant": "idref",
    "aria-atomic": "bool",
    "aria-autocomplete": ["inline", "list", "both", "none"],
    "aria-braillelabel": "string",
    "aria-brailleroledescription": "string",
    "aria-busy": "bool",
    "aria-checked": ["true", "false", "mixed", "undefined"],
    "aria-colcount": "int",
    "aria-colindex": "int",
    "aria-colindextext": "string",
    "aria-colspan": "int",
    "aria-controls": "idrefs",
    "aria-current": ["page", "step", "location", "date", "time", "true", "false"],
    "aria-describedby": "idrefs",
    "aria-description": "string",
    "aria-details": "idrefs",
    "aria-disabled": "bool",
    "aria-dropeffect": { list: ["copy", "execute", "link", "move", "none", "popup"] },
    "aria-errormessage": "idrefs",
    "aria-expanded": "bool-undefined",
    "aria-flowto": "idrefs",
    "aria-grabbed": "bool-undefined",
    "aria-haspopup": ["false", "true", "menu", "listbox", "tree", "grid", "dialog"],
    "aria-hidden": "bool-undefined",
    // Any other value counts as `true`, by the spec: nothing is wrong.
    "aria-invalid": "string",
    "aria-keyshortcuts": "string",
    "aria-label": "string",
    "aria-labelledby": "idrefs",
    "aria-level": "int",
    "aria-live": ["assertive", "off", "polite"],
    "aria-modal": "bool",
    "aria-multiline": "bool",
    "aria-multiselectable": "bool",
    "aria-orientation": ["horizontal", "vertical", "undefined"],
    "aria-owns": "idrefs",
    "aria-placeholder": "string",
    "aria-posinset": "int",
    "aria-pressed": ["true", "false", "mixed", "undefined"],
    "aria-readonly": "bool",
    "aria-relevant": { list: ["additions", "removals", "text", "all"] },
    "aria-required": "bool",
    "aria-roledescription": "string",
    "aria-rowcount": "int",
    "aria-rowindex": "int",
    "aria-rowindextext": "string",
    "aria-rowspan": "int",
    "aria-selected": "bool-undefined",
    "aria-setsize": "int",
    "aria-sort": ["ascending", "descending", "none", "other"],
    "aria-valuemax": "number",
    "aria-valuemin": "number",
    "aria-valuenow": "number",
    "aria-valuetext": "string"
  };
  function badValue(name, value2, exists) {
    const kind = ATTRS[name];
    const v = value2.trim();
    if (!kind || !v || kind === "string") return null;
    const shown3 = `${name}="${value2}"`;
    const tokens = (allowed) => `${shown3}: not ${allowed.map((t) => `"${t}"`).join(", ").replace(/, ([^,]*)$/, " or $1")}`;
    const lower = v.toLowerCase();
    if (kind === "bool") return ["true", "false"].includes(lower) ? null : tokens(["true", "false"]);
    if (kind === "bool-undefined") {
      return ["true", "false", "undefined"].includes(lower) ? null : tokens(["true", "false", "undefined"]);
    }
    if (kind === "int") return /^-?\d+$/.test(v) ? null : `${shown3}: not a whole number`;
    if (kind === "number") return /^-?(\d+\.?\d*|\.\d+)([eE][-+]?\d+)?$/.test(v) ? null : `${shown3}: not a number`;
    if (kind === "idref" || kind === "idrefs") {
      const ids = v.split(/\s+/);
      if (kind === "idref" && ids.length > 1) return `${shown3}: takes one id`;
      const missing = ids.filter((id) => !exists(id));
      if (!missing.length) return null;
      return `${shown3}: no element has id ${missing.map((id) => `"${id}"`).join(", ")}`;
    }
    if (Array.isArray(kind)) return kind.includes(lower) ? null : tokens(kind);
    const bad = lower.split(/\s+/).find((t) => !kind.list.includes(t));
    return bad === void 0 ? null : `${shown3}: "${bad}" is not ${kind.list.map((t) => `"${t}"`).join(", ").replace(/, ([^,]*)$/, " or $1")}`;
  }
  var REQUIRED_ATTRS = {
    checkbox: ["aria-checked"],
    combobox: ["aria-expanded"],
    heading: ["aria-level"],
    menuitemcheckbox: ["aria-checked"],
    menuitemradio: ["aria-checked"],
    meter: ["aria-valuenow"],
    radio: ["aria-checked"],
    scrollbar: ["aria-controls", "aria-valuenow"],
    slider: ["aria-valuenow"],
    switch: ["aria-checked"]
  };
  var REQUIRED_CHILDREN = {
    feed: ["article"],
    grid: ["row", "rowgroup"],
    list: ["listitem"],
    listbox: ["option", "group"],
    menu: ["menuitem", "menuitemcheckbox", "menuitemradio", "group", "separator"],
    menubar: ["menuitem", "menuitemcheckbox", "menuitemradio", "group", "separator"],
    row: ["cell", "columnheader", "gridcell", "rowheader"],
    rowgroup: ["row"],
    table: ["row", "rowgroup", "caption"],
    tablist: ["tab"],
    tree: ["treeitem", "group"],
    treegrid: ["row", "rowgroup"]
  };
  var REQUIRED_PARENT = {
    cell: ["row"],
    columnheader: ["row"],
    gridcell: ["row"],
    listitem: ["list", "directory"],
    menuitem: ["menu", "menubar", "group"],
    menuitemcheckbox: ["menu", "menubar", "group"],
    menuitemradio: ["menu", "menubar", "group"],
    option: ["listbox", "group"],
    row: ["grid", "rowgroup", "table", "treegrid"],
    rowgroup: ["grid", "table", "treegrid"],
    rowheader: ["row"],
    tab: ["tablist"],
    treeitem: ["tree", "group"]
  };
  var FIELDS = /* @__PURE__ */ new Set([
    "name",
    "honorific-prefix",
    "given-name",
    "additional-name",
    "family-name",
    "honorific-suffix",
    "nickname",
    "username",
    "new-password",
    "current-password",
    "one-time-code",
    "organization-title",
    "organization",
    "street-address",
    "address-line1",
    "address-line2",
    "address-line3",
    "address-level4",
    "address-level3",
    "address-level2",
    "address-level1",
    "country",
    "country-name",
    "postal-code",
    "cc-name",
    "cc-given-name",
    "cc-additional-name",
    "cc-family-name",
    "cc-number",
    "cc-exp",
    "cc-exp-month",
    "cc-exp-year",
    "cc-csc",
    "cc-type",
    "transaction-currency",
    "transaction-amount",
    "language",
    "bday",
    "bday-day",
    "bday-month",
    "bday-year",
    "sex",
    "url",
    "photo"
  ]);
  var CONTACT = /* @__PURE__ */ new Set([
    "tel",
    "tel-country-code",
    "tel-national",
    "tel-area-code",
    "tel-local",
    "tel-local-prefix",
    "tel-local-suffix",
    "tel-extension",
    "email",
    "impp"
  ]);
  function badAutocomplete(value2) {
    const tokens = value2.trim().toLowerCase().split(/\s+/).filter(Boolean);
    if (!tokens.length) return null;
    if (tokens.length === 1 && (tokens[0] === "on" || tokens[0] === "off")) return null;
    let i = 0;
    if (tokens[i]?.startsWith("section-")) i++;
    if (tokens[i] === "shipping" || tokens[i] === "billing") i++;
    const contact = ["home", "work", "mobile", "fax", "pager"].includes(tokens[i]);
    if (contact) i++;
    const field2 = tokens[i];
    const known = field2 !== void 0 && (!contact && FIELDS.has(field2) || CONTACT.has(field2));
    if (!known) {
      if (field2 === void 0) return `autocomplete="${value2}": no field name`;
      if (contact && FIELDS.has(field2)) return `autocomplete="${value2}": "${tokens[i - 1]}" only goes with tel, email or impp`;
      const near = closest2(field2, [...FIELDS, ...CONTACT]);
      return `autocomplete="${value2}": "${field2}" is not a field name${near ? ` (did you mean ${near}?)` : ""}`;
    }
    i++;
    if (tokens[i] === "webauthn") i++;
    if (i < tokens.length) return `autocomplete="${value2}": "${tokens[i]}" is out of place`;
    return null;
  }
  function closest2(word, known) {
    const words = Array.from(known);
    if (word.length >= 3) {
      const longer = words.filter((k) => k.startsWith(word)).sort((a2, b) => a2.length - b.length)[0];
      if (longer) return longer;
    }
    let best = null;
    let bestDistance = 3;
    for (const k of words) {
      const d = distance(word, k);
      if (d < bestDistance) {
        best = k;
        bestDistance = d;
      }
    }
    return best;
  }
  function distance(a2, b) {
    let prev = Array.from({ length: b.length + 1 }, (_, j) => j);
    for (let i = 1; i <= a2.length; i++) {
      const row = [i];
      for (let j = 1; j <= b.length; j++) {
        row.push(Math.min(prev[j] + 1, row[j - 1] + 1, prev[j - 1] + (a2[i - 1] === b[j - 1] ? 0 : 1)));
      }
      prev = row;
    }
    return prev[b.length];
  }

  // src/a11y.ts
  var Context = class {
    constructor(all, hidden3) {
      this.all = all;
      this.hidden = hidden3;
    }
    all;
    hidden;
    memo = /* @__PURE__ */ new Map();
    once(key, make) {
      if (!this.memo.has(key)) this.memo.set(key, make());
      return this.memo.get(key);
    }
    /** Elements shown to assistive tech. */
    shown() {
      return this.once("shown", () => this.all.filter((el) => !this.hidden.get(el)));
    }
    /** The elements that `aria-owns` moves, and their owner. */
    owners() {
      return this.once("owners", () => {
        const out = /* @__PURE__ */ new Map();
        for (const el of this.all) {
          const ids = el.getAttribute("aria-owns");
          if (!ids) continue;
          for (const owned of idsIn(el, ids)) out.set(owned, el);
        }
        return out;
      });
    }
    /** Each heading's previous heading's level (in document order). */
    previousLevels() {
      return this.once("headings", () => {
        const out = /* @__PURE__ */ new Map();
        let previous = 0;
        for (const el of this.shown()) {
          if (roleOf(el) !== "heading") continue;
          if (previous) out.set(el, previous);
          previous = level(el, "heading") || 2;
        }
        return out;
      });
    }
    /** A root's elements by id, and which ids something refers to (and
     * how). */
    ids(root) {
      const key = root instanceof ShadowRoot ? root : root.documentElement;
      let roots = this.memo.get("ids");
      if (!roots) this.memo.set("ids", roots = /* @__PURE__ */ new Map());
      let found = roots.get(key);
      if (found) return found;
      found = { byId: /* @__PURE__ */ new Map(), referred: /* @__PURE__ */ new Map() };
      for (const el of Array.from(root.querySelectorAll("*"))) {
        const id = el.id;
        if (id) {
          const list = found.byId.get(id);
          if (list) list.push(el);
          else found.byId.set(id, [el]);
        }
        if (tag(el) === "label" && el.getAttribute("for")) {
          const id2 = el.getAttribute("for");
          if (!found.referred.has(id2)) found.referred.set(id2, "a <label for>");
        }
        for (const name of IDREFS) {
          const ids = el.getAttribute(name);
          if (!ids) continue;
          for (const id2 of ids.trim().split(/\s+/)) if (!found.referred.has(id2)) found.referred.set(id2, name);
        }
      }
      roots.set(key, found);
      return found;
    }
    /** The pointer targets shown: their box and whether it is under
     * 24×24, by element. */
    targets() {
      return this.once("targets", () => new Map(this.shown().filter((el) => TARGETS.has(roleOf(el) || "") && !disabled(el) && visible(el)).map((el) => {
        const box = el.getBoundingClientRect();
        return [el, { el, box, small: box.width < 24 - 0.01 || box.height < 24 - 0.01 }];
      })));
    }
    /** How many elements `focus-visible` focused. */
    focused = 0;
  };
  var IDREFS = Object.keys(ATTRS).filter((name) => ATTRS[name] === "idref" || ATTRS[name] === "idrefs");
  function idsIn(el, ids) {
    const root = el.getRootNode();
    return ids.trim().split(/\s+/).map((id) => root.getElementById(id)).filter((e) => !!e);
  }
  function hasId(el, id) {
    return !!el.getRootNode().getElementById(id);
  }
  function explicitRole(el) {
    const first = (el.getAttribute("role") || "").trim().toLowerCase().split(/\s+/)[0];
    return first || null;
  }
  var TRANSPARENT = /* @__PURE__ */ new Set(["none", "presentation", "generic"]);
  function owner(el, ctx) {
    return ctx.owners().get(el) || parentOf(el);
  }
  function a(role) {
    return /^[aeiou]/.test(role) ? `an ${role}` : `a ${role}`;
  }
  function either(roles) {
    return roles.length < 2 ? roles.join("") : `${roles.slice(0, -1).join(", ")} or ${roles[roles.length - 1]}`;
  }
  function ownedOf(el, ctx) {
    const out = [];
    const visit = (parent, depth) => {
      const kids = [...Array.from((parent.shadowRoot || parent).children)];
      if (parent === el) {
        const ids = el.getAttribute("aria-owns");
        if (ids) kids.push(...idsIn(el, ids));
      }
      for (const kid of kids) {
        if (["script", "template", "style"].includes(tag(kid)) || ctx.hidden.get(kid) || hiddenForAria(kid)) continue;
        const role = roleOf(kid);
        if (!role) continue;
        if (TRANSPARENT.has(role) && depth < 20) visit(kid, depth + 1);
        else out.push(kid);
      }
    };
    visit(el, 0);
    return out;
  }
  var TARGETS = new Set([...INTERACTIVE].filter((r) => r !== "iframe"));
  function targetSize(el, ctx) {
    const me = ctx.targets().get(el);
    if (!me || !me.small) return null;
    const style = getComputedStyle(el);
    if (style.display === "inline") {
      const parent = el.parentElement;
      if (parent && norm(parent.textContent).length > norm(el.textContent).length) return null;
    }
    if (uaSized(el, style)) return null;
    const cx = me.box.left + me.box.width / 2;
    const cy = me.box.top + me.box.height / 2;
    for (const other of ctx.targets().values()) {
      if (other.el === el || other.el.contains(el) || el.contains(other.el)) continue;
      const b = other.box;
      const dx = Math.max(b.left - cx, 0, cx - b.right);
      const dy = Math.max(b.top - cy, 0, cy - b.bottom);
      let near = Math.hypot(dx, dy) < 12;
      if (!near && other.small) {
        near = Math.hypot(b.left + b.width / 2 - cx, b.top + b.height / 2 - cy) < 24;
      }
      if (near) return `${Math.round(me.box.width)}×${Math.round(me.box.height)}px, ${describeShort(other.el)} is within 12px of its centre`;
    }
    return null;
  }
  function uaSized(el, style) {
    if (!["input", "button", "select", "textarea"].includes(tag(el))) return false;
    return style.appearance !== "none" && Math.abs(parseFloat(style.fontSize) - 40 / 3) < 0.01;
  }
  function describeShort(el) {
    const c = describe(el);
    return c.name ? `${c.role} "${c.name}"` : c.role;
  }
  var FOCUS_CHECKED = 300;
  function look(el) {
    const parts = [];
    const add = (e, pseudo) => {
      const s = getComputedStyle(e, pseudo);
      if (pseudo && (s.content === "none" || s.content === "normal")) return;
      const outline2 = s.outlineStyle === "none" || parseFloat(s.outlineWidth) === 0 ? "none" : `${s.outlineStyle} ${s.outlineWidth} ${s.outlineColor} ${s.outlineOffset}`;
      const borders = ["Top", "Right", "Bottom", "Left"].map((side) => {
        const get = (p) => s.getPropertyValue(`border-${side.toLowerCase()}-${p}`);
        return get("style") === "none" || parseFloat(get("width")) === 0 ? "-" : `${get("width")} ${get("style")} ${get("color")}`;
      });
      parts.push(
        outline2,
        s.boxShadow,
        borders.join(","),
        s.backgroundColor,
        s.backgroundImage,
        s.color,
        s.textDecorationLine,
        s.opacity,
        s.transform,
        pseudo ? s.content : ""
      );
    };
    add(el);
    add(el, "::before");
    add(el, "::after");
    let up = parentOf(el);
    for (let i = 0; up && i < 3; i++, up = parentOf(up)) add(up);
    descendants(el).slice(0, 10).forEach((d) => add(d));
    return parts.join("|");
  }
  function noFocusChange(el, ctx) {
    if (!focusable(el) || tag(el) === "iframe" || !visible(el) || ctx.focused >= FOCUS_CHECKED) return false;
    ctx.focused++;
    const before2 = look(el);
    el.focus({ preventScroll: true, focusVisible: true });
    if (activeElement() !== el) return false;
    if (!el.matches(":focus-visible")) {
      el.blur();
      return false;
    }
    const same = !el.getAnimations().length && look(el) === before2;
    el.blur();
    return same;
  }
  function field(el) {
    const t = tag(el);
    if (t === "select" || t === "textarea") return true;
    if (t !== "input") return false;
    const type = (el.type || "text").toLowerCase();
    return !["hidden", "button", "submit", "reset", "image"].includes(type);
  }
  function focusable(el) {
    const h = el;
    if (h.tabIndex < 0 || disabled(el)) return false;
    if (h.tabIndex > 0 || el.hasAttribute("tabindex")) return true;
    const t = tag(el);
    if (t === "a" || t === "area") return el.hasAttribute("href");
    return ["button", "input", "select", "textarea", "iframe", "summary"].includes(t) || h.isContentEditable;
  }
  function parseColour(s) {
    const m = /^rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)(?:\s*[,/]\s*([\d.]+%?))?\s*\)$/.exec(s.trim());
    if (!m) return null;
    let a2 = m[4] === void 0 ? 1 : parseFloat(m[4]);
    if (m[4] && m[4].endsWith("%")) a2 /= 100;
    return [parseFloat(m[1]), parseFloat(m[2]), parseFloat(m[3]), a2];
  }
  function over(top, under) {
    const a2 = top[3];
    return [0, 1, 2].map((i) => top[i] * a2 + under[i] * (1 - a2)).concat(1);
  }
  function luminance(c) {
    const [r, g, b] = [0, 1, 2].map((i) => {
      const v = c[i] / 255;
      return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  }
  function background(el) {
    const layers = [];
    for (let a2 = el; a2; a2 = a2.parentElement) {
      const style = getComputedStyle(a2);
      if (style.backgroundImage !== "none") return null;
      const c = parseColour(style.backgroundColor);
      if (c && c[3] > 0) {
        layers.push(c);
        if (c[3] >= 1) break;
      }
    }
    let colour3 = [255, 255, 255, 1];
    for (let i = layers.length - 1; i >= 0; i--) colour3 = over(layers[i], colour3);
    return colour3;
  }
  function fade(el) {
    let out = 1;
    for (let a2 = el; a2; a2 = a2.parentElement) out *= parseFloat(getComputedStyle(a2).opacity);
    return out;
  }
  function contrast(el) {
    const own = Array.from(el.childNodes).some((n) => n.nodeType === Node.TEXT_NODE && n.data.trim());
    if (!own || !visible(el) || disabled(el)) return null;
    const style = getComputedStyle(el);
    if (style.textShadow !== "none") return null;
    const fg = parseColour(style.color);
    const bg = background(el);
    if (!fg || !bg) return null;
    const text2 = over([fg[0], fg[1], fg[2], fg[3] * fade(el)], bg);
    const [l1, l2] = [luminance(text2), luminance(bg)].sort((x, y) => y - x);
    const ratio = (l1 + 0.05) / (l2 + 0.05);
    const size2 = parseFloat(style.fontSize);
    const large = size2 >= 24 || size2 >= 18.66 && parseInt(style.fontWeight, 10) >= 700;
    const needed = large ? 3 : 4.5;
    if (ratio >= needed) return null;
    return `contrast ${ratio.toFixed(2)}:1, needs ${needed}:1`;
  }
  var RULES = [
    {
      id: "document-title",
      wcag: "2.4.2",
      level: "A",
      help: "the page has no <title>",
      page: (doc) => doc.title.trim() ? null : "the page has no <title>"
    },
    {
      id: "html-has-lang",
      wcag: "3.1.1",
      level: "A",
      help: "<html> has no lang attribute",
      page: (doc) => doc.documentElement.getAttribute("lang")?.trim() ? null : "<html> has no lang attribute"
    },
    {
      id: "image-alt",
      wcag: "1.1.1",
      level: "A",
      help: "has no alt text",
      check: (el) => tag(el) === "img" && roleOf(el) === "img" && !el.hasAttribute("alt") && !nameOf(el, "img")
    },
    {
      id: "input-image-alt",
      wcag: "1.1.1",
      level: "A",
      help: "is an image button with no alt text",
      check: (el) => tag(el) === "input" && el.type === "image" && !(el.getAttribute("alt") || el.getAttribute("aria-label") || el.getAttribute("title") || "").trim()
    },
    {
      id: "button-name",
      wcag: "4.1.2",
      level: "A",
      help: "is a button with no name",
      check: (el) => roleOf(el) === "button" && tag(el) !== "input" && !nameOf(el, "button")
    },
    {
      id: "link-name",
      wcag: "2.4.4",
      level: "A",
      help: "is a link with no name",
      check: (el) => roleOf(el) === "link" && !nameOf(el, "link")
    },
    {
      id: "label",
      wcag: "4.1.2",
      level: "A",
      help: "is a form field with no label",
      check: (el) => field(el) && !nameOf(el, roleOf(el) || "textbox")
    },
    {
      id: "frame-title",
      wcag: "4.1.2",
      level: "A",
      help: "is a frame with no title",
      check: (el) => (tag(el) === "iframe" || tag(el) === "frame") && !nameOf(el, "iframe")
    },
    {
      id: "aria-hidden-focus",
      wcag: "4.1.2",
      level: "A",
      help: 'can take focus inside aria-hidden="true"',
      check: (el) => focusable(el) && !!el.closest('[aria-hidden="true"]')
    },
    {
      id: "aria-valid-role",
      wcag: "4.1.2",
      level: "A",
      help: "has a role that is not an ARIA role",
      check: (el) => {
        const role = explicitRole(el);
        if (!role || ROLES.has(role)) return false;
        const near = closest2(role, ROLES);
        return `role="${el.getAttribute("role")}"${near ? ` (did you mean ${near}?)` : ""}`;
      }
    },
    {
      id: "aria-valid-attr",
      wcag: "4.1.2",
      level: "A",
      help: "has an aria-* attribute that does not exist",
      check: (el) => {
        const bad = el.getAttributeNames().find((n) => n.startsWith("aria-") && !(n in ATTRS));
        if (!bad) return false;
        const near = closest2(bad, Object.keys(ATTRS));
        return `${bad}${near ? ` (did you mean ${near}?)` : ""}`;
      }
    },
    {
      id: "aria-valid-attr-value",
      wcag: "4.1.2",
      level: "A",
      help: "has an aria-* attribute with a wrong value",
      check: (el) => {
        for (const name of el.getAttributeNames()) {
          if (!(name in ATTRS)) continue;
          const lenient = name === "aria-controls" && el.getAttribute("aria-expanded") === "false";
          const why = badValue(name, el.getAttribute(name) || "", (id) => lenient || hasId(el, id));
          if (why) return why;
        }
        return false;
      }
    },
    {
      id: "aria-required-attr",
      wcag: "4.1.2",
      level: "A",
      help: "has a role without an attribute it needs",
      check: (el) => {
        const role = explicitRole(el);
        const needs = role ? REQUIRED_ATTRS[role] : void 0;
        if (!role || !needs) return false;
        const t = tag(el);
        const type = (el.type || "").toLowerCase();
        if (t === "input" && ["checkbox", "radio", "range", "number"].includes(type)) return false;
        if (["select", "meter", "progress", "h1", "h2", "h3", "h4", "h5", "h6"].includes(t)) return false;
        const missing = needs.filter((attr2) => !el.hasAttribute(attr2));
        return missing.length ? `role=${role} needs ${missing.join(" and ")}` : false;
      }
    },
    {
      id: "aria-required-children",
      wcag: "1.3.1",
      level: "A",
      help: "owns elements its role does not allow",
      check: (el, ctx) => {
        const role = explicitRole(el);
        const allowed = role ? REQUIRED_CHILDREN[role] : void 0;
        if (!role || !allowed || roleOf(el) !== role || implicitRole(el) === role) return false;
        const bad = ownedOf(el, ctx).find((o) => !allowed.includes(roleOf(o) || ""));
        if (!bad) return false;
        return `owns ${describeShort(bad).replace(/^(\w+)/, (r) => a(r))}; ${a(role)} owns ${either(allowed)}`;
      }
    },
    {
      id: "aria-required-parent",
      wcag: "1.3.1",
      level: "A",
      help: "is not inside the role it needs",
      check: (el, ctx) => {
        const role = explicitRole(el) || (tag(el) === "li" ? "listitem" : null);
        const allowed = role ? REQUIRED_PARENT[role] : void 0;
        if (!role || !allowed || roleOf(el) !== role) return false;
        const parent = el.parentElement;
        if (!explicitRole(el) && parent && ["ul", "ol", "menu"].includes(tag(parent))) return false;
        for (let o = owner(el, ctx); o; o = owner(o, ctx)) {
          const r = roleOf(o);
          if (!r || TRANSPARENT.has(r)) continue;
          return allowed.includes(r) ? false : `${a(role)} needs ${a(either(allowed))} parent, not ${a(r)}`;
        }
        return `${a(role)} needs ${a(either(allowed))} parent`;
      }
    },
    {
      id: "duplicate-id-referenced",
      wcag: "4.1.2",
      level: "A",
      help: "has the id of an earlier element, which something refers to",
      check: (el, ctx) => {
        const id = el.id;
        if (!id) return false;
        const { byId, referred } = ctx.ids(el.getRootNode());
        const same = byId.get(id) || [];
        if (same.length < 2 || same[0] === el || !referred.has(id)) return false;
        return `id "${id}" (on ${same.length} elements; ${referred.get(id)} refers to it)`;
      }
    },
    {
      id: "list",
      wcag: "1.3.1",
      level: "A",
      help: "is a list with children that are not <li>",
      check: (el) => {
        if (tag(el) !== "ul" && tag(el) !== "ol" || roleOf(el) !== "list") return false;
        const bad = Array.from(el.children).find((c) => !["li", "script", "template"].includes(tag(c)));
        return bad ? `has a <${tag(bad)}>` : false;
      }
    },
    {
      id: "table-headers",
      wcag: "1.3.1",
      level: "A",
      help: "is a data table with no header cells",
      ax: "data-table",
      check: (el) => {
        if (tag(el) !== "table" || roleOf(el) !== "table") return false;
        const mine = (e) => e.closest("table") === el;
        if (!Array.from(el.querySelectorAll("td")).some(mine)) return false;
        return !Array.from(el.querySelectorAll("th, [role=columnheader], [role=rowheader]")).some(mine);
      }
    },
    {
      id: "heading-order",
      wcag: "1.3.1",
      level: "A",
      help: "skips a heading level",
      check: (el, ctx) => {
        const previous = ctx.previousLevels().get(el);
        const level2 = level(el, "heading") || 2;
        return previous !== void 0 && level2 > previous + 1 ? `level ${previous} → ${level2}` : false;
      }
    },
    {
      id: "empty-heading",
      wcag: "1.3.1",
      level: "A",
      help: "is a heading with no text",
      ax: "no-name",
      check: (el) => roleOf(el) === "heading" && !nameOf(el, "heading")
    },
    {
      id: "landmark-one-main",
      wcag: "1.3.1",
      level: "A",
      help: "the page needs one main landmark",
      page: (_doc, ctx) => {
        const mains = ctx.shown().filter((el) => roleOf(el) === "main" && visible(el));
        if (mains.length === 1) return null;
        return mains.length ? `the page has ${mains.length} visible main landmarks` : "the page has no main landmark";
      }
    },
    {
      id: "meta-viewport",
      wcag: "1.4.4",
      level: "AA",
      help: "the viewport meta tag stops zooming",
      page: (doc) => {
        const meta = doc.querySelector('meta[name="viewport"]');
        const content = (meta?.getAttribute("content") || "").toLowerCase().replace(/\s/g, "");
        if (/user-scalable=(no|0)(,|$)/.test(content)) return "the viewport meta tag has user-scalable=no";
        const max = /maximum-scale=([\d.]+)/.exec(content);
        if (max && parseFloat(max[1]) < 2) return `the viewport meta tag has maximum-scale=${max[1]}`;
        return null;
      }
    },
    {
      id: "color-contrast",
      wcag: "1.4.3",
      level: "AA",
      help: "has text with too little contrast",
      check: (el) => contrast(el) || false
    },
    {
      id: "autocomplete-valid",
      wcag: "1.3.5",
      level: "AA",
      help: "has an autocomplete value browsers do not know",
      check: (el) => {
        if (!["input", "select", "textarea"].includes(tag(el))) return false;
        const value2 = el.getAttribute("autocomplete");
        return value2 ? badAutocomplete(value2) || false : false;
      }
    },
    {
      id: "target-size",
      wcag: "2.5.8",
      level: "AA",
      help: "is a pointer target under 24×24px, close to another",
      check: (el, ctx) => targetSize(el, ctx) || false
    },
    {
      id: "focus-visible",
      wcag: "2.4.7",
      level: "AA",
      help: "looks the same when it has keyboard focus",
      check: (el, ctx) => noFocusChange(el, ctx)
    }
  ];
  var LISTED3 = 5;
  var AX_LISTED = 20;
  var ELEMENTS = 200;
  var focusBefore = null;
  function refocus(el) {
    if (el && el !== document.body && el !== document.documentElement) {
      el.focus({ preventScroll: true });
    } else if (document.activeElement && document.activeElement !== document.body) {
      document.activeElement.blur();
    }
  }
  function a11y(root, level2, page = true, perRule = LISTED3) {
    const rules = RULES.filter((r) => level2 === "AA" || r.level === "A");
    const violations = [];
    const els = [];
    const scope = root || document.documentElement;
    const all = [scope, ...descendants(scope)];
    const hidden3 = new Map(all.map((el) => [el, hiddenForAria(el)]));
    const ctx = new Context(all, hidden3);
    const before2 = activeElement();
    if (page) focusBefore = before2;
    for (const rule of rules) {
      const v = { rule: rule.id, wcag: rule.wcag, level: rule.level, help: rule.help, count: 0, nodes: [] };
      if (rule.ax) v.ax = rule.ax;
      if (rule.page) {
        if (root || !page) continue;
        const why = rule.page(document, ctx);
        if (why) violations.push({ ...v, count: 1, page: why });
        continue;
      }
      const listed2 = rule.ax ? Math.max(AX_LISTED, perRule) : perRule;
      for (const el of all) {
        if (rule.id === "aria-hidden-focus" ? !hidden3.get(el) || !visible(el) : hidden3.get(el)) continue;
        const found = rule.check?.(el, ctx);
        if (!found) continue;
        v.count++;
        if (v.nodes.length < listed2 && els.length < ELEMENTS) {
          v.nodes.push({ candidate: describe(el), why: typeof found === "string" ? found : void 0, index: els.length });
          els.push(el);
        }
      }
      if (v.count) violations.push(v);
    }
    if (ctx.focused) refocus(before2);
    return { json: JSON.stringify({ rules: rules.length, violations }), els };
  }
  function a11yRefocus() {
    refocus(focusBefore);
  }

  // src/audit.ts
  var FORMS = 50;
  function metaNamed(doc, name) {
    for (const meta of Array.from(doc.querySelectorAll("meta[name]"))) {
      if ((meta.getAttribute("name") || "").trim().toLowerCase() === name) {
        return meta.getAttribute("content") || "";
      }
    }
    return null;
  }
  function auditPage(root) {
    const doc = document;
    const scope = root || doc;
    const csp = Array.from(doc.querySelectorAll("meta[http-equiv]")).some(
      (m) => (m.getAttribute("http-equiv") || "").trim().toLowerCase() === "content-security-policy"
    );
    const els = [];
    const forms = [];
    const all = Array.from(scope.querySelectorAll("form"));
    if (root && root.tagName.toLowerCase() === "form") all.unshift(root);
    for (const form of all.slice(0, FORMS)) {
      const f = form;
      forms.push({
        candidate: describe(f),
        method: (f.getAttribute("method") || "get").toLowerCase(),
        // `form.action` resolves against the base URL; an input named
        // `action` shadows it.
        action: typeof f.action === "string" ? f.action : new URL(f.getAttribute("action") || "", doc.baseURI).href,
        index: els.length
      });
      els.push(f);
    }
    const password = scope.querySelector("input[type=password i]") !== null;
    const title = doc.querySelector("title") ? doc.title.trim() : null;
    const html2 = doc.documentElement;
    const json = {
      title,
      lang: html2 && html2.hasAttribute("lang") ? (html2.getAttribute("lang") || "").trim() : null,
      viewport: metaNamed(doc, "viewport"),
      description: metaNamed(doc, "description"),
      robots: metaNamed(doc, "robots"),
      csp_meta: csp,
      password,
      forms
    };
    return { json: JSON.stringify(json), els };
  }

  // src/stable.ts
  var LONGEST = 80;
  var quoted = (s) => '"' + s.replace(/[\\"]/g, (m) => "\\" + m) + '"';
  var bare = (s) => !!s && !/^["'/]|>>|[\[\]]/.test(s);
  function candidates(el, testid, texts, own) {
    const out = [];
    const role = roleOf(el);
    if (role) {
      const name = nameOf(el, role);
      if (name && name.length <= LONGEST) {
        out.push({
          steps: [{ kind: "role", role, name: { mode: "exact", value: name } }],
          locator: `role=${role}[name=${quoted(name)}]`
        });
      }
    }
    for (const label of labelsOf(el)) {
      if (label.length <= LONGEST) {
        out.push({
          steps: [{ kind: "label", text: { mode: "exact", value: label } }],
          locator: `label=${quoted(label)}`
        });
      }
    }
    const id = el.getAttribute(testid);
    if (id && !/\s|>>|"|'/.test(id)) {
      const locator = testid === "data-testid" || own ? `testid=${id}` : `css=[${testid}=${quoted(id)}]`;
      out.push({ steps: [{ kind: "testid", value: id, attribute: testid }], locator });
    }
    const placeholder = norm(el.getAttribute("placeholder"));
    if (placeholder && placeholder.length <= LONGEST) {
      out.push({
        steps: [{ kind: "placeholder", text: { mode: "exact", value: placeholder } }],
        locator: `placeholder=${quoted(placeholder)}`
      });
    }
    const text2 = norm(texts.of(el));
    if (text2 && text2.length <= LONGEST) {
      out.push({
        steps: [{ kind: "text", text: { mode: "exact", value: text2 } }],
        locator: `text=${quoted(text2)}`
      });
    }
    return out;
  }
  function only(steps, el) {
    const found = query(steps, null);
    return found.length === 1 && found[0] === el;
  }
  function firstText(el) {
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const text2 = norm(node.data);
      if (text2) return text2.length > 40 ? text2.slice(0, 40).replace(/\s+\S*$/, "") : text2;
    }
    return "";
  }
  function containers(a2, role) {
    const out = [];
    const name = nameOf(a2, role);
    if (name && name.length <= LONGEST) {
      out.push({
        steps: [{ kind: "role", role, name: { mode: "exact", value: name } }],
        locator: `role=${role}[name=${quoted(name)}]`
      });
    }
    const text2 = firstText(a2);
    if (bare(text2)) {
      out.push({
        steps: [{ kind: "role", role }, { kind: "has-text", text: { mode: "substring", value: text2 } }],
        locator: `role=${role} >> has-text=${text2}`
      });
    }
    return out.filter((c) => only(c.steps, a2));
  }
  function leafText(a2, el, texts) {
    for (const leaf of Array.from(a2.querySelectorAll("*"))) {
      if (leaf.childElementCount > 0 || el.contains(leaf) || leaf.contains(el)) continue;
      const text2 = norm(texts.of(leaf));
      if (text2) return text2.length <= LONGEST ? text2 : "";
    }
    return "";
  }
  function* tried(el, testid, texts, short) {
    const own = candidates(el, testid, texts, short);
    for (const c of own) yield { ...c, how: c.steps[0].kind };
    if (own.length === 0) return;
    const around = [];
    for (let a2 = parentOf(el); a2; a2 = parentOf(a2)) {
      const role = roleOf(a2);
      if (role && CONTEXT_ROLES.has(role)) around.push([a2, role]);
    }
    for (const [a2, role] of around) {
      for (const outer of containers(a2, role)) {
        for (const c of own) {
          yield { steps: [...outer.steps, ...c.steps], locator: `${outer.locator} >> ${c.locator}`, how: "scoped" };
        }
      }
    }
    for (const [a2, role] of around) {
      const text2 = leafText(a2, el, texts);
      if (!text2) continue;
      const outer = [
        { kind: "role", role },
        { kind: "has", steps: [{ kind: "text", text: { mode: "exact", value: text2 } }] }
      ];
      if (!only(outer, a2)) continue;
      for (const c of own) {
        yield {
          steps: [...outer, ...c.steps],
          locator: `role=${role} >> has=text=${quoted(text2)} >> ${c.locator}`,
          how: "has"
        };
      }
    }
    const first = own[0];
    const found = query(first.steps, null);
    const index = found.indexOf(el);
    if (index >= 0 && found.length > 1) {
      yield { steps: [...first.steps, { kind: "nth", index }], locator: `${first.locator} >> nth=${index}`, how: "nth" };
    }
  }
  function stable(el, testid, all, short) {
    const texts = new Texts();
    const usable = [];
    const others = [];
    let best = null;
    let any = false;
    const own = /* @__PURE__ */ new Set(["role", "label", "testid", "placeholder", "text"]);
    for (const c of tried(el, testid || "data-testid", texts, !!short)) {
      if (best && own.has(best.how) && !own.has(c.how)) break;
      any = true;
      const found = query(c.steps, null);
      if (found.length === 1 && found[0] === el) {
        if (!best) best = { locator: c.locator, how: c.how };
        if (!all) break;
        usable.push({ locator: c.locator, count: 1 });
      } else if (all && c.how !== "scoped" && c.how !== "has" && found.includes(el)) {
        others.push({ locator: c.locator, count: found.length });
      }
    }
    const candidates2 = all ? { candidates: [...usable, ...others] } : {};
    if (best) return JSON.stringify({ ...best, ...candidates2 });
    const reason = any ? "no locator finds it (is it hidden?)" : "it has no role and name, label, test id, placeholder or text";
    return JSON.stringify({ reason, ...candidates2 });
  }

  // src/why.ts
  var Els = class {
    els = [];
    add(el) {
      if (!el) return void 0;
      const i = this.els.indexOf(el);
      return i >= 0 ? i : this.els.push(el) - 1;
    }
    /** `{json, els}`, with each element's name in `names`. */
    answer(json) {
      const names = this.els.map(named);
      return { json: JSON.stringify({ ...json, names }), els: this.els };
    }
  };
  var px = (n) => Math.round(n);
  var size = (r) => `${px(r.width)}×${px(r.height)}`;
  function animationName(a2) {
    if (typeof CSSAnimation !== "undefined" && a2 instanceof CSSAnimation) return `animation "${a2.animationName}"`;
    if (typeof CSSTransition !== "undefined" && a2 instanceof CSSTransition) return `transition of ${a2.transitionProperty}`;
    return a2.id ? `animation "${a2.id}"` : "a script animation";
  }
  function animations(el, els) {
    const running = (a2) => a2.getAnimations().filter((x) => x.playState === "running" && x.effect?.target === a2);
    const parts = [];
    let index;
    const own = running(el);
    if (own.length) parts.push(`${own.slice(0, 2).map(animationName).join(", ")} on it`);
    for (let a2 = parentOf(el); a2; a2 = parentOf(a2)) {
      const theirs = running(a2);
      if (!theirs.length) continue;
      index = els.add(a2);
      parts.push(`${theirs.slice(0, 2).map(animationName).join(", ")} on {el}`);
      break;
    }
    return { text: parts.join("; "), el: index };
  }
  async function explainActionable(el, needs) {
    const els = new Els();
    const checks = [];
    const add = (name, cause, ok) => {
      if (!cause) {
        checks.push({ name, ok: true, text: ok });
        return;
      }
      const el2 = els.add(cause.el);
      checks.push({ name, ok: false, text: cause.what, el: el2 });
    };
    if (!el.isConnected) {
      checks.push({ name: "visible", ok: false, text: "it was removed from the page" });
      return els.answer({ checks });
    }
    const box = el.getBoundingClientRect();
    add("visible", invisibleCause(el), `${size(box)} at ${px(box.x)},${px(box.y)}`);
    await frame();
    const before2 = el.getBoundingClientRect();
    await frame();
    const after = el.getBoundingClientRect();
    const moved = Math.hypot(after.x - before2.x, after.y - before2.y);
    const resized = before2.width !== after.width || before2.height !== after.height;
    if (moved || resized) {
      const what = moved ? `moved ${moved < 1 ? "less than 1" : px(moved)}px between two frames` : `changed size from ${size(before2)} to ${size(after)} between two frames`;
      const running = animations(el, els);
      const text2 = running.text ? `${what} (${running.text})` : what;
      checks.push({ name: "stable", ok: false, text: text2, el: running.el });
    } else {
      checks.push({ name: "stable", ok: true });
    }
    if (needs.enabled || needs.editable) add("enabled", disabledCause(el) || inertCause(el));
    if (needs.editable) {
      const cause = readOnly(el);
      if (cause) {
        checks.push({ name: "editable", ok: false, text: cause.why.replace(/^it has /, ""), final: cause.final });
      } else {
        checks.push({ name: "editable", ok: true });
      }
    }
    const none = pointerCause(el);
    const pointer = none ? { text: none.what, el: els.add(none.el) } : null;
    return els.answer({ checks, pointer, own: named(el) });
  }
  function clipped(el) {
    const els = new Els();
    const r = el.getBoundingClientRect();
    for (let a2 = parentOf(el); a2; a2 = parentOf(a2)) {
      const s = getComputedStyle(a2);
      const clips2 = (o) => o === "hidden" || o === "clip";
      const b = a2.getBoundingClientRect();
      const outX = clips2(s.overflowX) && (r.right <= b.left || r.left >= b.right);
      const outY = clips2(s.overflowY) && (r.bottom <= b.top || r.top >= b.bottom);
      if (outX || outY) {
        const overflow = outX ? s.overflowX : s.overflowY;
        return els.answer({ text: `clipped out of {el} (overflow: ${overflow})`, el: els.add(a2) });
      }
    }
    const view = el.ownerDocument.defaultView || window;
    const x = r.left + view.scrollX;
    const y = r.top + view.scrollY;
    if (r.width && r.height && (x + r.width <= 0 || y + r.height <= 0)) {
      return els.answer({ text: `it lies at a negative offset (x=${px(x)} y=${px(y)}), where no scroll reaches` });
    }
    return els.answer({});
  }
  var marks = /* @__PURE__ */ new Map();
  var offsetOf = (a2) => a2 ? [a2.scrollLeft, a2.scrollTop] : [scrollX, scrollY];
  function scrollMark(el) {
    marks = /* @__PURE__ */ new Map();
    const root = document.scrollingElement || document.documentElement;
    for (let a2 = parentOf(el); a2 && a2 !== root && a2 !== document.body; a2 = parentOf(a2)) {
      if (a2.scrollHeight > a2.clientHeight || a2.scrollWidth > a2.clientWidth) marks.set(a2, offsetOf(a2));
    }
    marks.set(null, offsetOf(null));
  }
  function scrolledSince() {
    const els = new Els();
    const moved = [];
    for (const [a2, [x, y]] of marks) {
      const [nx, ny] = offsetOf(a2);
      const dx = px(nx - x);
      const dy = px(ny - y);
      if (dx || dy) moved.push({ dx, dy, el: els.add(a2) });
    }
    marks = /* @__PURE__ */ new Map();
    return els.answer({ moved });
  }

  // src/layout.ts
  var px2 = (n) => Math.round(n);
  var boxOf = (r) => [r.x, r.y, r.width, r.height];
  var Els2 = class {
    els = [];
    add(el) {
      const i = this.els.indexOf(el);
      return i >= 0 ? i : this.els.push(el) - 1;
    }
  };
  function keyOf(el, testid) {
    try {
      const found = JSON.parse(stable(el, testid));
      if (found.locator) return found.locator;
    } catch {
    }
    const parts = [];
    for (let a2 = el; a2 && parts.length < 5; a2 = parentOf(a2)) {
      const t = tag(a2);
      if (t === "html" || t === "body") break;
      const id = a2.getAttribute("id");
      if (id && /^[A-Za-z][\w-]*$/.test(id)) {
        parts.unshift(`#${id}`);
        break;
      }
      const cls = norm(a2.getAttribute("class")).split(" ")[0];
      let part = cls && /^[A-Za-z_][\w-]*$/.test(cls) ? `${t}.${cls}` : t;
      const parent = a2.parentElement;
      if (parent) {
        const same = Array.from(parent.children).filter((c) => tag(c) === t);
        if (same.length > 1) part += `:nth-of-type(${same.indexOf(a2) + 1})`;
      }
      parts.unshift(part);
    }
    return `css=${parts.join(" > ")}`;
  }
  var clips = (o) => o === "hidden" || o === "clip";
  var scrolls2 = (o) => o === "auto" || o === "scroll" || o === "overlay";
  function paddingBox(el) {
    const r = el.getBoundingClientRect();
    return [r.x + el.clientLeft, r.y + el.clientTop, el.clientWidth, el.clientHeight];
  }
  function intersect(a2, b) {
    const x = Math.max(a2[0], b[0]);
    const y = Math.max(a2[1], b[1]);
    const right = Math.min(a2[0] + a2[2], b[0] + b[2]);
    const bottom = Math.min(a2[1] + a2[3], b[1] + b[3]);
    return right > x && bottom > y ? [x, y, right - x, bottom - y] : null;
  }
  var area = (b) => b[2] * b[3];
  var centre = (b) => [b[0] + b[2] / 2, b[1] + b[3] / 2];
  function clipBox(el, box) {
    for (let a2 = parentOf(el); a2 && box; a2 = parentOf(a2)) {
      const t = tag(a2);
      if (t === "html" || t === "body") break;
      const s = getComputedStyle(a2);
      if (s.overflowX === "visible" && s.overflowY === "visible") continue;
      const p = paddingBox(a2);
      const limit = [
        s.overflowX === "visible" ? -1e9 : p[0],
        s.overflowY === "visible" ? -1e9 : p[1],
        s.overflowX === "visible" ? 2e9 : p[2],
        s.overflowY === "visible" ? 2e9 : p[3]
      ];
      box = intersect(box, limit);
      if (s.position === "fixed") break;
    }
    return box;
  }
  function shownBoxes(el, style) {
    const rects = style.display === "inline" ? Array.from(el.getClientRects()) : [];
    const boxes = rects.length > 1 ? rects.map(boxOf) : [boxOf(el.getBoundingClientRect())];
    return boxes.filter((b) => b[2] >= 1 && b[3] >= 1).map((b) => clipBox(el, b)).filter((b) => !!b);
  }
  var largest = (boxes) => boxes.reduce((a2, b) => area(b) > area(a2) ? b : a2);
  var reach = (p, b) => Math.hypot(Math.max(b[0] - p[0], 0, p[0] - b[0] - b[2]), Math.max(b[1] - p[1], 0, p[1] - b[1] - b[3]));
  function visuallyHidden2(el, style) {
    const r = el.getBoundingClientRect();
    if (r.width <= 1 || r.height <= 1) return true;
    const clip = style.clip.replace(/\s+/g, "");
    if (style.position === "absolute" || style.position === "fixed") {
      const m = /^rect\(([-\d.]+)px,?([-\d.]+)px,?([-\d.]+)px,?([-\d.]+)px\)$/.exec(clip);
      if (m && (parseFloat(m[3]) - parseFloat(m[1]) <= 1 || parseFloat(m[2]) - parseFloat(m[4]) <= 1)) return true;
    }
    const path = style.clipPath.replace(/\s+/g, "");
    return /^inset\((50|100)%\)$/.test(path) || /^circle\(0(px)?/.test(path) || /^polygon\(0(px)?0(px)?,0(px)?0(px)?/.test(path);
  }
  function rendered(root) {
    const out = [];
    const clear = /* @__PURE__ */ new Map();
    const transparent = (el) => {
      if (!el) return false;
      const known = clear.get(el);
      if (known !== void 0) return known;
      const value2 = getComputedStyle(el).opacity === "0" || transparent(parentOf(el));
      clear.set(el, value2);
      return value2;
    };
    const all = root instanceof Document ? descendants(root) : [root, ...descendants(root)];
    const modal = openModal(document);
    for (const el of all) {
      const t = tag(el);
      if (SKIP.has(t) || t === "html" || t === "body") continue;
      const style = getComputedStyle(el);
      if (style.display === "none" || style.visibility !== "visible") continue;
      const r = el.getBoundingClientRect();
      if (r.width < 1 || r.height < 1) continue;
      if (transparent(el)) continue;
      const role = roleOf(el) || "generic";
      const parentStyle = parentOf(el) ? getComputedStyle(parentOf(el)) : null;
      const tabindex = parseInt(el.getAttribute("tabindex") || "", 10);
      const pointer = style.cursor === "pointer" && !(parentStyle && parentStyle.cursor === "pointer");
      const clickable = el.hasAttribute("onclick") || tabindex >= 0 || pointer;
      const inert = !!el.closest("[inert]") || outsideModal(el, modal);
      const interactive = role !== "iframe" && !inert && (INTERACTIVE.has(role) || clickable);
      out.push({ el, style, role, interactive });
    }
    return out;
  }
  function hitAt(doc, x, y) {
    let hit = doc.elementFromPoint(x, y);
    while (hit && hit.shadowRoot) {
      const inner = hit.shadowRoot.elementFromPoint(x, y);
      if (!inner || inner === hit) break;
      hit = inner;
    }
    return hit;
  }
  function ownText(el) {
    const nodes = [];
    for (let n = el.firstChild; n; n = n.nextSibling) {
      if (n.nodeType === Node.TEXT_NODE && norm(n.data)) nodes.push(n);
    }
    return nodes;
  }
  function textBox(el) {
    const nodes = ownText(el);
    if (!nodes.length) return null;
    let left = Infinity;
    let top = Infinity;
    let right = -Infinity;
    let bottom = -Infinity;
    const range = document.createRange();
    for (const node of nodes) {
      range.selectNodeContents(node);
      for (const r of Array.from(range.getClientRects())) {
        if (r.width < 1 || r.height < 1) continue;
        left = Math.min(left, r.left);
        top = Math.min(top, r.top);
        right = Math.max(right, r.right);
        bottom = Math.max(bottom, r.bottom);
      }
    }
    range.detach();
    return right > left && bottom > top ? [left, top, right - left, bottom - top] : null;
  }
  function colour2(s) {
    const m = /^rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)(?:\s*[,/]\s*([\d.]+%?))?\s*\)$/.exec(s.trim());
    if (!m) return null;
    let a2 = m[4] === void 0 ? 1 : parseFloat(m[4]);
    if (m[4] && m[4].endsWith("%")) a2 /= 100;
    return [parseFloat(m[1]), parseFloat(m[2]), parseFloat(m[3]), a2];
  }
  function opacity(el) {
    let o = 1;
    for (let a2 = el; a2; a2 = parentOf(a2)) o *= parseFloat(getComputedStyle(a2).opacity) || 0;
    return o;
  }
  var NATIVE = /* @__PURE__ */ new Set(["button", "input", "select", "textarea"]);
  var Native = class {
    host = null;
    root = null;
    sizes = /* @__PURE__ */ new Map();
    /** Whether `el`'s small sides (under `min`) are the size an unstyled
     * control of its kind has: the browser sized them (WCAG 2.5.8's
     * exception), as it does the options of a native select. */
    unstyled(el, box, min) {
      const t = tag(el);
      if (t === "option") return !!el.closest("select");
      if (!NATIVE.has(t)) return false;
      const ref = this.size(el);
      if (!ref) return false;
      const width = box[2] >= min || Math.abs(box[2] - ref.width) <= 1;
      const height = box[3] >= min || Math.abs(box[3] - ref.height) <= 1;
      return width && height;
    }
    size(el) {
      const t = tag(el);
      const type = t === "input" ? el.type : "";
      const multiple = t === "select" ? `${el.multiple}:${el.size}` : "";
      const text2 = t === "button" ? norm(el.textContent) : "";
      const key = `${t}:${type}:${multiple}:${text2}`;
      const known = this.sizes.get(key);
      if (known) return known;
      if (!this.root) {
        this.host = document.createElement("div");
        this.host.style.cssText = "position: absolute; left: -10000px; top: 0; visibility: hidden";
        this.root = this.host.attachShadow({ mode: "closed" });
        (document.body || document.documentElement).append(this.host);
      }
      const copy = document.createElement(t);
      if (type) copy.setAttribute("type", type);
      if (text2) copy.textContent = text2;
      if (t === "select") {
        const select = el;
        copy.multiple = select.multiple;
        copy.size = select.size;
        for (const o of Array.from(select.options)) copy.append(new Option(o.text));
      }
      this.root.append(copy);
      const size2 = copy.getBoundingClientRect();
      this.sizes.set(key, size2);
      return size2;
    }
    remove() {
      this.host?.remove();
      this.host = null;
      this.root = null;
    }
  };
  var measured = [];
  function layoutCheck(root, options) {
    const kinds = new Set(options.kinds);
    const els = new Els2();
    const findings = [];
    const seen = rendered(root || document);
    const view = options.view || [0, 0, innerWidth, innerHeight];
    const inView = (b) => {
      const [x, y] = centre(b);
      return x >= view[0] && y >= view[1] && x < view[0] + view[2] && y < view[1] + view[3];
    };
    const targets = seen.filter((s) => s.interactive && !visuallyHidden2(s.el, s.style));
    const shown3 = /* @__PURE__ */ new Map();
    const shownOf = (s) => {
      if (!shown3.has(s.el)) shown3.set(s.el, shownBoxes(s.el, s.style));
      return shown3.get(s.el);
    };
    if (kinds.has("covered")) {
      for (const t of targets) {
        const box = shownOf(t).find(inView);
        if (!box) continue;
        const [x, y] = centre(box);
        const hit = hitAt(t.el.ownerDocument, x, y);
        if (!hit || contains(t.el, hit) || contains(hit, t.el)) continue;
        const labels = t.el.labels;
        if (labels && Array.from(labels).some((l) => contains(l, hit))) continue;
        const found = covering(t.el, hit);
        const top = found.els[0];
        if (!top) continue;
        const others = [els.add(top)];
        if (found.els[1]) others.push(els.add(found.els[1]));
        findings.push({
          kind: "covered",
          el: els.add(t.el),
          box: boxOf(t.el.getBoundingClientRect()),
          others,
          details: { at: [px2(x), px2(y)], dismiss: !!found.els[1] }
        });
      }
    }
    if (kinds.has("overlap")) {
      const boxes = targets.map((t) => {
        const parts = shownOf(t);
        const left = Math.min(...parts.map((b) => b[0]));
        const right = Math.max(...parts.map((b) => b[0] + b[2]));
        const size2 = parts.reduce((sum, b) => sum + area(b), 0);
        return { el: t.el, parts, left, right, size: size2 };
      }).filter((b) => b.parts.length).sort((a2, b) => a2.left - b.left);
      for (let i = 0; i < boxes.length; i++) {
        const a2 = boxes[i];
        for (let j = i + 1; j < boxes.length && boxes[j].left < a2.right; j++) {
          const b = boxes[j];
          let both = 0;
          let width = 0;
          let height = 0;
          for (const pa of a2.parts) {
            for (const pb of b.parts) {
              const part = intersect(pa, pb);
              if (!part) continue;
              both += area(part);
              width = Math.max(width, part[2]);
              height = Math.max(height, part[3]);
            }
          }
          if (both <= 4 || both < 0.1 * Math.min(a2.size, b.size)) continue;
          if (contains(a2.el, b.el) || contains(b.el, a2.el)) continue;
          const [first, second] = a2.el.compareDocumentPosition(b.el) & Node.DOCUMENT_POSITION_FOLLOWING ? [a2, b] : [b, a2];
          findings.push({
            kind: "overlap",
            el: els.add(first.el),
            box: boxOf(first.el.getBoundingClientRect()),
            others: [els.add(second.el)],
            details: { width: px2(width), height: px2(height) }
          });
        }
      }
    }
    if (kinds.has("clipped") || kinds.has("truncated")) {
      for (const s of seen) {
        if (visuallyHidden2(s.el, s.style)) continue;
        const text2 = textBox(s.el);
        if (!text2) continue;
        for (let a2 = s.el; a2; a2 = parentOf(a2)) {
          const t = tag(a2);
          if (t === "html" || t === "body") break;
          const style = a2 === s.el ? s.style : getComputedStyle(a2);
          const cutX = clips(style.overflowX);
          const cutY = clips(style.overflowY);
          if (!cutX && !cutY) {
            if (style.position === "fixed") break;
            continue;
          }
          const p = paddingBox(a2);
          const x = cutX && (text2[0] < p[0] - 1 || text2[0] + text2[2] > p[0] + p[2] + 1);
          const y = cutY && (text2[1] < p[1] - 1 || text2[1] + text2[3] > p[1] + p[3] + 1);
          if (!x && !y) {
            if (scrolls2(style.overflowX) || scrolls2(style.overflowY)) break;
            continue;
          }
          const both = intersect(text2, p);
          if (!both) break;
          const axis = x ? "x" : "y";
          const full = axis === "x" ? text2[2] : text2[3];
          const part = axis === "x" ? both[2] : both[3];
          const clamp = (st) => (st.webkitLineClamp || "none") !== "none";
          const how = axis === "x" && style.textOverflow === "ellipsis" ? "ellipsis" : axis === "y" && (clamp(style) || clamp(s.style)) ? "line-clamp" : null;
          const kind = how ? "truncated" : "clipped";
          if (!kinds.has(kind)) break;
          const details = {
            axis,
            shown: px2(part),
            full: px2(full),
            overflow: axis === "x" ? style.overflowX : style.overflowY,
            own: a2 === s.el
          };
          if (how) {
            details.how = how;
            const whole = norm(s.el.textContent);
            details.text = whole.length > 200 ? `${whole.slice(0, 199)}…` : whole;
          }
          findings.push({
            kind,
            el: els.add(s.el),
            box: boxOf(s.el.getBoundingClientRect()),
            others: a2 === s.el ? [] : [els.add(a2)],
            details
          });
          break;
        }
      }
    }
    const doc = document.documentElement;
    const htmlStyle = getComputedStyle(doc);
    const bodyStyle = document.body ? getComputedStyle(document.body) : null;
    const viewportOverflow = (axis) => htmlStyle[axis] !== "visible" ? htmlStyle[axis] : bodyStyle ? bodyStyle[axis] : "visible";
    const sideways = !clips(viewportOverflow("overflowX"));
    const downwards = !clips(viewportOverflow("overflowY"));
    if (kinds.has("overflow-x") && options.main && sideways && doc.scrollWidth > doc.clientWidth + 1) {
      const past = (s) => {
        const r = s.el.getBoundingClientRect();
        return r.right + scrollX > doc.clientWidth + 1;
      };
      const within = (el) => {
        for (let a2 = parentOf(el); a2; a2 = parentOf(a2)) {
          const t = tag(a2);
          if (t === "html" || t === "body") return false;
          const st = getComputedStyle(a2);
          if (st.overflowX !== "visible" || st.position === "fixed") return true;
        }
        return false;
      };
      const wide = seen.filter((s) => s.style.position !== "fixed" && past(s) && !within(s.el));
      const outer = wide.filter((s) => !wide.some((o) => o !== s && contains(o.el, s.el)));
      const named2 = outer.slice(0, 5);
      if (named2.length) {
        findings.push({
          kind: "overflow-x",
          el: els.add(named2[0].el),
          box: boxOf(named2[0].el.getBoundingClientRect()),
          others: named2.map((s) => els.add(s.el)),
          details: {
            width: doc.scrollWidth,
            viewport: doc.clientWidth,
            ends: named2.map((s) => px2(s.el.getBoundingClientRect().right + scrollX))
          }
        });
      }
    }
    if (kinds.has("offscreen")) {
      const width = sideways ? doc.scrollWidth : doc.clientWidth;
      const height = downwards ? doc.scrollHeight : doc.clientHeight;
      for (const t of targets) {
        const r = t.el.getBoundingClientRect();
        const fixed = t.style.position === "fixed";
        const x = r.left + (fixed ? 0 : scrollX);
        const y = r.top + (fixed ? 0 : scrollY);
        const [w, h] = fixed ? [doc.clientWidth, doc.clientHeight] : [width, height];
        if (!(x + r.width <= 0 || y + r.height <= 0 || x >= w || y >= h)) continue;
        if (tag(t.el) === "a" && (t.el.getAttribute("href") || "").startsWith("#")) continue;
        if (!shownOf(t).length) continue;
        findings.push({
          kind: "offscreen",
          el: els.add(t.el),
          box: boxOf(r),
          details: { x: px2(x), y: px2(y), width: px2(w), height: px2(h) }
        });
      }
    }
    if (kinds.has("small-target")) {
      const min = options.minTarget;
      const boxes = targets.map((t) => ({ t, parts: shownOf(t) })).filter((b) => b.parts.length).map((b) => ({ ...b, box: largest(b.parts) }));
      const small = (b) => b[2] < min - 0.01 || b[3] < min - 0.01;
      const native = new Native();
      try {
        for (const { t, box, parts } of boxes) {
          if (!small(box)) continue;
          if (t.role === "link" && t.style.display === "inline") {
            let block = parentOf(t.el);
            while (block && getComputedStyle(block).display.startsWith("inline")) block = parentOf(block);
            if (block && norm(block.textContent).length > norm(t.el.textContent).length) continue;
          }
          if (native.unstyled(t.el, box, min)) continue;
          const c = centre(box);
          const crowded = boxes.some((o) => {
            if (o.t === t || contains(o.t.el, t.el) || contains(t.el, o.t.el)) return false;
            if (o.parts.some((b) => reach(c, b) < min / 2)) return true;
            return small(o.box) && Math.hypot(c[0] - centre(o.box)[0], c[1] - centre(o.box)[1]) < min;
          });
          if (!crowded) continue;
          findings.push({
            kind: "small-target",
            el: els.add(t.el),
            box: parts.length > 1 ? box : boxOf(t.el.getBoundingClientRect()),
            details: { width: px2(box[2]), height: px2(box[3]), min }
          });
        }
      } finally {
        native.remove();
      }
    }
    const texts = [];
    let skipped = 0;
    measured = [];
    if (kinds.has("contrast")) {
      for (const s of seen) {
        if (tag(s.el) === "input" || tag(s.el) === "textarea" || tag(s.el) === "select") continue;
        if (visuallyHidden2(s.el, s.style)) continue;
        if (s.el.closest(':disabled, [aria-disabled="true"]')) continue;
        const own = textBox(s.el);
        if (!own) continue;
        const shownText = clipBox(s.el, own);
        const box = shownText && intersect(shownText, view);
        if (!box || box[2] < 1 || box[3] < 1) continue;
        const c = colour2(s.style.webkitTextFillColor || s.style.color);
        if (!c) continue;
        const a2 = c[3] * opacity(s.el);
        if (a2 <= 0.01) continue;
        if (texts.length >= options.texts) {
          skipped++;
          continue;
        }
        const size2 = parseFloat(s.style.fontSize) || 16;
        const weight = parseInt(s.style.fontWeight, 10) || 400;
        texts.push({ box, colour: [c[0], c[1], c[2], a2], large: size2 >= 24 || size2 >= 18.66 && weight >= 700 });
        measured.push(s.el);
      }
    }
    return answer(els, options.testid, {
      findings,
      texts,
      skipped,
      scroll: [scrollX, scrollY]
    });
  }
  function answer(els, testid, json) {
    for (const [node] of stretched) node.data = originals.get(node) ?? node.data;
    try {
      const names = els.els.map(named);
      const keys = els.els.map((el) => keyOf(el, testid));
      return { json: JSON.stringify({ ...json, names, keys }), els: els.els };
    } finally {
      for (const [node, data] of stretched) node.data = data;
    }
  }
  function textNames(indexes, testid) {
    const els = new Els2();
    const at = indexes.map((i) => measured[i] ? els.add(measured[i]) : -1);
    return answer(els, testid, { at });
  }
  var hidden2 = null;
  var HIDE_ROOTS = [];
  async function hideText() {
    showText();
    const sheet = new CSSStyleSheet();
    const all = ":is(*, #_#_#_#_#_#_)";
    sheet.replaceSync(`${all}, ${all}::before, ${all}::after, ${all}::first-letter, ${all}::first-line,
    ${all}::placeholder, ${all}::marker {
    color: transparent !important; -webkit-text-fill-color: transparent !important;
    -webkit-text-stroke-color: transparent !important; text-shadow: none !important;
    text-decoration-color: transparent !important; caret-color: transparent !important;
    transition: none !important; }`);
    hidden2 = sheet;
    const roots = [document];
    for (const el of descendants(document)) if (el.shadowRoot) roots.push(el.shadowRoot);
    for (const root of roots) {
      root.adoptedStyleSheets = [...root.adoptedStyleSheets, sheet];
      HIDE_ROOTS.push(root);
    }
    await frame();
    await frame();
  }
  function showText() {
    const sheet = hidden2;
    hidden2 = null;
    for (const root of HIDE_ROOTS.splice(0)) {
      if (sheet) root.adoptedStyleSheets = root.adoptedStyleSheets.filter((s) => s !== sheet);
    }
  }
  var stretched = [];
  var originals = /* @__PURE__ */ new WeakMap();
  async function stretchText(percent) {
    unstretchText();
    const roots = [document.documentElement];
    for (const el of descendants(document)) if (el.shadowRoot) roots.push(el.shadowRoot);
    const nodes = [];
    for (const root of roots) {
      const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
      for (let n = walker.nextNode(); n; n = walker.nextNode()) nodes.push(n);
    }
    for (const node of nodes) {
      const parent = node.parentElement;
      if (!parent || SKIP.has(tag(parent)) || tag(parent) === "textarea" || tag(parent) === "option") continue;
      const data = node.data;
      const words = data.trim().split(/\s+/).filter(Boolean);
      if (!words.length) continue;
      const want = Math.ceil(data.trim().length * (1 + percent / 100));
      let text2 = data.trim();
      for (let i = 0; text2.length < want; i++) text2 += ` ${words[i % words.length]}`;
      const lead = /^\s*/.exec(data)?.[0] || "";
      const trail = /\s*$/.exec(data)?.[0] || "";
      originals.set(node, data);
      node.data = lead + text2 + trail;
      stretched.push([node, node.data]);
    }
    await frame();
    return stretched.length;
  }
  function unstretchText() {
    for (const [node, data] of stretched) {
      const original = originals.get(node);
      if (original !== void 0 && node.data === data) node.data = original;
    }
    stretched = [];
  }
  async function scrollPage(x, y) {
    scrollTo({ left: x, top: y, behavior: "instant" });
    await frame();
    return [scrollX, scrollY];
  }
  function pageExtent() {
    const doc = document.scrollingElement || document.documentElement;
    return [doc.scrollHeight, innerHeight, scrollX, scrollY];
  }
  function contentBox(el) {
    const r = el.getBoundingClientRect();
    const s = getComputedStyle(el);
    const n = (v) => parseFloat(v) || 0;
    const x = r.x + n(s.borderLeftWidth) + n(s.paddingLeft);
    const y = r.y + n(s.borderTopWidth) + n(s.paddingTop);
    const w = r.width - n(s.borderLeftWidth) - n(s.borderRightWidth) - n(s.paddingLeft) - n(s.paddingRight);
    const h = r.height - n(s.borderTopWidth) - n(s.borderBottomWidth) - n(s.paddingTop) - n(s.paddingBottom);
    return [x, y, Math.max(w, 0), Math.max(h, 0)];
  }

  // src/forms.ts
  var UNITS = /* @__PURE__ */ new Set(["form", "search"]);
  var WIDGETS = /* @__PURE__ */ new Set([
    "combobox",
    "listbox",
    "checkbox",
    "switch",
    "radiogroup",
    "textbox",
    "searchbox",
    "slider",
    "spinbutton",
    "radio"
  ]);
  var WRAPS_INPUT = /* @__PURE__ */ new Set(["combobox", "textbox", "searchbox"]);
  var OPTIONS = 10;
  var VALUE = 80;
  var cut5 = (s, n) => s.length > n ? s.slice(0, n - 1) + "…" : s;
  var isUnit = (el) => tag(el) === "form" || UNITS.has(roleOf(el) || "");
  function unitAbove(el) {
    for (let a2 = parentOf(el); a2; a2 = parentOf(a2)) if (isUnit(a2)) return a2;
    return null;
  }
  function ownerOf(el) {
    const owner2 = el.form;
    if (owner2 && !contains(owner2, el)) return owner2;
    return unitAbove(el);
  }
  function nativeKind(el) {
    const t = tag(el);
    if (el instanceof HTMLInputElement) {
      const type = el.type.toLowerCase();
      if (type === "hidden") return null;
      return type === "image" ? "submit" : type;
    }
    if (t === "select") return "select";
    if (t === "textarea") return "textarea";
    if (el instanceof HTMLButtonElement) return el.type;
    const html2 = el;
    if (html2.isContentEditable && !(el.parentElement && el.parentElement.isContentEditable)) return "contenteditable";
    return null;
  }
  function shown2(el) {
    if (!hiddenForAria(el) && visible(el)) return true;
    const labels = el.labels;
    return !!labels && Array.from(labels).some((l) => !hiddenForAria(l) && visible(l));
  }
  var shownValue = (s) => cut5(norm(s), VALUE);
  function problem(el) {
    const native = el;
    if (typeof native.willValidate === "boolean" && native.willValidate && !native.validity.valid) {
      const v = native.validity;
      const other = v.typeMismatch || v.patternMismatch || v.tooLong || v.tooShort || v.rangeUnderflow || v.rangeOverflow || v.stepMismatch || v.badInput || v.customError;
      if (other) return { invalid: true, message: cut5(norm(native.validationMessage), 120) || void 0 };
    }
    const aria = attr(el, "aria-invalid");
    if (!aria || aria === "false") return {};
    for (const name of ["aria-errormessage", "aria-describedby"]) {
      const ids = attr(el, name);
      if (!ids) continue;
      const text2 = norm(byIds(el, ids).map((e) => e.innerText || e.textContent || "").join(" "));
      if (text2) return { invalid: true, message: cut5(text2, 120) };
    }
    return { invalid: true };
  }
  function customOptions(list) {
    const out = [];
    const owned = attr(list, "aria-owns");
    const els = [...descendants(list), ...owned ? byIds(list, owned) : []];
    for (const o of els) {
      if (roleOf(o) !== "option" || hiddenForAria(o)) continue;
      out.push([o, nameOf(o, "option")]);
    }
    return out;
  }
  function listsOf(el) {
    const ids = [attr(el, "aria-controls"), attr(el, "aria-owns")].filter(Boolean).join(" ");
    const lists = ids ? byIds(el, ids) : [];
    const active = attr(el, "aria-activedescendant");
    if (active) {
      for (const o of byIds(el, active)) {
        for (let a2 = parentOf(o); a2; a2 = parentOf(a2)) {
          if (roleOf(a2) === "listbox") {
            lists.push(a2);
            break;
          }
        }
      }
    }
    const list = el.list;
    if (list) lists.push(list);
    return Array.from(new Set(lists));
  }
  function listed(field2, names) {
    if (!names.length) return;
    field2.options = names.slice(0, OPTIONS).map((n) => cut5(n, 40));
    if (names.length > OPTIONS) field2.more = names.length - OPTIONS;
  }
  function fill(field2, el, role) {
    const input = el;
    const aria = (name) => attr(el, name) === "true";
    if (input.required && !(el instanceof HTMLButtonElement) || aria("aria-required")) field2.required = true;
    if (el.matches(":disabled") || el.closest('[aria-disabled="true"]')) field2.disabled = true;
    if (input.readOnly && (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) || aria("aria-readonly")) {
      field2.readonly = true;
    }
    if (attr(el, "name")) field2.html = attr(el, "name");
    if (el.id) field2.id = el.id;
    if (attr(el, "placeholder")) field2.placeholder = norm(attr(el, "placeholder"));
    Object.assign(field2, problem(el));
    const kind = field2.kind;
    if (field2.button) {
      if (kind === "submit") field2.submit = true;
      return;
    }
    if (el instanceof HTMLInputElement && (kind === "checkbox" || kind === "radio")) {
      field2.checked = el.indeterminate && kind === "checkbox" ? "mixed" : el.checked;
      if (!el.checked) field2.empty = true;
      return;
    }
    if (el instanceof HTMLInputElement && kind === "file") {
      field2.files = Array.from(el.files || []).map((f) => f.name);
      if (el.accept) field2.accept = el.accept;
      if (el.multiple) field2.multiple = true;
      if (!field2.files.length) field2.empty = true;
      return;
    }
    if (el instanceof HTMLSelectElement) {
      const options = Array.from(el.options);
      listed(field2, options.map((o) => norm(o.label)));
      const selected = options.filter((o) => o.selected).map((o) => norm(o.label));
      if (el.multiple) {
        field2.multiple = true;
        field2.selected = selected;
      } else {
        field2.value = shownValue(selected[0] || "");
      }
      if (el.value === "") field2.empty = true;
      return;
    }
    if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) {
      if (input.type === "password") {
        field2.secret = true;
      } else {
        field2.value = shownValue(input.value);
      }
      if (input.value === "") field2.empty = true;
      if (role === "combobox") listed(field2, listsOf(el).flatMap((l) => l instanceof HTMLDataListElement ? Array.from(l.options).map((o) => norm(o.label || o.value)) : customOptions(l).map(([, n]) => n)));
      return;
    }
    if (role === "checkbox" || role === "switch" || role === "radio") {
      const c = attr(el, "aria-checked");
      field2.checked = c === "mixed" ? "mixed" : c === "true";
      if (c !== "true") field2.empty = true;
      return;
    }
    if (role === "listbox") {
      const options = customOptions(el);
      listed(field2, options.map(([, n]) => n));
      field2.selected = options.filter(([o]) => attr(o, "aria-selected") === "true").map(([, n]) => n);
      if (aria("aria-multiselectable")) field2.multiple = true;
      else if (field2.selected.length) field2.value = shownValue(field2.selected[0]);
      if (!field2.selected.length) field2.empty = true;
      return;
    }
    if (role === "slider" || role === "spinbutton") {
      field2.value = shownValue(attr(el, "aria-valuetext") || attr(el, "aria-valuenow") || "");
      return;
    }
    if (role === "combobox") listed(field2, listsOf(el).flatMap((l) => customOptions(l).map(([, n]) => n)));
    const text2 = el.innerText || "";
    field2.value = shownValue(text2);
    if (!norm(text2)) field2.empty = true;
  }
  function formsList(root) {
    const doc = root ? root.ownerDocument : document;
    const names = [];
    const els = [];
    const index = /* @__PURE__ */ new Map();
    const ref = (el) => {
      let i = index.get(el);
      if (i === void 0) {
        i = els.length;
        index.set(el, i);
        els.push(el);
        names.push(named(el));
      }
      return i;
    };
    const all = descendants(doc);
    let wanted = null;
    let scope = null;
    if (root) {
      const up = isUnit(root) ? root : unitAbove(root);
      if (up) wanted = [up];
      else {
        const inside = all.filter((e) => contains(root, e) && e !== root && isUnit(e));
        if (inside.length) wanted = inside;
        else scope = root;
      }
    }
    const units = /* @__PURE__ */ new Map();
    const unitOf = (owner2) => {
      if (scope) owner2 = scope;
      else if (wanted && (!owner2 || !wanted.includes(owner2))) return null;
      let unit = units.get(owner2);
      if (!unit) {
        unit = { fields: [] };
        if (owner2) unit.el = ref(owner2);
        else unit.outside = true;
        if (scope) unit.scope = true;
        if (owner2 instanceof HTMLFormElement && (owner2.hasAttribute("action") || owner2.hasAttribute("method"))) {
          unit.method = owner2.method.toUpperCase();
          const action = new URL(owner2.action, doc.baseURI);
          const here = new URL(doc.URL);
          unit.action = action.origin === here.origin ? action.pathname + action.search : action.href;
        }
        units.set(owner2, unit);
      }
      return unit;
    };
    for (const e of all) {
      if (!isUnit(e)) continue;
      if (scope ? false : wanted ? wanted.includes(e) : true) unitOf(e);
    }
    if (scope) unitOf(scope);
    const groups = /* @__PURE__ */ new Map();
    const inputRole = /* @__PURE__ */ new Map();
    const claimed = [];
    const popups = new Set(all.filter((e) => roleOf(e) === "combobox").flatMap(listsOf));
    for (const el of all) {
      if (scope && !contains(scope, el)) continue;
      while (claimed.length && !contains(claimed[claimed.length - 1], el)) claimed.pop();
      if (claimed.length) continue;
      let role = inputRole.get(el) || roleOf(el) || "";
      const native = el instanceof HTMLButtonElement && WIDGETS.has(role) ? null : nativeKind(el);
      if (!native && !WIDGETS.has(role)) continue;
      if (popups.has(el)) {
        claimed.push(el);
        continue;
      }
      if (!native && WRAPS_INPUT.has(role) && el.querySelector("input:not([type=hidden]), textarea")) {
        const inner = el.querySelector("input:not([type=hidden]), textarea");
        inputRole.set(inner, role);
        continue;
      }
      const unit = unitOf(ownerOf(el));
      if (!unit) continue;
      const isRadio = native === "radio" || !native && role === "radio";
      if (role === "radiogroup" && !native) {
        if (!shown2(el)) continue;
        const group = { el: ref(el), kind: "radiogroup", role, custom: true, radios: [] };
        fill(group, el, role);
        delete group.empty;
        groups.set(el, group);
        unit.fields.push(group);
        continue;
      }
      if (isRadio) {
        if (!shown2(el)) continue;
        let around = null;
        for (let a2 = parentOf(el); a2 && a2 !== doc.body; a2 = parentOf(a2)) {
          if (roleOf(a2) === "radiogroup") {
            around = a2;
            break;
          }
        }
        let key = around || el;
        if (!around && native && attr(el, "name")) {
          const owner2 = ownerOf(el);
          key = `${owner2 ? ref(owner2) : ""}|${attr(el, "name")}`;
        }
        let group = groups.get(key);
        if (!group) {
          const fieldset = el.closest("fieldset");
          group = { el: ref(fieldset && !around ? fieldset : el), kind: "radiogroup", role: "radiogroup", radios: [] };
          if (native && attr(el, "name")) group.html = attr(el, "name");
          groups.set(key, group);
          unit.fields.push(group);
        }
        const input = el;
        const checked2 = native ? input.checked : attr(el, "aria-checked") === "true";
        const radio = { el: ref(el), name: nameOf(el, "radio"), checked: checked2 };
        if (native) radio.value = input.value;
        if (el.matches(":disabled") || el.closest('[aria-disabled="true"]')) radio.disabled = true;
        if (native && input.required || attr(el, "aria-required") === "true") group.required = true;
        group.radios.push(radio);
        if (!group.invalid) Object.assign(group, problem(el));
        continue;
      }
      if (!shown2(el)) continue;
      const button = !!native && (el instanceof HTMLButtonElement || ["submit", "reset", "button"].includes(native));
      if (button && unit.outside) continue;
      if (native === "file") role = "file";
      const field2 = { el: ref(el), kind: native || role, role };
      if (!native) field2.custom = true;
      if (button) field2.button = true;
      fill(field2, el, role);
      unit.fields.push(field2);
      claimed.push(el);
    }
    for (const group of groups.values()) {
      if (group.radios && !group.radios.some((r) => r.checked)) group.empty = true;
      if (group.radios) group.value = group.radios.find((r) => r.checked)?.name;
    }
    const out = Array.from(units.values()).filter((u) => !u.outside || u.fields.length);
    out.sort((a2, b) => Number(!!a2.outside) - Number(!!b.outside));
    return { json: JSON.stringify({ units: out, names }), els };
  }
  function pickOption(el, want) {
    let lists = roleOf(el) === "listbox" ? [el] : listsOf(el).filter((l) => !(l instanceof HTMLDataListElement));
    if (!lists.length && roleOf(el) !== "listbox") {
      lists = descendants(el.ownerDocument).filter((l) => roleOf(l) === "listbox" && !hiddenForAria(l) && visible(l) && tag(l) !== "select");
    }
    const options = lists.flatMap(customOptions).filter(([o]) => visible(o));
    const w = norm(want).toLowerCase();
    const by = (test2) => options.filter(([, n]) => test2(norm(n).toLowerCase()));
    let found = by((n) => n === w);
    if (found.length !== 1) {
      const starts = by((n) => n.startsWith(w));
      found = found.length > 1 ? found : starts.length === 1 ? starts : by((n) => n.includes(w));
    }
    const pick = found.length === 1 || found.length > 1 && norm(found[0][1]).toLowerCase() === w ? found[0] : null;
    const out = {
      found: !!pick,
      count: options.length,
      options: options.slice(0, OPTIONS).map(([, n]) => cut5(n, 40)),
      selected: pick ? attr(pick[0], "aria-selected") === "true" : false,
      name: pick ? named(pick[0]) : null
    };
    return { json: JSON.stringify(out), els: pick ? [pick[0]] : [] };
  }

  // src/compare.ts
  function elementsAt(points) {
    const boxes = [];
    for (const el of descendants(document)) {
      const t = tag(el);
      if (t === "html" || t === "head" || t === "script" || t === "style") continue;
      const r = el.getBoundingClientRect();
      if (r.width <= 0 || r.height <= 0) continue;
      if (!points.some(([x, y]) => x >= r.left && x < r.right && y >= r.top && y < r.bottom)) continue;
      if (!visible(el)) continue;
      boxes.push([el, r]);
    }
    const els = [];
    const at = points.map(([x, y]) => {
      let best = null;
      let area2 = Infinity;
      for (const [el, r] of boxes) {
        if (x < r.left || x >= r.right || y < r.top || y >= r.bottom) continue;
        if (r.width * r.height <= area2) {
          best = el;
          area2 = r.width * r.height;
        }
      }
      if (!best) return -1;
      const i = els.indexOf(best);
      return i >= 0 ? i : els.push(best) - 1;
    });
    const names = els.map(named);
    return { json: JSON.stringify({ names, at }), els };
  }

  // src/explore.ts
  var CONTROL_ROLES = /* @__PURE__ */ new Set(["button", "tab", "menuitem", "menuitemcheckbox", "menuitemradio", "switch"]);
  function submits(el) {
    if (el instanceof HTMLButtonElement) return el.type === "submit" && !!el.form;
    if (el instanceof HTMLInputElement) return (el.type === "submit" || el.type === "image") && !!el.form;
    return false;
  }
  function kindOf(el) {
    const t = tag(el);
    if ((t === "a" || t === "area") && el.hasAttribute("href") && !(el instanceof SVGElement)) return "link";
    if (el instanceof HTMLButtonElement || el instanceof HTMLInputElement) {
      if (el.disabled) return null;
      if (el instanceof HTMLInputElement && !["button", "submit", "image", "reset"].includes(el.type)) return null;
      if (el.type === "reset") return "reset";
      return submits(el) ? "submit" : "button";
    }
    if (t === "summary") return "button";
    const role = el.getAttribute("role") ? roleOf(el) : null;
    if (el.getAttribute("aria-disabled") === "true") return null;
    if (role && CONTROL_ROLES.has(role) || el.hasAttribute("aria-expanded") || el.hasAttribute("aria-haspopup")) {
      return "button";
    }
    return null;
  }
  function exploreCandidates(testid, buttons) {
    const out = [];
    for (const el of descendants(document)) {
      const kind = kindOf(el);
      if (!kind) continue;
      if (!buttons && kind === "button") continue;
      if (!visible(el) || hiddenForAria(el) || el.closest("[inert]")) continue;
      const described = describe(el);
      const found = { kind, role: described.role, name: described.name, tag: described.tag };
      if (kind === "link") {
        const a2 = el;
        found.href = a2.href;
        if (a2.target) found.target = a2.target;
        if (a2.hasAttribute("download")) found.download = true;
      }
      const located = JSON.parse(stable(el, testid));
      if (located.locator) found.locator = located.locator;
      else found.reason = located.reason;
      out.push(found);
    }
    return JSON.stringify(out);
  }

  // src/fingerprint.ts
  var LONGEST2 = 80;
  var CLASSES = 20;
  var MOST = 400;
  var CONTAINERS = /* @__PURE__ */ new Set([
    "dialog",
    "alertdialog",
    "form",
    "listitem",
    "row",
    "region",
    "banner",
    "main",
    "navigation",
    "contentinfo",
    "complementary",
    "search"
  ]);
  var FAMILIES = [["button", "link"], ["textbox", "searchbox", "combobox"]];
  var cut6 = (s, n) => s.length > n ? s.slice(0, n) : s;
  function containerOf(el) {
    for (let a2 = parentOf(el); a2; a2 = parentOf(a2)) {
      const role = roleOf(a2);
      if (!role || !CONTAINERS.has(role)) continue;
      const name = cut6(role === "row" || role === "listitem" ? norm(textOf(a2, null, 0)) : nameOf(a2, role), 40);
      return [a2, name ? { role, name } : { role }];
    }
    return null;
  }
  function features(el, testid, texts) {
    const out = { tag: tag(el) };
    const role = roleOf(el);
    if (role && role !== "generic" && role !== "none") {
      out.role = role;
      const name = cut6(norm(nameOf(el, role)), LONGEST2);
      if (name) out.name = name;
    }
    const type = el.getAttribute("type");
    if (type) out.type = type.toLowerCase();
    const id = el.getAttribute(testid);
    if (id) out.testid = id;
    if (el.id) out.id = el.id;
    const label = labelsOf(el).map(norm).find((l) => l);
    if (label) out.label = cut6(label, LONGEST2);
    const placeholder = norm(el.getAttribute("placeholder"));
    if (placeholder) out.placeholder = cut6(placeholder, LONGEST2);
    const text2 = cut6(norm(texts.of(el)), LONGEST2);
    if (text2) out.text = text2;
    const classes = Array.from(el.classList).slice(0, CLASSES);
    if (classes.length) out.classes = classes;
    const found = containerOf(el);
    if (found) out.container = found[1];
    if (out.role) {
      const scope = found ? found[0] : el.ownerDocument;
      const index = descendants(scope).filter((e) => roleOf(e) === out.role).indexOf(el);
      if (index >= 0) out.index = index;
    }
    return out;
  }
  function fingerprint(el, testid) {
    const attribute = testid || "data-testid";
    const out = features(el, attribute, new Texts());
    out.page = el.ownerDocument.location ? el.ownerDocument.location.pathname : "";
    const found = JSON.parse(stable(el, attribute, true, true));
    const candidates2 = (found.candidates || []).filter((c) => c.count === 1).map((c) => c.locator).slice(0, 5);
    if (candidates2.length) out.candidates = candidates2;
    return JSON.stringify(out);
  }
  var read = [];
  function akin(a2, b) {
    if (!a2 || !b) return false;
    return a2 === b || FAMILIES.some((f) => f.includes(a2) && f.includes(b));
  }
  function healCandidates(fp, testid, steps) {
    const attribute = testid || "data-testid";
    const found = steps ? query(steps, null).length : 0;
    const texts = new Texts();
    read = [];
    const elements = [];
    for (const el of descendants(document)) {
      if (read.length >= MOST) break;
      const could = akin(roleOf(el), fp.role) || tag(el) === fp.tag || !!fp.testid && el.getAttribute(attribute) === fp.testid || !!fp.id && el.id === fp.id;
      if (!could || !visible(el)) continue;
      elements.push({ i: read.length, ...features(el, attribute, texts) });
      read.push(el);
    }
    return JSON.stringify({ found, elements });
  }
  function healStable(i, testid) {
    const el = read[i];
    if (!el || !el.isConnected) return JSON.stringify({ reason: "it is gone" });
    return stable(el, testid || "data-testid", false, true);
  }

  // src/index.ts
  var api = {
    version: 1,
    query,
    one,
    describe,
    walk,
    visible,
    contains,
    text,
    tidy,
    html,
    value,
    markdown,
    checkActionable,
    covering,
    scrollState,
    scrollSettled,
    draggable,
    focused,
    hasFocus,
    caretToEnd,
    fillPrepare,
    fieldValue,
    selectOptions,
    checkState,
    reaches,
    focusMark,
    acted,
    focusMoved,
    expectElements,
    expectPage,
    expectNamed,
    a11y,
    a11yRefocus,
    auditPage,
    stable,
    explainActionable,
    clipped,
    scrollMark,
    scrolledSince,
    layoutCheck,
    textNames,
    hideText,
    showText,
    stretchText,
    unstretchText,
    scrollPage,
    pageExtent,
    contentBox,
    formsList,
    pickOption,
    elementsAt,
    exploreCandidates,
    fingerprint,
    healCandidates,
    healStable
  };
  if (!globalThis.__ccdp) globalThis.__ccdp = api;
})();
