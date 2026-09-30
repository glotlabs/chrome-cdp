// Built by `cargo xtask build-js` from src/*.ts; do not edit. sources: cea84edc3568731f
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
    source: () => source,
    sources: () => sources
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
  function parseStack(stack) {
    const frames = [];
    for (const line of stack.split("\n")) {
      const m = /^\s*at (?:(.*?) \((.*):(\d+):(\d+)\)|(.*):(\d+):(\d+))$/.exec(line);
      if (!m) continue;
      const [fn, url, l, c] = m[2] !== void 0 ? [m[1], m[2], m[3], m[4]] : ["", m[5], m[6], m[7]];
      frames.push({ fn: fn || "", url: url || "", line: Number(l), column: Number(c) });
      if (frames.length >= MAX_FRAMES) break;
    }
    return frames;
  }
  function reactPlace(fiber) {
    const source2 = fiber?._debugSource;
    if (source2 && typeof source2.fileName === "string") {
      return { file: source2.fileName, line: source2.lineNumber, column: source2.columnNumber };
    }
    const stack = fiber?._debugStack;
    if (stack && typeof stack.stack === "string") {
      return { frames: parseStack(stack.stack) };
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
    let below = fiber;
    let owner = fiber._debugOwner;
    const seen = /* @__PURE__ */ new Set();
    while (owner && !seen.has(owner)) {
      seen.add(owner);
      if (found.components.length >= MAX_COMPONENTS) {
        found.more++;
      } else {
        const name = owner.type !== void 0 ? reactName(owner.type) : String(owner.name || "Anonymous");
        found.components.push({ name, place: reactPlace(below) });
      }
      below = owner;
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
    const tag = el.localName;
    const same = Array.from(el.ownerDocument.getElementsByTagName(tag)).filter(
      (e) => e.__vueParentComponent === instance
    );
    const text = (el.textContent || "").trim().slice(0, 80);
    const match = { tag, index: same.indexOf(el), count: same.length, text };
    const file = typeof instance.type?.__file === "string" ? instance.type.__file : void 0;
    found.at = { file, match };
    let below = null;
    const seen = /* @__PURE__ */ new Set();
    for (let at = instance; at && !seen.has(at); at = at.parent) {
      seen.add(at);
      if (found.components.length >= MAX_COMPONENTS) {
        found.more++;
        continue;
      }
      const atFile = typeof at.type?.__file === "string" ? at.type.__file : void 0;
      const place = below ? { file: atFile, match: { tag: vueName(below.type) } } : { file: atFile, match };
      found.components.push({ name: vueName(at.type), place });
      below = at;
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
  return __toCommonJS(frameworks_exports);
})();
