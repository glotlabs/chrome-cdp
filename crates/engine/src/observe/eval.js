// `eval`'s last step, called on the script's value (`this`) when it is an
// object or a function: a function is called (with `el`, the `--in`
// element), a promise awaited, and the value turned into JSON text that
// keeps its keys' order. DOM nodes, cycles, `NaN`, bigints and the like
// become strings that say what they were; `budget` bounds the work on a
// huge value.
//
// Elements of the frame's document are collected in `els` (the first
// `cap` of them) and marked in the text with `mark`, their index and `⟧`,
// where the caller puts their refs; `over` counts the elements past the
// cap. The answer goes back by reference, so `els` stays a list of nodes.
async function (el, mark, cap) {
  'use strict'; // Keeps a symbol `this` a symbol.
  let value = this;
  if (typeof value === 'function') value = value(el);
  value = await value;
  let budget = 20000;
  const ancestors = new Set();
  const els = [];
  const indexes = new Map();
  const over = new Set();
  const tag = (v) => Object.prototype.toString.call(v).slice(8, -1);
  const isNode = (v) => {
    try {
      return typeof v.nodeType === 'number' && typeof v.nodeName === 'string'
        && typeof v.cloneNode === 'function';
    } catch {
      return false;
    }
  };
  const node = (n) => {
    if (n.nodeType === 1) {
      let text = n.localName;
      if (n.id) text += `#${n.id}`;
      for (const c of n.classList || []) text += `.${c}`;
      if (!n.isConnected || n.ownerDocument !== document) return text;
      let i = indexes.get(n);
      if (i === undefined && els.length < cap) {
        i = els.length;
        indexes.set(n, i);
        els.push(n);
      }
      if (i === undefined) {
        over.add(n);
        return text;
      }
      return `${text}${mark}${i}⟧`;
    }
    if (n.nodeType === 3) {
      const data = n.data.length > 60 ? `${n.data.slice(0, 60)}…` : n.data;
      return `#text ${JSON.stringify(data)}`;
    }
    return n.nodeName;
  };
  const json = (v, depth) => {
    budget--;
    switch (typeof v) {
      case 'string':
      case 'boolean':
        return v;
      case 'number':
        return Number.isFinite(v) ? v : String(v);
      case 'bigint':
        return `${v}n`;
      case 'symbol':
        return v.toString();
      case 'undefined':
        return null;
      case 'function':
        return `ƒ ${v.name || 'anonymous'}()`;
      default:
    }
    if (v === null) return null;
    if (budget < 0) return '…';
    if (ancestors.has(v)) return '[Circular]';
    if (isNode(v)) return node(v);
    const kind = tag(v);
    if (kind === 'Window') return '[Window]';
    if (kind === 'Date') return Number.isNaN(v.getTime()) ? 'Invalid Date' : v.toISOString();
    if (kind === 'RegExp' || kind === 'Error' || v instanceof Error) return String(v);
    if (depth >= 20) return Array.isArray(v) ? '[Array]' : '[Object]';
    ancestors.add(v);
    try {
      if (typeof v.toJSON === 'function') return json(v.toJSON(), depth + 1);
      const list = Array.isArray(v) || ArrayBuffer.isView(v) || kind === 'Arguments'
        || kind === 'NodeList' || kind === 'HTMLCollection';
      if (list || kind === 'Set') {
        const out = [];
        for (const item of list ? Array.from({ length: v.length }, (_, i) => v[i]) : v) {
          if (budget < 0) {
            out.push('…');
            break;
          }
          out.push(json(item, depth + 1));
        }
        return out;
      }
      if (kind === 'Map' && [...v.keys()].some((k) => typeof k !== 'string')) {
        return json([...v], depth);
      }
      const out = {};
      const entries = kind === 'Map' ? [...v] : Object.keys(v).map((k) => [k]);
      for (const [key, ...given] of entries) {
        if (budget < 0) {
          out['…'] = '…';
          break;
        }
        let item;
        try {
          item = given.length ? given[0] : v[key];
        } catch (e) {
          item = `[Exception: ${e}]`;
        }
        if (item !== undefined) out[key] = json(item, depth + 1);
      }
      return out;
    } catch (e) {
      return `[Exception: ${e}]`;
    } finally {
      ancestors.delete(v);
    }
  };
  let type = value === null ? 'null' : typeof value;
  if (type === 'object') {
    type = Array.isArray(value) ? 'array' : isNode(value) ? 'node' : 'object';
  }
  const text = type === 'undefined' ? '' : JSON.stringify(json(value, 0), null, 2);
  return { type, text, els, over: over.size };
}
