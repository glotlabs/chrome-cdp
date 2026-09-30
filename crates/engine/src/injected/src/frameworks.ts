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

/** Frames of a V8 `Error.stack`, after its message line: at most
 * `max`. */
function parseStack(stack: string, max = MAX_FRAMES): Frame[] {
  const frames: Frame[] = [];
  for (const line of stack.split('\n')) {
    const m = /^\s*at (?:(.*?) \((.*):(\d+):(\d+)\)|(.*):(\d+):(\d+))$/.exec(line);
    if (!m) continue;
    const [fn, url, l, c] = m[2] !== undefined ? [m[1], m[2], m[3], m[4]] : ['', m[5], m[6], m[7]];
    frames.push({ fn: fn || '', url: url || '', line: Number(l), column: Number(c) });
    if (frames.length >= max) break;
  }
  return frames;
}

/** Where a fiber's JSX element was written: React 18's `_debugSource`,
 * or React 19's `_debugStack` for the engine to source-map. */
function reactPlace(fiber: Any, frames = MAX_FRAMES): Place | undefined {
  const source = fiber?._debugSource;
  if (source && typeof source.fileName === 'string') {
    return { file: source.fileName, line: source.lineNumber, column: source.columnNumber };
  }
  const stack = fiber?._debugStack;
  if (stack && typeof stack.stack === 'string') {
    return { frames: parseStack(stack.stack, frames) };
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

/** An element's place in the template of the Vue component `instance`
 * that rendered it: its file, and what to find in its `<template>`. Vue
 * does not keep the element's line: the engine finds it by the
 * element's position among the elements of its tag that the same
 * component rendered. */
function vueElementPlace(el: Element, instance: Any): Place {
  const tag = el.localName;
  const same = Array.from(el.ownerDocument.getElementsByTagName(tag)).filter(
    (e) => (e as Any).__vueParentComponent === instance,
  );
  const text = (el.textContent || '').trim().slice(0, 80);
  const match: Match = { tag, index: same.indexOf(el), count: same.length, text };
  const file = typeof instance.type?.__file === 'string' ? instance.type.__file : undefined;
  return { file, match };
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
  found.at = vueElementPlace(el, instance);
  let below: Any = null;
  const seen = new Set<Any>();
  for (let at: Any = instance; at && !seen.has(at); at = at.parent) {
    seen.add(at);
    if (found.components.length >= MAX_COMPONENTS) {
      found.more++;
      continue;
    }
    const atFile = typeof at.type?.__file === 'string' ? at.type.__file : undefined;
    const place: Place = below ? { file: atFile, match: { tag: vueName(below.type) } } : found.at;
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

// ---- Components (`components [loc]`) -------------------------------

/** A prop or a piece of state, its value previewed in the page (values
 * never leave the page as objects). `hook` names a React hook's kind. */
export interface Value {
  name: string;
  value: string;
  hook?: string;
}

/** A component of the tree. Its place is in its own file: where it
 * renders its first element. `count` alike siblings are folded into
 * one; `more` components below it are past the depth asked for. */
export interface TreeNode {
  name: string;
  place?: Place;
  props: Value[];
  children: TreeNode[];
  count: number;
  more: number;
  /** A root's framework and version. */
  framework?: string;
  version?: string;
}

/** The component trees of a page, from each framework root. `more`
 * components were past the cap on how many are read. */
export interface Tree {
  frameworks: { framework: string; version: string; dev: boolean }[];
  roots: TreeNode[];
  more: number;
}

/** The component that rendered an element: its props, its state (`null`
 * with `unavailable` when the framework keeps none that can be read), and
 * its owners, innermost first, each at the place in its file that
 * renders the next one down. */
export interface Held {
  framework: 'react' | 'vue' | 'svelte';
  version: string;
  dev: boolean;
  name: string;
  place?: Place;
  props: Value[] | null;
  state: Value[] | null;
  unavailable?: string;
  owners: Component[];
  more: number;
}

/** At most this many components are read for a tree. */
const MAX_NODES = 5000;
/** A preview shows entries this deep; deeper collections are counted. */
const PREVIEW_DEPTH = 2;
/** A preview's strings are cut at this many characters (the tree's at
 * `TREE_STRING`). */
const PREVIEW_STRING = 80;
const TREE_STRING = 30;
/** Collections with more entries than this are counted, not listed. */
const PREVIEW_ENTRIES = 5;
/** The tree shows at most this many props of a component. */
const TREE_PROPS = 6;
/** A tree node's React debug stack: enough frames for its first frame of
 * the app, and alike for siblings, so they fold. */
const TREE_FRAMES = 4;

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

function isReactElement(value: Any): boolean {
  const kind = value?.$$typeof;
  return typeof kind === 'symbol' && /react\.(transitional\.)?element/.test(String(kind.description));
}

/** A value as text, JavaScript-like: entries `depth` levels deep at
 * most, strings cut at `cut` characters, collections over
 * `PREVIEW_ENTRIES` entries counted (`[12 items]`, `{…8 keys}`),
 * functions as `ƒ`. Never throws: a getter that does shows `<error>`. */
export function preview(value: Any, depth = 0, cut = PREVIEW_STRING, seen: Set<Any> = new Set()): string {
  switch (typeof value) {
    case 'string':
      return JSON.stringify(value.length > cut ? `${value.slice(0, cut)}…` : value);
    case 'number':
      return Object.is(value, -0) ? '-0' : String(value);
    case 'boolean':
      return String(value);
    case 'bigint':
      return `${value}n`;
    case 'undefined':
      return 'undefined';
    case 'symbol':
      return value.toString();
    case 'function':
      return 'ƒ';
  }
  if (value === null) return 'null';
  if (seen.has(value)) return '[circular]';
  try {
    if (isReactElement(value)) return `<${reactName(value.type)}>`;
    if (typeof Node !== 'undefined' && value instanceof Node) {
      return value instanceof Element ? `<${value.localName}>` : value.nodeName;
    }
    if (value instanceof Date) return `Date(${isNaN(value.getTime()) ? 'invalid' : value.toISOString()})`;
    if (value instanceof RegExp) return String(value);
    if (value instanceof Error) return preview(`${value.name}: ${value.message}`, depth, cut, seen);
    if (value instanceof Map) return `Map(${value.size})`;
    if (value instanceof Set) return `Set(${value.size})`;
    if (value instanceof Promise) return 'Promise';
    const array = Array.isArray(value);
    const keys = array ? [] : Object.keys(value);
    const size = array ? value.length : keys.length;
    const ctor = array ? '' : value.constructor?.name;
    const prefix = ctor && ctor !== 'Object' ? `${ctor} ` : '';
    if (size === 0) return array ? '[]' : `${prefix}{}`;
    if (size > PREVIEW_ENTRIES || depth >= PREVIEW_DEPTH) {
      return array ? `[${plural(size, 'item', 'items')}]` : `${prefix}{…${plural(size, 'key', 'keys')}}`;
    }
    seen.add(value);
    const inner = (v: () => Any) => {
      try {
        return preview(v(), depth + 1, cut, seen);
      } catch {
        return '<error>';
      }
    };
    const parts = array
      ? Array.from({ length: size }, (_, i) => inner(() => value[i]))
      : keys.map((k) => `${/^[A-Za-z_$][\w$]*$/.test(k) ? k : JSON.stringify(k)}: ${inner(() => value[k])}`);
    seen.delete(value);
    return array ? `[${parts.join(', ')}]` : `${prefix}{${parts.join(', ')}}`;
  } catch {
    return '<error>';
  }
}

/** An object's own entries as values, previewed. */
function entries(object: Any, skip: (key: string, value: Any) => boolean = () => false): Value[] {
  const out: Value[] = [];
  if (!object || typeof object !== 'object') return out;
  for (const key of Object.keys(object)) {
    let value: Any;
    try {
      value = object[key];
    } catch {
      out.push({ name: key, value: '<error>' });
      continue;
    }
    if (!skip(key, value)) out.push({ name: key, value: preview(value) });
  }
  return out;
}

/** The props a tree line shows: no functions or children, each value one
 * level deep (collections counted), at most `TREE_PROPS`. */
function treeProps(object: Any): Value[] {
  const out: Value[] = [];
  if (!object || typeof object !== 'object') return out;
  for (const key of Object.keys(object)) {
    if (key === 'children' || key === 'key' || key === 'ref') continue;
    let value: Any;
    try {
      value = object[key];
    } catch {
      continue;
    }
    if (typeof value === 'function') continue;
    if (out.length >= TREE_PROPS) {
      out.push({ name: '…', value: '' });
      break;
    }
    out.push({ name: key, value: preview(value, PREVIEW_DEPTH, TREE_STRING) });
  }
  return out;
}

function node(name: string, place: Place | undefined, props: Value[]): TreeNode {
  return { name, place, props, children: [], count: 1, more: 0 };
}

/** Counts the components below `n` (`n` itself not included). */
function below(n: TreeNode): number {
  return n.children.reduce((sum, c) => sum + c.count * (1 + below(c)) + c.more, 0);
}

/** Cuts the tree `depth` levels deep, counting what is cut, and folds
 * runs of alike siblings (same name, place, props and subtree). */
function shape(n: TreeNode, depth: number): void {
  if (depth <= 1) {
    n.more += below(n);
    n.children = [];
    return;
  }
  for (const c of n.children) shape(c, depth - 1);
  const folded: TreeNode[] = [];
  let last = '';
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

/** Every element of the document, in document order, open shadow roots
 * included. */
function allElements(doc: Document): Element[] {
  const out: Element[] = [];
  const visit = (root: Document | ShadowRoot) => {
    for (const el of Array.from(root.querySelectorAll('*'))) {
      out.push(el);
      if (el.shadowRoot) visit(el.shadowRoot);
    }
  };
  visit(doc);
  return out;
}

// React.

/** Fiber tags of components: function, class, (18's) indeterminate,
 * forwardRef and simple memo. A `memo` wrapper (14) is left out: its
 * child is the component. */
const REACT_COMPONENTS = new Set([0, 1, 2, 11, 15]);

function isReactComponent(fiber: Any): boolean {
  return REACT_COMPONENTS.has(fiber?.tag) && typeof fiber.type !== 'string';
}

/** Where a component's own JSX is: the first fiber below it that it
 * rendered (its owner is the component). */
function reactOwnPlace(fiber: Any, frames = MAX_FRAMES): Place | undefined {
  const stack: Any[] = [fiber.child];
  let seen = 0;
  while (stack.length && seen++ < 1000) {
    const f = stack.pop();
    if (!f) continue;
    if (f._debugOwner === fiber || f._debugOwner === fiber.alternate) return reactPlace(f, frames);
    stack.push(f.sibling, f.child);
  }
  return undefined;
}

function reactVersion(fiber: Any): { version: string; dev: boolean } {
  if (!fiber) return { version: '', dev: false };
  const nineteen = '_debugStack' in fiber;
  const dev = nineteen || '_debugSource' in fiber;
  return { version: nineteen ? '19' : dev ? '18' : '', dev };
}

/** The component fibers under `fiber`, host fibers flattened. */
function reactChildren(fiber: Any): Any[] {
  const out: Any[] = [];
  const visit = (f: Any, depth: number) => {
    for (; f; f = f.sibling) {
      if (isReactComponent(f)) out.push(f);
      else if (depth < 2000) visit(f.child, depth + 1);
    }
  };
  visit(fiber.child, 0);
  return out;
}

function reactTrees(elements: Element[], tree: Tree, budget: { left: number }): void {
  for (const el of elements) {
    const key = Object.keys(el).find((k) => k.startsWith('__reactContainer$'));
    if (!key) continue;
    // The container holds the first root fiber; the root's `current` is
    // the tree on screen.
    const host = (el as Any)[key];
    const root = host?.stateNode?.current ?? host;
    const tops = reactChildren(root);
    const { version, dev } = reactVersion(tops[0] ?? root.child);
    tree.frameworks.push({ framework: 'react', version, dev });
    if (!dev) continue;
    const build = (f: Any): TreeNode | undefined => {
      if (budget.left <= 0) {
        tree.more++;
        return undefined;
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
      if (n) tree.roots.push({ ...n, framework: 'react', version });
    }
  }
}

/** A React function component's state: its hooks with a queue
 * (`useState`, `useReducer`), by their index among its hooks. */
function reactState(fiber: Any): Value[] {
  if (fiber.tag === 1) return entries(fiber.stateNode?.state);
  const out: Value[] = [];
  let index = 0;
  for (let hook = fiber.memoizedState; hook && index < 100; hook = hook.next, index++) {
    const queue = hook.queue;
    if (!queue || typeof queue !== 'object' || !('pending' in queue)) continue;
    const reducer = queue.lastRenderedReducer;
    const kind = reducer && reducer.name === 'basicStateReducer' ? 'useState' : 'useReducer';
    out.push({ name: `#${index}`, value: preview(hook.memoizedState), hook: kind });
  }
  return out;
}

function reactHeld(el: Element): Held | undefined {
  const found = react(el);
  if (!found) return undefined;
  const fiber = fiberOf(el);
  // The component that rendered the element (its owner), else the one
  // around it.
  let at: Any = fiber?._debugOwner;
  if (!isReactComponent(at)) {
    at = fiber?.return;
    while (at && !isReactComponent(at)) at = at.return;
  }
  const held: Held = {
    framework: 'react',
    version: found.version,
    dev: found.dev,
    name: at ? reactName(at.type) : '',
    props: null,
    state: null,
    owners: [],
    more: found.more,
  };
  if (!found.dev || !at) return held;
  // The fiber on screen: an owner may be the other copy.
  const alternate = at.alternate;
  if (alternate && fiberIsCurrent(alternate)) at = alternate;
  held.place = reactOwnPlace(at);
  held.props = entries(at.memoizedProps);
  held.state = reactState(at);
  held.owners = found.components.slice(1);
  return held;
}

/** Whether a fiber is the copy on screen: its root's `current` tree
 * leads to it. */
function fiberIsCurrent(fiber: Any): boolean {
  let top = fiber;
  while (top.return) top = top.return;
  return top.stateNode?.current === top;
}

// Vue.

function vueComponents(vnode: Any, out: Any[], depth = 0): Any[] {
  if (!vnode || typeof vnode !== 'object' || depth > 2000) return out;
  if (vnode.component) {
    out.push(vnode.component);
  } else if (vnode.suspense) {
    vueComponents(vnode.suspense.activeBranch, out, depth + 1);
  } else if (Array.isArray(vnode.children)) {
    for (const child of vnode.children) vueComponents(child, out, depth + 1);
  }
  return out;
}

/** A Vue component's own place: its file, at its first element (not by
 * its text, which differs between instances). */
function vueOwnPlace(instance: Any): Place {
  const el = instance.subTree?.el;
  if (el instanceof Element && (el as Any).__vueParentComponent === instance) {
    const place = vueElementPlace(el, instance);
    if (place.match) delete place.match.text;
    return place;
  }
  const file = typeof instance.type?.__file === 'string' ? instance.type.__file : undefined;
  return { file };
}

function vueTrees(elements: Element[], tree: Tree, budget: { left: number }): void {
  for (const el of elements) {
    const app = (el as Any).__vue_app__;
    const top = app?._instance;
    if (!top) continue;
    const version = String(app.version || '');
    const first = top.subTree?.el;
    // A dev build marks the elements it renders with their component.
    const dev = !!first && !!(first as Any).__vueParentComponent;
    tree.frameworks.push({ framework: 'vue', version, dev });
    if (!dev) continue;
    const build = (instance: Any): TreeNode | undefined => {
      if (budget.left <= 0) {
        tree.more++;
        return undefined;
      }
      budget.left--;
      const n = node(vueName(instance.type), vueOwnPlace(instance), treeProps(instance.props));
      for (const c of vueComponents(instance.subTree, [])) {
        const child = build(c);
        if (child) n.children.push(child);
      }
      return n;
    };
    const n = build(top);
    if (n) tree.roots.push({ ...n, framework: 'vue', version });
  }
}

/** The object behind Vue's reactive and readonly proxies. */
function vueRaw(value: Any): Any {
  for (let i = 0; i < 8 && value && typeof value === 'object' && value.__v_raw; i++) value = value.__v_raw;
  return value;
}

/** Whether `value` is a component (an import `<script setup>` exposes). */
function isVueComponent(value: Any): boolean {
  return (
    !!value &&
    typeof value === 'object' &&
    ('__name' in value || '__file' in value || 'render' in value || 'setup' in value)
  );
}

function vueHeld(el: Element): Held | undefined {
  const found = vue(el);
  if (!found) return undefined;
  const held: Held = {
    framework: 'vue',
    version: found.version,
    dev: found.dev,
    name: found.components[0]?.name ?? '',
    props: null,
    state: null,
    owners: [],
    more: found.more,
  };
  const instance = (el as Any).__vueParentComponent;
  if (!found.dev || !instance) return held;
  held.place = vueOwnPlace(instance);
  held.props = entries(instance.props);
  // `setupState` unwraps refs; functions, components and the props
  // object `<script setup>` also exposes (a proxy of them) are left out.
  const props = vueRaw(instance.props);
  const state = entries(
    instance.setupState,
    (_, value) => typeof value === 'function' || vueRaw(value) === props || isVueComponent(value),
  );
  state.push(...entries(instance.data));
  held.state = state;
  held.owners = found.components.slice(1);
  return held;
}

// Svelte.

/** The innermost component entry above a Svelte element's place. */
function svelteOwner(meta: Any): Any {
  for (let entry = meta?.parent; entry; entry = entry.parent) {
    if (entry.type === 'component') return entry;
  }
  return null;
}

const SVELTE_UNAVAILABLE = 'Svelte 5 has no public API for a component\'s props or state';

function svelteTrees(elements: Element[], tree: Tree, budget: { left: number }): void {
  const version = svelteVersion();
  // Svelte keeps no tree: each element's place names the component
  // entries around it, one object per instance, so the tree is rebuilt
  // from them. A root component has no entry: it is known by its file.
  const nodes = new Map<Any, TreeNode>();
  const roots = new Map<string, TreeNode>();
  let found = false;
  const nodeOf = (entry: Any, file: string): TreeNode | undefined => {
    if (!entry) {
      let root = roots.get(file);
      if (!root) {
        root = node(svelteBase(file), undefined, []);
        roots.set(file, root);
      }
      return root;
    }
    let n = nodes.get(entry);
    if (n) return n;
    const parent = nodeOf(svelteOwner(entry), String(entry.file));
    if (!parent) return undefined;
    if (budget.left <= 0) {
      tree.more++;
      return undefined;
    }
    budget.left--;
    n = node(String(entry.componentTag || 'Anonymous'), undefined, []);
    nodes.set(entry, n);
    parent.children.push(n);
    return n;
  };
  for (const el of elements) {
    const meta = (el as Any).__svelte_meta;
    if (!meta?.loc) continue;
    found = true;
    const n = nodeOf(svelteOwner(meta), String(meta.loc.file));
    if (n && !n.place) n.place = sveltePlace(meta.loc);
  }
  if (!found) {
    if (version) tree.frameworks.push({ framework: 'svelte', version, dev: false });
    return;
  }
  tree.frameworks.push({ framework: 'svelte', version, dev: true });
  for (const root of roots.values()) tree.roots.push({ ...root, framework: 'svelte', version });
}

function svelteHeld(el: Element): Held | undefined {
  const found = svelte(el);
  if (!found) return undefined;
  const held: Held = {
    framework: 'svelte',
    version: found.version,
    dev: found.dev,
    name: found.components[0]?.name ?? '',
    props: null,
    state: null,
    unavailable: SVELTE_UNAVAILABLE,
    owners: found.components.slice(1),
    more: found.more,
  };
  const meta = (el as Any).__svelte_meta;
  if (!found.dev || !meta) return held;
  // Its place: the first element it renders.
  const owner = svelteOwner(meta);
  for (const other of allElements(el.ownerDocument)) {
    const m = (other as Any).__svelte_meta;
    if (!m?.loc) continue;
    if (owner ? svelteOwner(m) === owner : !svelteOwner(m) && m.loc.file === meta.loc?.file) {
      held.place = sveltePlace(m.loc);
      break;
    }
  }
  return held;
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

/** The page's component trees, `depth` levels deep (see `Tree`). */
export function tree(depth: number): Tree {
  const out: Tree = { frameworks: [], roots: [], more: 0 };
  const elements = allElements(document);
  const budget = { left: MAX_NODES };
  for (const adapter of [reactTrees, vueTrees, svelteTrees]) {
    try {
      adapter(elements, out, budget);
    } catch {
      // A framework's internals changed: the others still answer.
    }
  }
  const top = node('', undefined, []);
  top.children = out.roots;
  shape(top, depth + 1);
  out.roots = top.children;
  return out;
}

/** The component that rendered `el`, its props, state and owners; `null`
 * when no framework rendered it. */
export function component(el: Element): Held | null {
  let fallback: Held | null = null;
  for (const adapter of [reactHeld, vueHeld, svelteHeld]) {
    let held: Held | undefined;
    try {
      held = adapter(el);
    } catch {
      held = undefined;
    }
    if (held?.dev && held.name) return held;
    if (held && !fallback) fallback = held;
  }
  return fallback;
}
