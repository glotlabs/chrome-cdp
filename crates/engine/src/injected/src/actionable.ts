// Actionability (plan 05, milestone 5.1): what an action checks of its
// element in the page before it acts. The engine does the rest with CDP
// (scrolling, the point to act at, the hit test) and retries the whole
// pipeline until it passes or the timeout expires.

import { roleOf } from './aria';
import { contains, parentOf, tag, visible } from './dom';
import { Candidate, describe } from './locator';

/** Which checks an action needs besides visible and stable. */
export interface Needs {
  enabled: boolean;
  editable: boolean;
}

/** The first check that failed, and why; `final` when waiting cannot
 * help (the element can never be edited). */
export interface Failed {
  check: 'attached' | 'visible' | 'stable' | 'enabled' | 'editable';
  why: string;
  final?: boolean;
}

/** `<div class="cookie-banner">`: how an error names an element. */
export function outline(el: Element): string {
  const id = el.getAttribute('id');
  if (id) return `<${tag(el)} id="${cut(id, 30)}">`;
  const cls = norm(el.getAttribute('class'));
  if (cls) return `<${tag(el)} class="${cut(cls, 30)}">`;
  return `<${tag(el)}>`;
}

const norm = (s: string | null) => (s || '').replace(/\s+/g, ' ').trim();
const cut = (s: string, n: number) => (s.length > n ? s.slice(0, n - 1) + '…' : s);

/** The element or its nearest ancestor (across shadow roots) for which
 * `test` holds. */
function closest(el: Element, test: (el: Element, style: CSSStyleDeclaration) => boolean): Element | null {
  for (let a: Element | null = el; a; a = parentOf(a)) {
    if (test(a, getComputedStyle(a))) return a;
  }
  return null;
}

/** ` on <div class="modal">` when an ancestor, not the element, is why. */
const on = (el: Element, cause: Element | null) => (cause && cause !== el ? ` on ${outline(cause)}` : '');

/** Why the element is not visible, or `null` if it is: a non-empty box,
 * not `visibility: hidden`, and not transparent (`opacity: 0` on it or an
 * ancestor). */
export function invisible(el: Element): string | null {
  if (!visible(el)) {
    const box = el.getBoundingClientRect();
    if (box.width > 0 && box.height > 0) return 'it has visibility: hidden';
    const none = closest(el, (_, s) => s.display === 'none');
    if (none) return `it has display: none${on(el, none)}`;
    return `its box is empty (${Math.round(box.width)}x${Math.round(box.height)})`;
  }
  const clear = closest(el, (_, s) => s.opacity === '0');
  if (clear) return `it has opacity: 0${on(el, clear)}`;
  return null;
}

/** Form controls the `disabled` attribute (or a disabled fieldset)
 * applies to. */
const CONTROLS = new Set(['button', 'input', 'select', 'textarea', 'optgroup', 'option', 'fieldset']);

/** Why the element is disabled, or `null`. */
function disabled(el: Element): string | null {
  if (CONTROLS.has(tag(el)) && el.matches(':disabled')) {
    if (el.hasAttribute('disabled')) return 'it has the disabled attribute';
    const parent = closest(el, (a) => a !== el && a.matches('fieldset:disabled, select:disabled, optgroup:disabled'));
    if (parent) return `it is inside a disabled ${outline(parent)}`;
    return 'it is disabled';
  }
  const aria = closest(el, (a) => a.getAttribute('aria-disabled') === 'true');
  if (aria) return `it has aria-disabled="true"${on(el, aria)}`;
  return null;
}

/** Input types `fill` and `type` cannot edit. */
const NOT_TEXT = new Set(['checkbox', 'radio', 'file', 'button', 'submit', 'reset', 'image', 'hidden']);

/** Why the element cannot be edited: `final` when it is not a kind of
 * element that can be (waiting will not help). */
function readOnly(el: Element): { why: string; final?: boolean } | null {
  const name = tag(el);
  if (name === 'input') {
    const type = (el as HTMLInputElement).type;
    if (NOT_TEXT.has(type)) return { why: `it is an <input type=${type}>, which takes no text`, final: true };
  } else if (name === 'select') {
    return { why: 'it is a <select>: choose its options with select', final: true };
  } else if (name !== 'textarea' && !(el as HTMLElement).isContentEditable) {
    const role = roleOf(el);
    if (role !== 'textbox' && role !== 'searchbox' && role !== 'combobox' && role !== 'spinbutton') {
      return { why: 'it is not an input, textarea or contenteditable element', final: true };
    }
  }
  if ((name === 'input' || name === 'textarea') && (el as HTMLInputElement).readOnly) {
    return { why: 'it has the readonly attribute' };
  }
  if (el.getAttribute('aria-readonly') === 'true') return { why: 'it has aria-readonly="true"' };
  return null;
}

/** Resolves on the next animation frame, or after 100 ms where frames do
 * not run (a background tab). */
const frame = () =>
  new Promise<void>((resolve) => {
    requestAnimationFrame(() => resolve());
    setTimeout(resolve, 100);
  });

const rect = (el: Element) => {
  const r = el.getBoundingClientRect();
  return [r.x, r.y, r.width, r.height];
};

/** Runs checks 2-5 of the pipeline on `el`: visible, stable (the same box
 * on two consecutive animation frames), enabled and editable (when
 * `needs` asks). The first that fails, or `null` when all pass. */
export async function checkActionable(el: Element, needs: Needs): Promise<Failed | null> {
  if (!el.isConnected) return { check: 'attached', why: 'it was removed from the page' };
  const hidden = invisible(el);
  if (hidden) return { check: 'visible', why: hidden };
  await frame();
  const before = rect(el);
  await frame();
  const after = rect(el);
  if (!el.isConnected) return { check: 'attached', why: 'it was removed from the page' };
  if (before.some((v, i) => v !== after[i])) {
    const moved = before[0] !== after[0] || before[1] !== after[1];
    return { check: 'stable', why: moved ? 'it is still moving' : 'it is still changing size' };
  }
  if (needs.enabled || needs.editable) {
    const why = disabled(el);
    if (why) return { check: 'enabled', why };
  }
  if (needs.editable) {
    const why = readOnly(el);
    if (why) return { check: 'editable', ...why };
  }
  return null;
}

/** Buttons whose name says they close what they are in. */
const DISMISS = /^(accept|agree|allow|ok|okay|got it|close|dismiss|continue|no,? thanks|reject|decline|×|✕|x)\b/i;

/** What covers `el` at the point where `hit` was found: the highest
 * ancestor of `hit` that does not contain `el` (below `<body>`; `el` is
 * `null` when it is in another frame), and a button in it that likely
 * dismisses it. For the `Obscured` error, as `{json, els}`. */
export function covering(el: Element | null, hit: Node): { json: string; els: (Element | null)[] } {
  // The hit test can find a text node: start from its element.
  const start = hit instanceof Element ? hit : hit.parentElement;
  if (!start) return { json: '{}', els: [] };
  const doc = start.ownerDocument;
  let top = start;
  for (let a = parentOf(start); a && a !== doc.body && a !== doc.documentElement; a = parentOf(a)) {
    if (el && contains(a, el)) break;
    top = a;
  }
  // `el` itself does not receive pointer events: the hit is its ancestor,
  // which is what the pointer reaches.
  const inert = el !== null && contains(start, el);
  if (inert) top = start;
  let dismiss: Element | null = null;
  if (!inert) {
    const buttons = Array.from(top.querySelectorAll('button, [role=button], a[href], input[type=button], input[type=submit]'))
      .filter((b) => visible(b));
    dismiss = buttons.find((b) => DISMISS.test(describe(b).name)) || buttons[0] || null;
  }
  const out: { covering: Candidate; dismiss: Candidate | null; inert: boolean; pointerEvents?: string } = {
    covering: describe(top),
    dismiss: dismiss && describe(dismiss),
    inert,
  };
  if (inert && el) out.pointerEvents = getComputedStyle(el).pointerEvents;
  return { json: JSON.stringify(out), els: [top, dismiss] };
}
