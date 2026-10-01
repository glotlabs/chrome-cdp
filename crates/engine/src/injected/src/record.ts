// The recorder (plan 18, milestone 18.6): `record start` puts a call to
// `record` in chrome-cdp's isolated world of every document, which
// listens in the capture phase, before the page's own listeners, and
// reports what a user (or anything that sends input) does through a
// binding: each report is one JSON object with `k`, what happened. The
// element's locator is taken right then (`stable`), before the page
// reacts. The daemon turns the reports into `.cdp` lines (it coalesces
// typing, and adds `frame=`, tabs, navigations and dialogs); see
// `crates/engine/src/record`.

import { roleOf } from './aria';
import { parentOf } from './dom';
import { stable } from './stable';

/** The recorder of this document: one, whatever calls `record`. */
const KEY = Symbol.for('chrome-cdp.record');

interface Recorder {
  on: boolean;
  binding: string;
  testid: string;
  short: boolean;
}

declare global {
  interface Window {
    [KEY]?: Recorder;
  }
}

/** Roles a click on something inside acts on. */
const ACTS = new Set(['button', 'link', 'checkbox', 'radio', 'switch', 'combobox', 'option', 'menuitem',
  'menuitemcheckbox', 'menuitemradio', 'tab', 'treeitem', 'textbox', 'searchbox', 'slider', 'spinbutton',
  'listbox', 'gridcell', 'row', 'cell']);

/** Input types that take typed text. */
const TEXT = new Set(['', 'email', 'number', 'password', 'search', 'tel', 'text', 'url']);

/** Input types whose value is set, not typed (`fill` sets them too). */
const SET = new Set(['color', 'date', 'time', 'datetime-local', 'month', 'range', 'week']);

/** Keys that type nothing on their own. */
const MODIFIERS = new Set(['Shift', 'Control', 'Alt', 'Meta', 'AltGraph', 'CapsLock', 'Fn', 'Dead', 'Process',
  'Unidentified']);

const isInput = (el: Element): el is HTMLInputElement => el instanceof HTMLInputElement;

/** Whether `el` takes typed text: what `fill` fills. */
function typedInto(el: Element): boolean {
  if (isInput(el)) return TEXT.has(el.type.toLowerCase());
  return el instanceof HTMLTextAreaElement || (el as HTMLElement).isContentEditable === true;
}

/** Whether Enter in `el` types a line break. */
const multiline = (el: Element) => el instanceof HTMLTextAreaElement || (el as HTMLElement).isContentEditable === true;

/** The element an event happened to, inside open shadow roots. */
function targetOf(e: Event): Element | null {
  const first = e.composedPath()[0];
  if (first instanceof Element) return first;
  return first instanceof Node ? first.parentElement : null;
}

/** The element a click on `el` acts on: `el`, or the nearest one around
 * it with a role that acts. */
function actor(el: Element): Element {
  for (let a: Element | null = el; a; a = parentOf(a)) {
    const role = roleOf(a);
    if (role && ACTS.has(role)) return a;
    if (a instanceof HTMLLabelElement) return a;
  }
  return el;
}

/** `{loc, how}` of `el`, or `{reason}` when no locator finds it alone. */
function locate(r: Recorder, el: Element): { loc?: string; how?: string; reason?: string } {
  try {
    const found = JSON.parse(stable(el, r.testid, false, r.short));
    return found.locator ? { loc: found.locator, how: found.how } : { reason: found.reason };
  } catch (e) {
    return { reason: String(e) };
  }
}

/** The key as `press` names it, with its modifiers: `Control+a`. */
function keyName(e: KeyboardEvent): string {
  const key = e.key === ' ' ? 'Space' : e.key === '+' ? 'Equal' : e.key;
  const mods = [e.ctrlKey && 'Control', e.altKey && 'Alt', e.metaKey && 'Meta'].filter(Boolean) as string[];
  // Shift is in the key already for a character (`A`), not for a named key.
  if (e.shiftKey && (key.length > 1 || mods.length > 0)) mods.push('Shift');
  return [...mods, key].join('+');
}

/** Starts recording in this document (again, after `recordOff`), with
 * `binding` to report on and the project's test id attribute (`short`:
 * `testid=` for any attribute, as `stable` takes it). */
export function record(binding: string, testid: string, short: boolean): void {
  const existing = window[KEY];
  if (existing) {
    Object.assign(existing, { on: true, binding, testid, short });
    return;
  }
  const r: Recorder = { on: true, binding, testid, short };
  window[KEY] = r;
  const send = (msg: Record<string, unknown>) => {
    if (!r.on) return;
    const call = (globalThis as unknown as Record<string, unknown>)[r.binding];
    if (typeof call !== 'function') return;
    try {
      (call as (s: string) => void)(JSON.stringify(msg));
    } catch {
      // The binding went away (recording stopped).
    }
  };
  // The last key pressed: a click it makes (Enter or Space on a button)
  // is the press, not a click of its own.
  let lastKey: { key: string; el: Element | null; at: number } | null = null;
  const listen = (type: string, fn: (e: Event) => void) =>
    window.addEventListener(type, (e) => {
      if (!r.on) return;
      try {
        fn(e);
      } catch {
        // Never break the page.
      }
    }, true);

  listen('click', (e) => {
    const event = e as MouseEvent;
    if (!event.isTrusted || event.button !== 0) return;
    const target = targetOf(event);
    if (!target) return;
    if (event.detail === 0 && lastKey && performance.now() - lastKey.at < 1000 &&
        (lastKey.key === 'Enter' || lastKey.key === ' ')) {
      return;
    }
    const el = actor(target);
    if (el instanceof HTMLLabelElement) {
      // A label passes the click on to its control, which reports it.
      const control = el.control;
      if (control && control !== target && !control.contains(target)) return;
    }
    if (el instanceof HTMLSelectElement || el instanceof HTMLOptionElement) return;
    if (isInput(el) && el.type.toLowerCase() === 'file') return;
    if (isInput(el) && SET.has(el.type.toLowerCase())) return;
    const role = roleOf(el);
    if (isInput(el) && (el.type === 'checkbox' || el.type === 'radio')) {
      // The click has toggled it already.
      send({ k: 'check', checked: el.checked, ...locate(r, el) });
      return;
    }
    if (role === 'checkbox' || role === 'radio' || role === 'switch' || role === 'menuitemcheckbox') {
      // The page toggles it after this listener.
      const was = el.getAttribute('aria-checked') === 'true';
      send({ k: 'check', checked: role === 'radio' ? true : !was, ...locate(r, el) });
      return;
    }
    send({ k: 'click', ...locate(r, el) });
  });

  listen('dblclick', (e) => {
    if (!e.isTrusted) return;
    const target = targetOf(e);
    if (!target) return;
    const el = actor(target);
    send({ k: 'dblclick', ...locate(r, el) });
  });

  listen('input', (e) => {
    const el = targetOf(e);
    if (!el) return;
    if (isInput(el) && SET.has(el.type.toLowerCase())) return; // `change` reports it
    if (!e.isTrusted || !typedInto(el)) return;
    if (isInput(el) && el.type.toLowerCase() === 'password') {
      // Never the value.
      send({ k: 'input', secret: true, ...locate(r, el) });
      return;
    }
    const value = isInput(el) || el instanceof HTMLTextAreaElement
      ? el.value
      : (el as HTMLElement).innerText.replace(/\n$/, '');
    send({ k: 'input', value, ...locate(r, el) });
  });

  listen('change', (e) => {
    const el = targetOf(e);
    if (!el) return;
    if (el instanceof HTMLSelectElement) {
      const values = Array.from(el.selectedOptions).map((o) => o.value);
      send({ k: 'select', values, ...locate(r, el) });
    } else if (isInput(el) && el.type.toLowerCase() === 'file') {
      const names = Array.from(el.files || []).map((f) => f.name);
      send({ k: 'upload', names, ...locate(r, el) });
    } else if (isInput(el) && SET.has(el.type.toLowerCase())) {
      send({ k: 'input', value: el.value, ...locate(r, el) });
    }
  });

  listen('keydown', (e) => {
    const event = e as KeyboardEvent;
    if (!event.isTrusted || event.isComposing || event.repeat || MODIFIERS.has(event.key)) return;
    const el = targetOf(event);
    lastKey = { key: event.key, el, at: performance.now() };
    const shortcut = event.ctrlKey || event.metaKey || event.altKey;
    if (el && typedInto(el)) {
      // Typing reports itself as `input`; only keys that do something
      // else count.
      const enter = event.key === 'Enter' && (!multiline(el) || shortcut);
      if (!enter && event.key !== 'Escape' && event.key !== 'Tab') return;
    } else if (el && (el instanceof HTMLSelectElement || (isInput(el) && SET.has(el.type.toLowerCase())))) {
      // `change` reports what the keys chose.
      if (event.key !== 'Enter' && event.key !== 'Escape' && event.key !== 'Tab') return;
    }
    // Where the key went, unless nothing has focus.
    const at = el && el !== document.body && el !== document.documentElement ? locate(r, el) : {};
    send({ k: 'press', key: keyName(event), ...at });
  });

  // Where the document is in the tab's history, for back and forward.
  const page = () => {
    if (window.top !== window) return;
    const nav = (globalThis as unknown as { navigation?: { currentEntry?: { index: number } } }).navigation;
    send({ k: 'page', url: location.href, index: nav?.currentEntry?.index ?? -1 });
  };
  page();
  window.addEventListener('pageshow', (e) => {
    if ((e as PageTransitionEvent).persisted && r.on) page();
  });
  const navigation = (globalThis as unknown as { navigation?: EventTarget }).navigation;
  navigation?.addEventListener('currententrychange', () => {
    if (r.on) page();
  });
}

/** Stops recording in this document. */
export function recordOff(): void {
  const r = window[KEY];
  if (r) r.on = false;
}
