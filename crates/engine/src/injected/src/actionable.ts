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

/** Why a check fails: the pipeline's phrase (`why`), and for `why` the
 * short form (`what`, with `{el}` where the element `el` that causes it
 * is named) and that element, when it is not the checked one. */
export interface Cause {
  why: string;
  what: string;
  el?: Element;
}

/** A cause set by `cause` (the element itself, an ancestor, or `null`
 * when nothing names one): `<what> on <el>` when it is an ancestor. */
function causeOn(el: Element, what: string, cause: Element | null): Cause {
  if (!cause || cause === el) return { why: `it has ${what}`, what };
  return { why: `it has ${what}${on(el, cause)}`, what: `${what} on {el}`, el: cause };
}

/** The outermost element of the unbroken chain of `el` and its ancestors
 * for which `test` holds: who sets an inherited property (`visibility`,
 * `pointer-events`). */
function setter(el: Element, test: (style: CSSStyleDeclaration) => boolean): Element {
  let top = el;
  for (let a = parentOf(el); a && test(getComputedStyle(a)); a = parentOf(a)) top = a;
  return top;
}

/** Why the element is not visible, with the element that causes it, or
 * `null` if it is: a non-empty box, not `visibility: hidden`, not in a
 * `content-visibility: hidden` subtree, and not transparent (`opacity: 0`
 * on it or an ancestor). */
export function invisibleCause(el: Element): Cause | null {
  if (!visible(el)) {
    const box = el.getBoundingClientRect();
    if (box.width > 0 && box.height > 0) {
      const hidden = (s: CSSStyleDeclaration) => s.visibility === 'hidden' || s.visibility === 'collapse';
      return causeOn(el, 'visibility: hidden', setter(el, hidden));
    }
    const none = closest(el, (_, s) => s.display === 'none');
    if (none) return causeOn(el, 'display: none', none);
    const skipped = closest(el, (a, s) => a !== el && s.contentVisibility === 'hidden');
    if (skipped) {
      return {
        why: `it is inside ${outline(skipped)}, which has content-visibility: hidden`,
        what: 'content-visibility: hidden on {el}',
        el: skipped,
      };
    }
    const what = `an empty box (${Math.round(box.width)}x${Math.round(box.height)})`;
    return { why: `its box is empty (${Math.round(box.width)}x${Math.round(box.height)})`, what };
  }
  const clear = closest(el, (_, s) => s.opacity === '0');
  if (clear) return causeOn(el, 'opacity: 0', clear);
  return null;
}

/** Why the element is not visible, or `null` if it is (see
 * `invisibleCause`). */
export function invisible(el: Element): string | null {
  return invisibleCause(el)?.why ?? null;
}

/** Form controls the `disabled` attribute (or a disabled fieldset)
 * applies to. */
const CONTROLS = new Set(['button', 'input', 'select', 'textarea', 'optgroup', 'option', 'fieldset']);

/** Why the element is disabled, with the element that causes it, or
 * `null`: its `disabled`, a disabled `<fieldset>` (or `<select>`,
 * `<optgroup>`) it is in, or `aria-disabled="true"` on it or an
 * ancestor. */
export function disabledCause(el: Element): Cause | null {
  if (CONTROLS.has(tag(el)) && el.matches(':disabled')) {
    if (el.hasAttribute('disabled')) return { why: 'it has the disabled attribute', what: 'the disabled attribute' };
    const parent = closest(el, (a) => a !== el && a.matches('fieldset:disabled, select:disabled, optgroup:disabled'));
    if (parent) return { why: `it is inside a disabled ${outline(parent)}`, what: 'inside the disabled {el}', el: parent };
    return { why: 'it is disabled', what: 'disabled' };
  }
  const aria = closest(el, (a) => a.getAttribute('aria-disabled') === 'true');
  if (aria) return causeOn(el, 'aria-disabled="true"', aria);
  return null;
}

/** Why the element is disabled, or `null` (see `disabledCause`). */
export function disabled(el: Element): string | null {
  return disabledCause(el)?.why ?? null;
}

/** Why the element is inert, or `null`: `inert` on it or an ancestor, or
 * a modal `<dialog>` open elsewhere (which makes the rest of the page
 * inert). Actions that need the element enabled check this too: an inert
 * element takes no clicks, focus or text. */
export function inertCause(el: Element): Cause | null {
  const inert = closest(el, (a) => a.hasAttribute('inert'));
  if (inert) return causeOn(el, 'inert', inert);
  const doc = el.ownerDocument;
  const modals = Array.from(doc.querySelectorAll('dialog')).filter((d) => d.matches(':modal'));
  if (modals.length && !modals.some((m) => contains(m, el))) {
    const modal = modals[modals.length - 1];
    return {
      why: `it is inert while the modal ${outline(modal)} is open`,
      what: 'inert while the modal {el} is open',
      el: modal,
    };
  }
  return null;
}

/** Why the element does not take pointer events, or `null`:
 * `pointer-events: none` on it or the ancestor that sets it. */
export function pointerCause(el: Element): Cause | null {
  if (getComputedStyle(el).pointerEvents !== 'none') return null;
  return causeOn(el, 'pointer-events: none', setter(el, (s) => s.pointerEvents === 'none'));
}

/** Input types `fill` and `type` cannot edit. */
const NOT_TEXT = new Set(['checkbox', 'radio', 'file', 'button', 'submit', 'reset', 'image', 'hidden']);

/** Why the element cannot be edited: `final` when it is not a kind of
 * element that can be (waiting will not help). */
export function readOnly(el: Element): { why: string; final?: boolean } | null {
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
export const frame = () =>
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
    const why = disabled(el) || inertCause(el)?.why;
    if (why) return { check: 'enabled', why };
  }
  if (needs.editable) {
    const why = readOnly(el);
    if (why) return { check: 'editable', ...why };
  }
  return null;
}

/** How `why` names an element: `describe`'s role, name and tag, and its
 * first class, which names an element without a role
 * (`div.cookie-banner`). */
export interface Named extends Candidate {
  cls?: string;
}

export function named(el: Element): Named {
  const out: Named = describe(el);
  const cls = norm(el.getAttribute('class')).split(' ')[0];
  if (cls) out.cls = cut(cls, 30);
  return out;
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
  const out: { covering: Named; dismiss: Named | null; inert: boolean; pointerEvents?: string } = {
    covering: named(top),
    dismiss: dismiss && named(dismiss),
    inert,
  };
  if (inert && el) out.pointerEvents = getComputedStyle(el).pointerEvents;
  return { json: JSON.stringify(out), els: [top, dismiss] };
}
