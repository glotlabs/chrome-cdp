// `expect` (plan 09, milestone 9.1): what one check of an assertion sees
// of the elements a locator matched, or of the page. The engine's probe
// (`wait/probe.js`) runs it on every animation frame until it passes (or,
// with `--not`, fails) or the time is up, like Playwright's web-first
// assertions.

import { disabled, invisible, readOnly } from './actionable';
import { roleOf } from './aria';
import { norm, visible } from './dom';
import { focused } from './form';
import { describe } from './locator';
import { text as shownText, value as fieldValue } from './read';

/** A text or value to compare with: a part of it (case counts), all of
 * it, or a regex (`/re/flags`). */
export interface Pattern {
  mode: 'substring' | 'exact' | 'regex';
  value: string;
  flags?: string;
}

/** One assertion's check, as the engine sends it (`Check` in
 * `expect/mod.rs`). */
export interface Spec {
  matcher: string;
  pattern?: Pattern;
  values?: string[];
  name?: string;
  count?: { eq?: number | null; min?: number | null; max?: number | null };
}

/** What one check saw. `pass` is whether the matcher holds; `invalid`
 * says why it can hold neither way (no element, several, not a field).
 * `actual` is the value compared (text, a count, …), `state` the state
 * in words. */
export interface Seen {
  pass: boolean;
  invalid?: string;
  actual?: unknown;
  state?: string;
  count: number;
}

/** An error in the assertion itself (a bad regex): no use retrying. */
export class ArgError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ArgError';
  }
}

function test(p: Pattern): (s: string) => boolean {
  if (p.mode === 'regex') {
    let re: RegExp;
    try {
      re = new RegExp(p.value, (p.flags || '').replace(/[gy]/g, ''));
    } catch (e) {
      throw new ArgError(`/${p.value}/${p.flags || ''} is not a valid regex: ${(e as Error).message}`);
    }
    return (s) => re.test(s);
  }
  const want = norm(p.value);
  if (p.mode === 'exact') return (s) => norm(s) === want;
  return (s) => norm(s).includes(want);
}

/** Longer values are cut in what a check reports. */
const SHOWN = 300;
const cut = (s: string) => (s.length > SHOWN ? s.slice(0, SHOWN - 1) + '…' : s);

/** The element's text: its rendered text, or, when it is not rendered,
 * its text content. */
function textOf(el: Element): string {
  return visible(el) ? norm(shownText(el)) : norm(el.textContent);
}

const CHECKABLE = new Set(['checkbox', 'radio', 'switch', 'menuitemcheckbox', 'menuitemradio']);

/** `true`, `false` or `'mixed'`; `null` when it is no checkbox, radio
 * or switch. A <label> answers for its control. */
function checked(el: Element): boolean | 'mixed' | null {
  const control = el instanceof HTMLLabelElement && el.control ? el.control : el;
  if (control instanceof HTMLInputElement && (control.type === 'checkbox' || control.type === 'radio')) {
    return control.indeterminate && control.type === 'checkbox' ? 'mixed' : control.checked;
  }
  if (!CHECKABLE.has(roleOf(control) || '')) return null;
  const aria = control.getAttribute('aria-checked');
  return aria === 'mixed' ? 'mixed' : aria === 'true';
}

/** How much of the element is in the viewport (0 to 1), as an
 * IntersectionObserver sees it (clipping by scroll containers counts).
 * Where no frame runs (a background tab) the box is compared with the
 * viewport instead. */
function inViewport(el: Element): Promise<number> {
  return new Promise((resolve) => {
    const observer = new IntersectionObserver((entries) => {
      observer.disconnect();
      resolve(entries[entries.length - 1].intersectionRatio);
    });
    observer.observe(el);
    setTimeout(() => {
      observer.disconnect();
      const r = el.getBoundingClientRect();
      const view = el.ownerDocument.defaultView as Window;
      const w = Math.max(0, Math.min(r.right, view.innerWidth) - Math.max(r.left, 0));
      const h = Math.max(0, Math.min(r.bottom, view.innerHeight) - Math.max(r.top, 0));
      resolve(r.width * r.height > 0 ? (w * h) / (r.width * r.height) : 0);
    }, 200);
  });
}

let canvas: CanvasRenderingContext2D | null = null;

/** A CSS colour as canvas writes it (`#ff0000`), or `null` if `s` is no
 * colour: `red`, `#f00` and `rgb(255, 0, 0)` compare equal. */
function colour(s: string): string | null {
  canvas = canvas || document.createElement('canvas').getContext('2d');
  if (!canvas) return null;
  canvas.fillStyle = '#010203';
  canvas.fillStyle = s;
  const first = canvas.fillStyle;
  canvas.fillStyle = '#040506';
  canvas.fillStyle = s;
  return first === canvas.fillStyle ? String(first) : null;
}

/** Whether a computed CSS value is what was asked for: the same text,
 * the same colour, or a regex match. */
function cssMatches(actual: string, p: Pattern): boolean {
  if (p.mode === 'regex') return test(p)(actual);
  if (norm(actual) === norm(p.value)) return true;
  const a = colour(actual);
  return a !== null && a === colour(p.value);
}

/** Checks that need exactly one element. */
const ONE = new Set(['visible', 'enabled', 'disabled', 'editable', 'checked', 'unchecked', 'focused',
  'text', 'value', 'values', 'attr', 'class', 'css', 'in-viewport', 'hidden']);

/** Runs `spec` on `els`, what the locator matched now. */
export async function expectElements(spec: Spec, els: Element[]): Promise<Seen> {
  const count = els.length;
  const m = spec.matcher;
  if (m === 'attached') return { pass: count > 0, actual: count, count };
  if (m === 'detached') return { pass: count === 0, actual: count, count };
  if (m === 'count') {
    const c = spec.count || {};
    // Bounds not given are null.
    const pass = (c.eq == null || count === c.eq)
      && (c.min == null || count >= c.min)
      && (c.max == null || count <= c.max);
    return { pass, actual: count, count };
  }
  if (ONE.has(m) && count > 1) {
    return { pass: false, invalid: `${count} elements match`, count };
  }
  if (count === 0) {
    if (m === 'visible') return { pass: false, state: 'no element matches', count };
    if (m === 'hidden') return { pass: true, state: 'no element matches', count };
    return { pass: false, invalid: 'no element matches', count };
  }
  const el = els[0];
  const one = (pass: boolean, state: string, actual?: unknown): Seen => ({ pass, state, actual, count });
  switch (m) {
    case 'visible':
    case 'hidden': {
      const shown = visible(el);
      const why = shown ? null : invisible(el);
      const state = shown ? 'visible' : `hidden${why ? ` (${why})` : ''}`;
      return one(m === 'visible' ? shown : !shown, state);
    }
    case 'enabled':
    case 'disabled': {
      const why = disabled(el);
      return one(m === 'enabled' ? !why : !!why, why ? `disabled (${why})` : 'enabled');
    }
    case 'editable': {
      const why = disabled(el) || readOnly(el)?.why || null;
      return one(!why, why ? `not editable (${why})` : 'editable');
    }
    case 'checked':
    case 'unchecked': {
      const state = checked(el);
      if (state === null) {
        return { pass: false, invalid: `it is ${roleOf(el) || 'an element'}, not a checkbox, radio or switch`, count };
      }
      const words = state === 'mixed' ? 'mixed' : state ? 'checked' : 'unchecked';
      return one(m === 'checked' ? state === true : state === false, words);
    }
    case 'focused': {
      const f = focused(el);
      if (f === el) return one(true, 'focused');
      if (!f) return one(false, 'not focused (nothing has focus)');
      const d = describe(f);
      return one(false, `not focused (${d.role}${d.name ? ` "${d.name}"` : ''} has focus)`);
    }
    case 'in-viewport': {
      const ratio = await inViewport(el);
      return one(ratio > 0, ratio > 0 ? `in the viewport (${Math.round(ratio * 100)}% of it)` : 'outside the viewport', ratio);
    }
    case 'text': {
      const actual = textOf(el);
      return one(test(spec.pattern as Pattern)(actual), '', cut(actual));
    }
    case 'value':
    case 'values': {
      const read = fieldValue(el);
      if (read.error) return { pass: false, invalid: read.error, count };
      if (m === 'value') {
        const actual = Array.isArray(read.value) ? read.value.join(', ') : read.value || '';
        return one(test(spec.pattern as Pattern)(actual), '', cut(actual));
      }
      const actual = Array.isArray(read.value) ? read.value : [read.value || ''];
      const want = [...(spec.values || [])].sort();
      const have = [...actual].sort();
      return one(want.length === have.length && want.every((v, i) => v === have[i]), '', actual);
    }
    case 'attr': {
      const actual = el.getAttribute(spec.name as string);
      if (!spec.pattern) return one(actual !== null, actual === null ? 'no such attribute' : '', actual);
      return one(actual !== null && test(spec.pattern)(actual), actual === null ? 'no such attribute' : '', actual === null ? null : cut(actual));
    }
    case 'class': {
      const actual = norm(el.getAttribute('class'));
      if (spec.pattern) return one(test(spec.pattern)(actual), '', cut(actual));
      const want = norm(spec.name).split(' ');
      return one(want.every((c) => el.classList.contains(c)), '', cut(actual));
    }
    case 'css': {
      const style = (el.ownerDocument.defaultView as Window).getComputedStyle(el);
      const actual = style.getPropertyValue(spec.name as string);
      return one(cssMatches(actual, spec.pattern as Pattern), '', cut(actual));
    }
    default:
      throw new ArgError(`unknown matcher ${m}`);
  }
}

/** Runs a page check of `spec` (`title`) on the document. */
export function expectPage(spec: Spec): Seen {
  if (spec.matcher === 'title') {
    const actual = document.title;
    return { pass: test(spec.pattern as Pattern)(actual), actual: cut(actual), count: 0 };
  }
  throw new ArgError(`unknown page check ${spec.matcher}`);
}

/** The element an `expect` failure names: the first match, described,
 * as `{json, els}` so the engine can give it a ref. */
export function expectNamed(els: Element[]): { json: string; els: Element[] } {
  const shown = els.slice(0, 3);
  return { json: JSON.stringify(shown.map(describe)), els: shown };
}
