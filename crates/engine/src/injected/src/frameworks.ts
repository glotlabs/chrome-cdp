// Framework adapters (plan 15): what a framework's development build
// knows about where an element came from. Built by `cargo xtask
// build-js` into its own script, `frameworks.js`, which is not installed
// in any world: the engine calls it with `Runtime.callFunctionOn` on
// elements resolved in the page's **main** world, because the page's
// JavaScript properties on DOM nodes (`__reactFiber$…`,
// `__vueParentComponent`, `__svelte_meta`) are not visible from an
// isolated world. It leaves nothing on the page's globals and returns
// plain JSON.
//
// This data comes from the page's own world, so a page can lie: the
// engine shows it as the page's debug info and acts on nothing in it.

/** A frame of a stack the page made (React 19's `_debugStack`), as
 * the script it ran in: 1-based line and column. */
export interface Frame {
  fn: string;
  url: string;
  line: number;
  column: number;
}

/** What to look for in a component file's `<template>` when the
 * framework does not know the line (Vue): an element's tag, its
 * position among the elements of that tag its component rendered
 * (`index` of `count`), and its text; or a child component's tag. */
export interface Match {
  tag: string;
  index?: number;
  count?: number;
  text?: string;
}

/** A place as the framework knows it: a file name (an absolute path, a
 * path relative to the app, or a URL) with 1-based line and column; or
 * a stack to source-map (`frames`); or a file and what to find in it. */
export interface Place {
  file?: string;
  line?: number;
  column?: number;
  frames?: Frame[];
  match?: Match;
}

/** A component around the element, innermost first: its name, and the
 * place in its file that renders the next step towards the element. */
export interface Component {
  name: string;
  place?: Place;
}

/** What an adapter found for an element. `dev` is false when the
 * framework is there but its debug info is not (a production build). */
export interface Found {
  framework: 'react' | 'vue' | 'svelte';
  version: string;
  dev: boolean;
  at?: Place;
  components: Component[];
  /** The owner chain went on past this many components. */
  more: number;
}

/** At most this many components are read; `more` says there were more. */
const MAX_COMPONENTS = 64;
/** At most this many frames of a debug stack are sent. */
const MAX_FRAMES = 12;

type Any = any; // eslint-disable-line @typescript-eslint/no-explicit-any

// ---- React ---------------------------------------------------------

function fiberOf(el: Element): Any {
  for (const key of Object.keys(el)) {
    if (key.startsWith('__reactFiber$')) return (el as Any)[key];
  }
  return undefined;
}

/** A component's name, through `memo` and `forwardRef` wrappers. */
function reactName(type: Any, depth = 0): string {
  if (!type || depth > 5) return 'Anonymous';
  if (typeof type === 'string') return type;
  if (typeof type === 'function') return type.displayName || type.name || 'Anonymous';
  if (typeof type === 'object') {
    if (typeof type.displayName === 'string' && type.displayName) return type.displayName;
    if (type.render) return reactName(type.render, depth + 1); // forwardRef
    if (type.type) return reactName(type.type, depth + 1); // memo
    if (typeof type.name === 'string' && type.name) return type.name;
  }
  return 'Anonymous';
}

/** Frames of a V8 `Error.stack`, after its message line. */
function parseStack(stack: string): Frame[] {
  const frames: Frame[] = [];
  for (const line of stack.split('\n')) {
    const m = /^\s*at (?:(.*?) \((.*):(\d+):(\d+)\)|(.*):(\d+):(\d+))$/.exec(line);
    if (!m) continue;
    const [fn, url, l, c] = m[2] !== undefined ? [m[1], m[2], m[3], m[4]] : ['', m[5], m[6], m[7]];
    frames.push({ fn: fn || '', url: url || '', line: Number(l), column: Number(c) });
    if (frames.length >= MAX_FRAMES) break;
  }
  return frames;
}

/** Where a fiber's JSX element was written: React 18's `_debugSource`,
 * or React 19's `_debugStack` for the engine to source-map. */
function reactPlace(fiber: Any): Place | undefined {
  const source = fiber?._debugSource;
  if (source && typeof source.fileName === 'string') {
    return { file: source.fileName, line: source.lineNumber, column: source.columnNumber };
  }
  const stack = fiber?._debugStack;
  if (stack && typeof stack.stack === 'string') {
    return { frames: parseStack(stack.stack) };
  }
  return undefined;
}

function react(el: Element): Found | undefined {
  const fiber = fiberOf(el);
  if (!fiber) return undefined;
  const nineteen = '_debugStack' in fiber;
  const dev = nineteen || '_debugSource' in fiber;
  const found: Found = {
    framework: 'react',
    version: nineteen ? '19' : dev ? '18' : '',
    dev,
    components: [],
    more: 0,
  };
  if (!dev) return found;
  found.at = reactPlace(fiber);
  // The owner chain: each owner rendered the element below it, so its
  // place is where that element's JSX is (in the owner's file).
  let below: Any = fiber;
  let owner: Any = fiber._debugOwner;
  const seen = new Set<Any>();
  while (owner && !seen.has(owner)) {
    seen.add(owner);
    if (found.components.length >= MAX_COMPONENTS) {
      found.more++;
    } else {
      // A server component's info has a `name`; a fiber has a `type`.
      const name = owner.type !== undefined ? reactName(owner.type) : String(owner.name || 'Anonymous');
      found.components.push({ name, place: reactPlace(below) });
    }
    below = owner;
    owner = owner._debugOwner ?? owner.owner;
  }
  return found;
}

// ---- Vue -----------------------------------------------------------

function vueName(type: Any): string {
  if (!type) return 'Anonymous';
  const name = type.__name || type.name || type.displayName;
  if (name) return String(name);
  const file = typeof type.__file === 'string' ? type.__file : '';
  const base = file.split('/').pop() || '';
  return base.replace(/\.vue$/, '') || 'Anonymous';
}

/** Whether an element above `el` has property `key`. */
function aboveHas(el: Element, key: string): boolean {
  for (let at = el.parentElement; at; at = at.parentElement) {
    if ((at as Any)[key]) return true;
  }
  return false;
}

/** The Vue app an element is in, from its mount container. */
function vueApp(el: Element): Any {
  for (let at: Element | null = el; at; at = at.parentElement) {
    const app = (at as Any).__vue_app__;
    if (app) return app;
  }
  return undefined;
}

function vue(el: Element): Found | undefined {
  const instance = (el as Any).__vueParentComponent;
  if (!instance) {
    const app = vueApp(el);
    // In a dev build, an element Vue did not render has no debug info
    // of its own: the node stack answers for it.
    if (!app || aboveHas(el, '__vueParentComponent')) return undefined;
    return { framework: 'vue', version: String(app.version || ''), dev: false, components: [], more: 0 };
  }
  const found: Found = {
    framework: 'vue',
    version: String(instance.appContext?.app?.version || ''),
    dev: true,
    components: [],
    more: 0,
  };
  // Vue does not keep the element's line: the engine finds it in the
  // component's template, by its position among the elements of its
  // tag that the same component rendered.
  const tag = el.localName;
  const same = Array.from(el.ownerDocument.getElementsByTagName(tag)).filter(
    (e) => (e as Any).__vueParentComponent === instance,
  );
  const text = (el.textContent || '').trim().slice(0, 80);
  const match: Match = { tag, index: same.indexOf(el), count: same.length, text };
  const file = typeof instance.type?.__file === 'string' ? instance.type.__file : undefined;
  found.at = { file, match };
  let below: Any = null;
  const seen = new Set<Any>();
  for (let at: Any = instance; at && !seen.has(at); at = at.parent) {
    seen.add(at);
    if (found.components.length >= MAX_COMPONENTS) {
      found.more++;
      continue;
    }
    const atFile = typeof at.type?.__file === 'string' ? at.type.__file : undefined;
    const place: Place = below
      ? { file: atFile, match: { tag: vueName(below.type) } }
      : { file: atFile, match };
    found.components.push({ name: vueName(at.type), place });
    below = at;
  }
  return found;
}

// ---- Svelte --------------------------------------------------------

function svelteVersion(): string {
  const versions = (globalThis as Any).__svelte?.v;
  if (versions && typeof versions[Symbol.iterator] === 'function') {
    const all = Array.from(versions as Iterable<unknown>).map(String);
    return all.sort().pop() || '';
  }
  return '';
}

function svelteBase(file: unknown): string {
  const base = String(file || '').split('/').pop() || '';
  return base.replace(/\.svelte$/, '') || 'Anonymous';
}

/** A Svelte place: 1-based line, 0-based column made 1-based. */
function sveltePlace(loc: Any): Place | undefined {
  if (!loc || typeof loc.file !== 'string') return undefined;
  const place: Place = { file: loc.file };
  if (typeof loc.line === 'number') place.line = loc.line;
  if (typeof loc.column === 'number') place.column = loc.column + 1;
  return place;
}

function svelte(el: Element): Found | undefined {
  const meta = (el as Any).__svelte_meta;
  const version = svelteVersion();
  if (!meta) {
    if (!version || aboveHas(el, '__svelte_meta')) return undefined;
    return { framework: 'svelte', version, dev: false, components: [], more: 0 };
  }
  const found: Found = { framework: 'svelte', version, dev: true, components: [], more: 0 };
  found.at = sveltePlace(meta.loc);
  // `parent` goes up through the blocks (`each`, `if`, …) and the
  // components around the element: a component entry is where that
  // component is used, in the file of the one around it.
  let place = found.at;
  let file = meta.loc?.file;
  const seen = new Set<Any>();
  for (let entry: Any = meta.parent; entry && !seen.has(entry); entry = entry.parent) {
    seen.add(entry);
    if (entry.type !== 'component') continue;
    if (found.components.length >= MAX_COMPONENTS) {
      found.more++;
    } else {
      found.components.push({ name: String(entry.componentTag || svelteBase(file)), place });
    }
    place = sveltePlace(entry);
    file = entry.file;
  }
  if (file !== undefined) {
    if (found.components.length >= MAX_COMPONENTS) found.more++;
    else found.components.push({ name: svelteBase(file), place });
  }
  return found;
}

// ---- Entry points --------------------------------------------------

/** What the framework that rendered `el` knows about its source; `null`
 * when no framework is found. A framework whose debug info is there
 * wins over one found without it. */
export function source(el: Element): Found | null {
  let fallback: Found | null = null;
  for (const adapter of [react, vue, svelte]) {
    let found: Found | undefined;
    try {
      found = adapter(el);
    } catch {
      found = undefined;
    }
    if (found?.dev) return found;
    if (found && !fallback) fallback = found;
  }
  return fallback;
}

/** `source` for many elements at once (`snapshot --source`). */
export function sources(els: Element[]): (Found | null)[] {
  return els.map((el) => (el instanceof Element ? source(el) : null));
}
