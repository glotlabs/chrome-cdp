// Built by `cargo xtask build-js` from src/*.ts; do not edit. sources: 2002db9f620dffbd
"use strict";
var __ccdpFrameworks = (() => {
  var __defProp = Object.defineProperty;
  var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
  var __getOwnPropNames = Object.getOwnPropertyNames;
  var __hasOwnProp = Object.prototype.hasOwnProperty;
  var __export = (target, all) => {
    for (var name in all)
      __defProp(target, name, { get: all[name], enumerable: true });
  };
  var __copyProps = (to, from, except, desc) => {
    if (from && typeof from === "object" || typeof from === "function") {
      for (let key of __getOwnPropNames(from))
        if (!__hasOwnProp.call(to, key) && key !== except)
          __defProp(to, key, { get: () => from[key], enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable });
    }
    return to;
  };
  var __toCommonJS = (mod) => __copyProps(__defProp({}, "__esModule", { value: true }), mod);

  // src/frameworks.ts
  var frameworks_exports = {};
  __export(frameworks_exports, {
    component: () => component,
    preview: () => preview,
    source: () => source,
    sources: () => sources,
    tree: () => tree
  });
  var MAX_COMPONENTS = 64;
  var MAX_FRAMES = 12;
  function fiberOf(el) {
    for (const key of Object.keys(el)) {
      if (key.startsWith("__reactFiber$")) return el[key];
    }
    return void 0;
  }
  function reactName(type, depth = 0) {
    if (!type || depth > 5) return "Anonymous";
    if (typeof type === "string") return type;
    if (typeof type === "function") return type.displayName || type.name || "Anonymous";
    if (typeof type === "object") {
      if (typeof type.displayName === "string" && type.displayName) return type.displayName;
      if (type.render) return reactName(type.render, depth + 1);
      if (type.type) return reactName(type.type, depth + 1);
      if (typeof type.name === "string" && type.name) return type.name;
    }
    return "Anonymous";
  }
  function parseStack(stack, max = MAX_FRAMES) {
    const frames = [];
    for (const line of stack.split("\n")) {
      const m = /^\s*at (?:(.*?) \((.*):(\d+):(\d+)\)|(.*):(\d+):(\d+))$/.exec(line);
      if (!m) continue;
      const [fn, url, l, c] = m[2] !== void 0 ? [m[1], m[2], m[3], m[4]] : ["", m[5], m[6], m[7]];
      frames.push({ fn: fn || "", url: url || "", line: Number(l), column: Number(c) });
      if (frames.length >= max) break;
    }
    return frames;
  }
  function reactPlace(fiber, frames = MAX_FRAMES) {
    const source2 = fiber?._debugSource;
    if (source2 && typeof source2.fileName === "string") {
      return { file: source2.fileName, line: source2.lineNumber, column: source2.columnNumber };
    }
    const stack = fiber?._debugStack;
    if (stack && typeof stack.stack === "string") {
      return { frames: parseStack(stack.stack, frames) };
    }
    return void 0;
  }
  function react(el) {
    const fiber = fiberOf(el);
    if (!fiber) return void 0;
    const nineteen = "_debugStack" in fiber;
    const dev = nineteen || "_debugSource" in fiber;
    const found = {
      framework: "react",
      version: nineteen ? "19" : dev ? "18" : "",
      dev,
      components: [],
      more: 0
    };
    if (!dev) return found;
    found.at = reactPlace(fiber);
    let below2 = fiber;
    let owner = fiber._debugOwner;
    const seen = /* @__PURE__ */ new Set();
    while (owner && !seen.has(owner)) {
      seen.add(owner);
      if (found.components.length >= MAX_COMPONENTS) {
        found.more++;
      } else {
        const name = owner.type !== void 0 ? reactName(owner.type) : String(owner.name || "Anonymous");
        found.components.push({ name, place: reactPlace(below2) });
      }
      below2 = owner;
      owner = owner._debugOwner ?? owner.owner;
    }
    return found;
  }
  function vueName(type) {
    if (!type) return "Anonymous";
    const name = type.__name || type.name || type.displayName;
    if (name) return String(name);
    const file = typeof type.__file === "string" ? type.__file : "";
    const base = file.split("/").pop() || "";
    return base.replace(/\.vue$/, "") || "Anonymous";
  }
  function aboveHas(el, key) {
    for (let at = el.parentElement; at; at = at.parentElement) {
      if (at[key]) return true;
    }
    return false;
  }
  function vueApp(el) {
    for (let at = el; at; at = at.parentElement) {
      const app = at.__vue_app__;
      if (app) return app;
    }
    return void 0;
  }
  function vueElementPlace(el, instance) {
    const tag = el.localName;
    const same = Array.from(el.ownerDocument.getElementsByTagName(tag)).filter(
      (e) => e.__vueParentComponent === instance
    );
    const text = (el.textContent || "").trim().slice(0, 80);
    const match = { tag, index: same.indexOf(el), count: same.length, text };
    const file = typeof instance.type?.__file === "string" ? instance.type.__file : void 0;
    return { file, match };
  }
  function vue(el) {
    const instance = el.__vueParentComponent;
    if (!instance) {
      const app = vueApp(el);
      if (!app || aboveHas(el, "__vueParentComponent")) return void 0;
      return { framework: "vue", version: String(app.version || ""), dev: false, components: [], more: 0 };
    }
    const found = {
      framework: "vue",
      version: String(instance.appContext?.app?.version || ""),
      dev: true,
      components: [],
      more: 0
    };
    found.at = vueElementPlace(el, instance);
    let below2 = null;
    const seen = /* @__PURE__ */ new Set();
    for (let at = instance; at && !seen.has(at); at = at.parent) {
      seen.add(at);
      if (found.components.length >= MAX_COMPONENTS) {
        found.more++;
        continue;
      }
      const atFile = typeof at.type?.__file === "string" ? at.type.__file : void 0;
      const place = below2 ? { file: atFile, match: { tag: vueName(below2.type) } } : found.at;
      found.components.push({ name: vueName(at.type), place });
      below2 = at;
    }
    return found;
  }
  function svelteVersion() {
    const versions = globalThis.__svelte?.v;
    if (versions && typeof versions[Symbol.iterator] === "function") {
      const all = Array.from(versions).map(String);
      return all.sort().pop() || "";
    }
    return "";
  }
  function svelteBase(file) {
    const base = String(file || "").split("/").pop() || "";
    return base.replace(/\.svelte$/, "") || "Anonymous";
  }
  function sveltePlace(loc) {
    if (!loc || typeof loc.file !== "string") return void 0;
    const place = { file: loc.file };
    if (typeof loc.line === "number") place.line = loc.line;
    if (typeof loc.column === "number") place.column = loc.column + 1;
    return place;
  }
  function svelte(el) {
    const meta = el.__svelte_meta;
    const version = svelteVersion();
    if (!meta) {
      if (!version || aboveHas(el, "__svelte_meta")) return void 0;
      return { framework: "svelte", version, dev: false, components: [], more: 0 };
    }
    const found = { framework: "svelte", version, dev: true, components: [], more: 0 };
    found.at = sveltePlace(meta.loc);
    let place = found.at;
    let file = meta.loc?.file;
    const seen = /* @__PURE__ */ new Set();
    for (let entry = meta.parent; entry && !seen.has(entry); entry = entry.parent) {
      seen.add(entry);
      if (entry.type !== "component") continue;
      if (found.components.length >= MAX_COMPONENTS) {
        found.more++;
      } else {
        found.components.push({ name: String(entry.componentTag || svelteBase(file)), place });
      }
      place = sveltePlace(entry);
      file = entry.file;
    }
    if (file !== void 0) {
      if (found.components.length >= MAX_COMPONENTS) found.more++;
      else found.components.push({ name: svelteBase(file), place });
    }
    return found;
  }
  var MAX_NODES = 5e3;
  var PREVIEW_DEPTH = 2;
  var PREVIEW_STRING = 80;
  var TREE_STRING = 30;
  var PREVIEW_ENTRIES = 5;
  var TREE_PROPS = 6;
  var TREE_FRAMES = 4;
  function plural(n, one, many) {
    return `${n} ${n === 1 ? one : many}`;
  }
  function isReactElement(value) {
    const kind = value?.$$typeof;
    return typeof kind === "symbol" && /react\.(transitional\.)?element/.test(String(kind.description));
  }
  function preview(value, depth = 0, cut = PREVIEW_STRING, seen = /* @__PURE__ */ new Set()) {
    switch (typeof value) {
      case "string":
        return JSON.stringify(value.length > cut ? `${value.slice(0, cut)}…` : value);
      case "number":
        return Object.is(value, -0) ? "-0" : String(value);
      case "boolean":
        return String(value);
      case "bigint":
        return `${value}n`;
      case "undefined":
        return "undefined";
      case "symbol":
        return value.toString();
      case "function":
        return "ƒ";
    }
    if (value === null) return "null";
    if (seen.has(value)) return "[circular]";
    try {
      if (isReactElement(value)) return `<${reactName(value.type)}>`;
      if (typeof Node !== "undefined" && value instanceof Node) {
        return value instanceof Element ? `<${value.localName}>` : value.nodeName;
      }
      if (value instanceof Date) return `Date(${isNaN(value.getTime()) ? "invalid" : value.toISOString()})`;
      if (value instanceof RegExp) return String(value);
      if (value instanceof Error) return preview(`${value.name}: ${value.message}`, depth, cut, seen);
      if (value instanceof Map) return `Map(${value.size})`;
      if (value instanceof Set) return `Set(${value.size})`;
      if (value instanceof Promise) return "Promise";
      const array = Array.isArray(value);
      const keys = array ? [] : Object.keys(value);
      const size = array ? value.length : keys.length;
      const ctor = array ? "" : value.constructor?.name;
      const prefix = ctor && ctor !== "Object" ? `${ctor} ` : "";
      if (size === 0) return array ? "[]" : `${prefix}{}`;
      if (size > PREVIEW_ENTRIES || depth >= PREVIEW_DEPTH) {
        return array ? `[${plural(size, "item", "items")}]` : `${prefix}{…${plural(size, "key", "keys")}}`;
      }
      seen.add(value);
      const inner = (v) => {
        try {
          return preview(v(), depth + 1, cut, seen);
        } catch {
          return "<error>";
        }
      };
      const parts = array ? Array.from({ length: size }, (_, i) => inner(() => value[i])) : keys.map((k) => `${/^[A-Za-z_$][\w$]*$/.test(k) ? k : JSON.stringify(k)}: ${inner(() => value[k])}`);
      seen.delete(value);
      return array ? `[${parts.join(", ")}]` : `${prefix}{${parts.join(", ")}}`;
    } catch {
      return "<error>";
    }
  }
  function entries(object, skip = () => false) {
    const out = [];
    if (!object || typeof object !== "object") return out;
    for (const key of Object.keys(object)) {
      let value;
      try {
        value = object[key];
      } catch {
        out.push({ name: key, value: "<error>" });
        continue;
      }
      if (!skip(key, value)) out.push({ name: key, value: preview(value) });
    }
    return out;
  }
  function treeProps(object) {
    const out = [];
    if (!object || typeof object !== "object") return out;
    for (const key of Object.keys(object)) {
      if (key === "children" || key === "key" || key === "ref") continue;
      let value;
      try {
        value = object[key];
      } catch {
        continue;
      }
      if (typeof value === "function") continue;
      if (out.length >= TREE_PROPS) {
        out.push({ name: "…", value: "" });
        break;
      }
      out.push({ name: key, value: preview(value, PREVIEW_DEPTH, TREE_STRING) });
    }
    return out;
  }
  function node(name, place, props) {
    return { name, place, props, children: [], count: 1, more: 0 };
  }
  function below(n) {
    return n.children.reduce((sum, c) => sum + c.count * (1 + below(c)) + c.more, 0);
  }
  function shape(n, depth) {
    if (depth <= 1) {
      n.more += below(n);
      n.children = [];
      return;
    }
    for (const c of n.children) shape(c, depth - 1);
    const folded = [];
    let last = "";
    for (const c of n.children) {
      const key = JSON.stringify([c.name, c.place, c.props, c.children, c.more]);
      const prev = folded[folded.length - 1];
      if (prev && key === last) {
        prev.count += c.count;
      } else {
        folded.push(c);
        last = key;
      }
    }
    n.children = folded;
  }
  function allElements(doc) {
    const out = [];
    const visit = (root) => {
      for (const el of Array.from(root.querySelectorAll("*"))) {
        out.push(el);
        if (el.shadowRoot) visit(el.shadowRoot);
      }
    };
    visit(doc);
    return out;
  }
  var REACT_COMPONENTS = /* @__PURE__ */ new Set([0, 1, 2, 11, 15]);
  function isReactComponent(fiber) {
    return REACT_COMPONENTS.has(fiber?.tag) && typeof fiber.type !== "string";
  }
  function reactOwnPlace(fiber, frames = MAX_FRAMES) {
    const stack = [fiber.child];
    let seen = 0;
    while (stack.length && seen++ < 1e3) {
      const f = stack.pop();
      if (!f) continue;
      if (f._debugOwner === fiber || f._debugOwner === fiber.alternate) return reactPlace(f, frames);
      stack.push(f.sibling, f.child);
    }
    return void 0;
  }
  function reactVersion(fiber) {
    if (!fiber) return { version: "", dev: false };
    const nineteen = "_debugStack" in fiber;
    const dev = nineteen || "_debugSource" in fiber;
    return { version: nineteen ? "19" : dev ? "18" : "", dev };
  }
  function reactChildren(fiber) {
    const out = [];
    const visit = (f, depth) => {
      for (; f; f = f.sibling) {
        if (isReactComponent(f)) out.push(f);
        else if (depth < 2e3) visit(f.child, depth + 1);
      }
    };
    visit(fiber.child, 0);
    return out;
  }
  function reactTrees(elements, tree2, budget) {
    for (const el of elements) {
      const key = Object.keys(el).find((k) => k.startsWith("__reactContainer$"));
      if (!key) continue;
      const host = el[key];
      const root = host?.stateNode?.current ?? host;
      const tops = reactChildren(root);
      const { version, dev } = reactVersion(tops[0] ?? root.child);
      tree2.frameworks.push({ framework: "react", version, dev });
      if (!dev) continue;
      const build = (f) => {
        if (budget.left <= 0) {
          tree2.more++;
          return void 0;
        }
        budget.left--;
        const n = node(reactName(f.type), reactOwnPlace(f, TREE_FRAMES), treeProps(f.memoizedProps));
        for (const c of reactChildren(f)) {
          const child = build(c);
          if (child) n.children.push(child);
        }
        return n;
      };
      for (const top of tops) {
        const n = build(top);
        if (n) tree2.roots.push({ ...n, framework: "react", version });
      }
    }
  }
  function reactState(fiber) {
    if (fiber.tag === 1) return entries(fiber.stateNode?.state);
    const out = [];
    let index = 0;
    for (let hook = fiber.memoizedState; hook && index < 100; hook = hook.next, index++) {
      const queue = hook.queue;
      if (!queue || typeof queue !== "object" || !("pending" in queue)) continue;
      const reducer = queue.lastRenderedReducer;
      const kind = reducer && reducer.name === "basicStateReducer" ? "useState" : "useReducer";
      out.push({ name: `#${index}`, value: preview(hook.memoizedState), hook: kind });
    }
    return out;
  }
  function reactHeld(el) {
    const found = react(el);
    if (!found) return void 0;
    const fiber = fiberOf(el);
    let at = fiber?._debugOwner;
    if (!isReactComponent(at)) {
      at = fiber?.return;
      while (at && !isReactComponent(at)) at = at.return;
    }
    const held = {
      framework: "react",
      version: found.version,
      dev: found.dev,
      name: at ? reactName(at.type) : "",
      props: null,
      state: null,
      owners: [],
      more: found.more
    };
    if (!found.dev || !at) return held;
    const alternate = at.alternate;
    if (alternate && fiberIsCurrent(alternate)) at = alternate;
    held.place = reactOwnPlace(at);
    held.props = entries(at.memoizedProps);
    held.state = reactState(at);
    held.owners = found.components.slice(1);
    return held;
  }
  function fiberIsCurrent(fiber) {
    let top = fiber;
    while (top.return) top = top.return;
    return top.stateNode?.current === top;
  }
  function vueComponents(vnode, out, depth = 0) {
    if (!vnode || typeof vnode !== "object" || depth > 2e3) return out;
    if (vnode.component) {
      out.push(vnode.component);
    } else if (vnode.suspense) {
      vueComponents(vnode.suspense.activeBranch, out, depth + 1);
    } else if (Array.isArray(vnode.children)) {
      for (const child of vnode.children) vueComponents(child, out, depth + 1);
    }
    return out;
  }
  function vueOwnPlace(instance) {
    const el = instance.subTree?.el;
    if (el instanceof Element && el.__vueParentComponent === instance) {
      const place = vueElementPlace(el, instance);
      if (place.match) delete place.match.text;
      return place;
    }
    const file = typeof instance.type?.__file === "string" ? instance.type.__file : void 0;
    return { file };
  }
  function vueTrees(elements, tree2, budget) {
    for (const el of elements) {
      const app = el.__vue_app__;
      const top = app?._instance;
      if (!top) continue;
      const version = String(app.version || "");
      const first = top.subTree?.el;
      const dev = !!first && !!first.__vueParentComponent;
      tree2.frameworks.push({ framework: "vue", version, dev });
      if (!dev) continue;
      const build = (instance) => {
        if (budget.left <= 0) {
          tree2.more++;
          return void 0;
        }
        budget.left--;
        const n2 = node(vueName(instance.type), vueOwnPlace(instance), treeProps(instance.props));
        for (const c of vueComponents(instance.subTree, [])) {
          const child = build(c);
          if (child) n2.children.push(child);
        }
        return n2;
      };
      const n = build(top);
      if (n) tree2.roots.push({ ...n, framework: "vue", version });
    }
  }
  function vueRaw(value) {
    for (let i = 0; i < 8 && value && typeof value === "object" && value.__v_raw; i++) value = value.__v_raw;
    return value;
  }
  function isVueComponent(value) {
    return !!value && typeof value === "object" && ("__name" in value || "__file" in value || "render" in value || "setup" in value);
  }
  function vueHeld(el) {
    const found = vue(el);
    if (!found) return void 0;
    const held = {
      framework: "vue",
      version: found.version,
      dev: found.dev,
      name: found.components[0]?.name ?? "",
      props: null,
      state: null,
      owners: [],
      more: found.more
    };
    const instance = el.__vueParentComponent;
    if (!found.dev || !instance) return held;
    held.place = vueOwnPlace(instance);
    held.props = entries(instance.props);
    const props = vueRaw(instance.props);
    const state = entries(
      instance.setupState,
      (_, value) => typeof value === "function" || vueRaw(value) === props || isVueComponent(value)
    );
    state.push(...entries(instance.data));
    held.state = state;
    held.owners = found.components.slice(1);
    return held;
  }
  function svelteOwner(meta) {
    for (let entry = meta?.parent; entry; entry = entry.parent) {
      if (entry.type === "component") return entry;
    }
    return null;
  }
  var SVELTE_UNAVAILABLE = "Svelte 5 has no public API for a component's props or state";
  function svelteTrees(elements, tree2, budget) {
    const version = svelteVersion();
    const nodes = /* @__PURE__ */ new Map();
    const roots = /* @__PURE__ */ new Map();
    let found = false;
    const nodeOf = (entry, file) => {
      if (!entry) {
        let root = roots.get(file);
        if (!root) {
          root = node(svelteBase(file), void 0, []);
          roots.set(file, root);
        }
        return root;
      }
      let n = nodes.get(entry);
      if (n) return n;
      const parent = nodeOf(svelteOwner(entry), String(entry.file));
      if (!parent) return void 0;
      if (budget.left <= 0) {
        tree2.more++;
        return void 0;
      }
      budget.left--;
      n = node(String(entry.componentTag || "Anonymous"), void 0, []);
      nodes.set(entry, n);
      parent.children.push(n);
      return n;
    };
    for (const el of elements) {
      const meta = el.__svelte_meta;
      if (!meta?.loc) continue;
      found = true;
      const n = nodeOf(svelteOwner(meta), String(meta.loc.file));
      if (n && !n.place) n.place = sveltePlace(meta.loc);
    }
    if (!found) {
      if (version) tree2.frameworks.push({ framework: "svelte", version, dev: false });
      return;
    }
    tree2.frameworks.push({ framework: "svelte", version, dev: true });
    for (const root of roots.values()) tree2.roots.push({ ...root, framework: "svelte", version });
  }
  function svelteHeld(el) {
    const found = svelte(el);
    if (!found) return void 0;
    const held = {
      framework: "svelte",
      version: found.version,
      dev: found.dev,
      name: found.components[0]?.name ?? "",
      props: null,
      state: null,
      unavailable: SVELTE_UNAVAILABLE,
      owners: found.components.slice(1),
      more: found.more
    };
    const meta = el.__svelte_meta;
    if (!found.dev || !meta) return held;
    const owner = svelteOwner(meta);
    for (const other of allElements(el.ownerDocument)) {
      const m = other.__svelte_meta;
      if (!m?.loc) continue;
      if (owner ? svelteOwner(m) === owner : !svelteOwner(m) && m.loc.file === meta.loc?.file) {
        held.place = sveltePlace(m.loc);
        break;
      }
    }
    return held;
  }
  function source(el) {
    let fallback = null;
    for (const adapter of [react, vue, svelte]) {
      let found;
      try {
        found = adapter(el);
      } catch {
        found = void 0;
      }
      if (found?.dev) return found;
      if (found && !fallback) fallback = found;
    }
    return fallback;
  }
  function sources(els) {
    return els.map((el) => el instanceof Element ? source(el) : null);
  }
  function tree(depth) {
    const out = { frameworks: [], roots: [], more: 0 };
    const elements = allElements(document);
    const budget = { left: MAX_NODES };
    for (const adapter of [reactTrees, vueTrees, svelteTrees]) {
      try {
        adapter(elements, out, budget);
      } catch {
      }
    }
    const top = node("", void 0, []);
    top.children = out.roots;
    shape(top, depth + 1);
    out.roots = top.children;
    return out;
  }
  function component(el) {
    let fallback = null;
    for (const adapter of [reactHeld, vueHeld, svelteHeld]) {
      let held;
      try {
        held = adapter(el);
      } catch {
        held = void 0;
      }
      if (held?.dev && held.name) return held;
      if (held && !fallback) fallback = held;
    }
    return fallback;
  }
  return __toCommonJS(frameworks_exports);
})();
