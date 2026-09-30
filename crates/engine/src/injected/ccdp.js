// Built by `cargo xtask build-js` from src/*.ts; do not edit. sources: 18a6410583ef99f3
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
      for (let a = d; a && !around.has(a); a = a === el ? null : parentOf(a)) around.add(a);
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
    for (let a = el; a; a = parentOf(a)) {
      if (test2(a, getComputedStyle(a))) return a;
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
    for (let a = parentOf(el); a && test2(getComputedStyle(a)); a = parentOf(a)) top = a;
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
      const skipped = closest(el, (a, s) => a !== el && s.contentVisibility === "hidden");
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
      const parent = closest(el, (a) => a !== el && a.matches("fieldset:disabled, select:disabled, optgroup:disabled"));
      if (parent) return { why: `it is inside a disabled ${outline(parent)}`, what: "inside the disabled {el}", el: parent };
      return { why: "it is disabled", what: "disabled" };
    }
    const aria = closest(el, (a) => a.getAttribute("aria-disabled") === "true");
    if (aria) return causeOn(el, 'aria-disabled="true"', aria);
    return null;
  }
  function disabled(el) {
    return disabledCause(el)?.why ?? null;
  }
  function inertCause(el) {
    const inert = closest(el, (a) => a.hasAttribute("inert"));
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
    for (let a = parentOf(start); a && a !== doc.body && a !== doc.documentElement; a = parentOf(a)) {
      if (el && contains(a, el)) break;
      top = a;
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
    for (let a = el; a; a = parentOf(a)) {
      if (a === root || a === document.body) return null;
      if (scrollable(a)) return a;
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
    for (let a = el; a; a = parentOf(a)) {
      if (a.getAttribute("draggable") === "false") return false;
      if (a instanceof HTMLElement && a.draggable) return true;
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
        const listed = options.slice(0, LISTED2).map((o) => ({ ...option(o), disabled: o.disabled || void 0 }));
        return JSON.stringify({ missing: v, options: listed, more: Math.max(0, options.length - LISTED2) });
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
    for (let a = hit instanceof Element ? hit : parentOf(hit); a; a = parentOf(a)) {
      if (a instanceof HTMLLabelElement) return a.control === el;
    }
    return false;
  }

  // src/report.ts
  var before = null;
  var actedOn = [];
  var isFrame = (el) => el instanceof HTMLIFrameElement || el instanceof HTMLFrameElement;
  function formOf(el) {
    if (!el) return null;
    const owner = el.form;
    return owner instanceof HTMLFormElement ? owner : el.closest("form");
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
    for (const a of actedOn) {
      if (a === el || contains(a, el) || a instanceof HTMLLabelElement && a.control === el) return null;
    }
    if (mark.away) return actedOn.some((a) => formOf(a) === formOf(el)) ? null : el;
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
    const a = colour(actual);
    return a !== null && a === colour(p.value);
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
        const shown2 = visible(el);
        const why = shown2 ? null : invisible(el);
        const state = shown2 ? "visible" : `hidden${why ? ` (${why})` : ""}`;
        return one2(m === "visible" ? shown2 : !shown2, state);
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
        const read = value(el);
        if (read.error) return { pass: false, invalid: read.error, count };
        if (m === "value") {
          const actual2 = Array.isArray(read.value) ? read.value.join(", ") : read.value || "";
          return one2(test(spec.pattern)(actual2), "", cut4(actual2));
        }
        const actual = Array.isArray(read.value) ? read.value : [read.value || ""];
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
    const shown2 = els.slice(0, 3);
    return { json: JSON.stringify(shown2.map(describe)), els: shown2 };
  }

  // src/a11y.ts
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
    let a = m[4] === void 0 ? 1 : parseFloat(m[4]);
    if (m[4] && m[4].endsWith("%")) a /= 100;
    return [parseFloat(m[1]), parseFloat(m[2]), parseFloat(m[3]), a];
  }
  function over(top, under) {
    const a = top[3];
    return [0, 1, 2].map((i) => top[i] * a + under[i] * (1 - a)).concat(1);
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
    for (let a = el; a; a = a.parentElement) {
      const style = getComputedStyle(a);
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
  function contrast(el) {
    const own = Array.from(el.childNodes).some((n) => n.nodeType === Node.TEXT_NODE && n.data.trim());
    if (!own || !visible(el) || disabled(el)) return null;
    const style = getComputedStyle(el);
    const fg = parseColour(style.color);
    const bg = background(el);
    if (!fg || !bg) return null;
    const text2 = over(fg, bg);
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
    }
  ];
  var LISTED3 = 5;
  function a11y(root, level2, page = true) {
    const rules = RULES.filter((r) => level2 === "AA" || r.level === "A");
    const violations = [];
    const els = [];
    const scope = root || document.documentElement;
    const all = [scope, ...descendants(scope)];
    const hidden3 = new Map(all.map((el) => [el, hiddenForAria(el)]));
    for (const rule of rules) {
      const v = { rule: rule.id, wcag: rule.wcag, level: rule.level, help: rule.help, count: 0, nodes: [] };
      if (rule.page) {
        if (root || !page) continue;
        const why = rule.page(document);
        if (why) violations.push({ ...v, count: 1, page: why });
        continue;
      }
      for (const el of all) {
        if (rule.id === "aria-hidden-focus" ? !hidden3.get(el) || !visible(el) : hidden3.get(el)) continue;
        const found = rule.check?.(el);
        if (!found) continue;
        v.count++;
        if (v.nodes.length < LISTED3) {
          v.nodes.push({ candidate: describe(el), why: typeof found === "string" ? found : void 0, index: els.length });
          els.push(el);
        }
      }
      if (v.count) violations.push(v);
    }
    return { json: JSON.stringify({ rules: rules.length, violations }), els };
  }

  // src/stable.ts
  var LONGEST = 80;
  var quoted = (s) => '"' + s.replace(/[\\"]/g, (m) => "\\" + m) + '"';
  var bare = (s) => !!s && !/^["'/]|>>|[\[\]]/.test(s);
  function candidates(el, testid, texts) {
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
      const locator = testid === "data-testid" ? `testid=${id}` : `css=[${testid}=${quoted(id)}]`;
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
  function containers(a, role) {
    const out = [];
    const name = nameOf(a, role);
    if (name && name.length <= LONGEST) {
      out.push({
        steps: [{ kind: "role", role, name: { mode: "exact", value: name } }],
        locator: `role=${role}[name=${quoted(name)}]`
      });
    }
    const text2 = firstText(a);
    if (bare(text2)) {
      out.push({
        steps: [{ kind: "role", role }, { kind: "has-text", text: { mode: "substring", value: text2 } }],
        locator: `role=${role} >> has-text=${text2}`
      });
    }
    return out.filter((c) => only(c.steps, a));
  }
  function leafText(a, el, texts) {
    for (const leaf of Array.from(a.querySelectorAll("*"))) {
      if (leaf.childElementCount > 0 || el.contains(leaf) || leaf.contains(el)) continue;
      const text2 = norm(texts.of(leaf));
      if (text2) return text2.length <= LONGEST ? text2 : "";
    }
    return "";
  }
  function* tried(el, testid, texts) {
    const own = candidates(el, testid, texts);
    for (const c of own) yield { ...c, how: c.steps[0].kind };
    if (own.length === 0) return;
    const around = [];
    for (let a = parentOf(el); a; a = parentOf(a)) {
      const role = roleOf(a);
      if (role && CONTEXT_ROLES.has(role)) around.push([a, role]);
    }
    for (const [a, role] of around) {
      for (const outer of containers(a, role)) {
        for (const c of own) {
          yield { steps: [...outer.steps, ...c.steps], locator: `${outer.locator} >> ${c.locator}`, how: "scoped" };
        }
      }
    }
    for (const [a, role] of around) {
      const text2 = leafText(a, el, texts);
      if (!text2) continue;
      const outer = [
        { kind: "role", role },
        { kind: "has", steps: [{ kind: "text", text: { mode: "exact", value: text2 } }] }
      ];
      if (!only(outer, a)) continue;
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
  function stable(el, testid, all) {
    const texts = new Texts();
    const usable = [];
    const others = [];
    let best = null;
    let any = false;
    const own = /* @__PURE__ */ new Set(["role", "label", "testid", "placeholder", "text"]);
    for (const c of tried(el, testid || "data-testid", texts)) {
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
  function animationName(a) {
    if (typeof CSSAnimation !== "undefined" && a instanceof CSSAnimation) return `animation "${a.animationName}"`;
    if (typeof CSSTransition !== "undefined" && a instanceof CSSTransition) return `transition of ${a.transitionProperty}`;
    return a.id ? `animation "${a.id}"` : "a script animation";
  }
  function animations(el, els) {
    const running = (a) => a.getAnimations().filter((x) => x.playState === "running" && x.effect?.target === a);
    const parts = [];
    let index;
    const own = running(el);
    if (own.length) parts.push(`${own.slice(0, 2).map(animationName).join(", ")} on it`);
    for (let a = parentOf(el); a; a = parentOf(a)) {
      const theirs = running(a);
      if (!theirs.length) continue;
      index = els.add(a);
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
    for (let a = parentOf(el); a; a = parentOf(a)) {
      const s = getComputedStyle(a);
      const clips2 = (o) => o === "hidden" || o === "clip";
      const b = a.getBoundingClientRect();
      const outX = clips2(s.overflowX) && (r.right <= b.left || r.left >= b.right);
      const outY = clips2(s.overflowY) && (r.bottom <= b.top || r.top >= b.bottom);
      if (outX || outY) {
        const overflow = outX ? s.overflowX : s.overflowY;
        return els.answer({ text: `clipped out of {el} (overflow: ${overflow})`, el: els.add(a) });
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
  var offsetOf = (a) => a ? [a.scrollLeft, a.scrollTop] : [scrollX, scrollY];
  function scrollMark(el) {
    marks = /* @__PURE__ */ new Map();
    const root = document.scrollingElement || document.documentElement;
    for (let a = parentOf(el); a && a !== root && a !== document.body; a = parentOf(a)) {
      if (a.scrollHeight > a.clientHeight || a.scrollWidth > a.clientWidth) marks.set(a, offsetOf(a));
    }
    marks.set(null, offsetOf(null));
  }
  function scrolledSince() {
    const els = new Els();
    const moved = [];
    for (const [a, [x, y]] of marks) {
      const [nx, ny] = offsetOf(a);
      const dx = px(nx - x);
      const dy = px(ny - y);
      if (dx || dy) moved.push({ dx, dy, el: els.add(a) });
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
    for (let a = el; a && parts.length < 5; a = parentOf(a)) {
      const t = tag(a);
      if (t === "html" || t === "body") break;
      const id = a.getAttribute("id");
      if (id && /^[A-Za-z][\w-]*$/.test(id)) {
        parts.unshift(`#${id}`);
        break;
      }
      const cls = norm(a.getAttribute("class")).split(" ")[0];
      let part = cls && /^[A-Za-z_][\w-]*$/.test(cls) ? `${t}.${cls}` : t;
      const parent = a.parentElement;
      if (parent) {
        const same = Array.from(parent.children).filter((c) => tag(c) === t);
        if (same.length > 1) part += `:nth-of-type(${same.indexOf(a) + 1})`;
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
  function intersect(a, b) {
    const x = Math.max(a[0], b[0]);
    const y = Math.max(a[1], b[1]);
    const right = Math.min(a[0] + a[2], b[0] + b[2]);
    const bottom = Math.min(a[1] + a[3], b[1] + b[3]);
    return right > x && bottom > y ? [x, y, right - x, bottom - y] : null;
  }
  var area = (b) => b[2] * b[3];
  var centre = (b) => [b[0] + b[2] / 2, b[1] + b[3] / 2];
  function clipBox(el, box) {
    for (let a = parentOf(el); a && box; a = parentOf(a)) {
      const t = tag(a);
      if (t === "html" || t === "body") break;
      const s = getComputedStyle(a);
      if (s.overflowX === "visible" && s.overflowY === "visible") continue;
      const p = paddingBox(a);
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
  var largest = (boxes) => boxes.reduce((a, b) => area(b) > area(a) ? b : a);
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
    let a = m[4] === void 0 ? 1 : parseFloat(m[4]);
    if (m[4] && m[4].endsWith("%")) a /= 100;
    return [parseFloat(m[1]), parseFloat(m[2]), parseFloat(m[3]), a];
  }
  function opacity(el) {
    let o = 1;
    for (let a = el; a; a = parentOf(a)) o *= parseFloat(getComputedStyle(a).opacity) || 0;
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
    const shown2 = /* @__PURE__ */ new Map();
    const shownOf = (s) => {
      if (!shown2.has(s.el)) shown2.set(s.el, shownBoxes(s.el, s.style));
      return shown2.get(s.el);
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
      }).filter((b) => b.parts.length).sort((a, b) => a.left - b.left);
      for (let i = 0; i < boxes.length; i++) {
        const a = boxes[i];
        for (let j = i + 1; j < boxes.length && boxes[j].left < a.right; j++) {
          const b = boxes[j];
          let both = 0;
          let width = 0;
          let height = 0;
          for (const pa of a.parts) {
            for (const pb of b.parts) {
              const part = intersect(pa, pb);
              if (!part) continue;
              both += area(part);
              width = Math.max(width, part[2]);
              height = Math.max(height, part[3]);
            }
          }
          if (both <= 4 || both < 0.1 * Math.min(a.size, b.size)) continue;
          if (contains(a.el, b.el) || contains(b.el, a.el)) continue;
          const [first, second] = a.el.compareDocumentPosition(b.el) & Node.DOCUMENT_POSITION_FOLLOWING ? [a, b] : [b, a];
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
        for (let a = s.el; a; a = parentOf(a)) {
          const t = tag(a);
          if (t === "html" || t === "body") break;
          const style = a === s.el ? s.style : getComputedStyle(a);
          const cutX = clips(style.overflowX);
          const cutY = clips(style.overflowY);
          if (!cutX && !cutY) {
            if (style.position === "fixed") break;
            continue;
          }
          const p = paddingBox(a);
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
            own: a === s.el
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
            others: a === s.el ? [] : [els.add(a)],
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
        for (let a = parentOf(el); a; a = parentOf(a)) {
          const t = tag(a);
          if (t === "html" || t === "body") return false;
          const st = getComputedStyle(a);
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
        const a = c[3] * opacity(s.el);
        if (a <= 0.01) continue;
        if (texts.length >= options.texts) {
          skipped++;
          continue;
        }
        const size2 = parseFloat(s.style.fontSize) || 16;
        const weight = parseInt(s.style.fontWeight, 10) || 400;
        texts.push({ box, colour: [c[0], c[1], c[2], a], large: size2 >= 24 || size2 >= 18.66 && weight >= 700 });
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
    contentBox
  };
  if (!globalThis.__ccdp) globalThis.__ccdp = api;
})();
