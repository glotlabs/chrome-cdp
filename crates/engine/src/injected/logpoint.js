// Built by `cargo xtask build-js` from src/*.ts; do not edit. sources: 749bc8e31ff5983c
"use strict";
var __ccdpLogpoint = (() => {
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

  // src/logpoint.ts
  var logpoint_exports = {};
  __export(logpoint_exports, {
    hit: () => hit,
    locals: () => locals
  });

  // src/frameworks.ts
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
  var PREVIEW_DEPTH = 2;
  var PREVIEW_STRING = 80;
  var PREVIEW_ENTRIES = 5;
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

  // src/logpoint.ts
  function thrown(error) {
    try {
      if (error instanceof Error) return `${error.name}: ${error.message}`;
      return `Uncaught ${preview(error)}`;
    } catch {
      return "error";
    }
  }
  function show(values) {
    return values.map((value) => {
      try {
        return { value: preview(value()) };
      } catch (e) {
        return { error: thrown(e) };
      }
    });
  }
  function hit(binding, id, values) {
    const shown = show(values);
    const send = globalThis[binding];
    if (typeof send === "function") send(JSON.stringify({ id, values: shown }));
  }
  function locals(values) {
    return JSON.stringify(show(values));
  }
  return __toCommonJS(logpoint_exports);
})();
