// Keyboard and form actions (plan 05, milestone 5.3): what `press`,
// `type`, `fill`, `select`, `check`, `focus` and `blur` ask of the page.
// Text and keys themselves are real input (Input.insertText and
// Input.dispatchKeyEvent), sent by the engine; only what takes no typing
// (a date, a colour, a range, a <select>) is set here.

import { invisible } from './actionable';
import { roleOf } from './aria';
import { contains, norm, parentOf, visible } from './dom';
import { Candidate, describe } from './locator';

/** The element that has focus in `el`'s document, inside shadow roots;
 * `null` when nothing (the body) has it. */
export function focused(el?: Node): Element | null {
  const doc = el ? el.ownerDocument || (el as Document) : document;
  let f = doc.activeElement;
  while (f && f.shadowRoot && f.shadowRoot.activeElement) f = f.shadowRoot.activeElement;
  return f && f !== doc.body && f !== doc.documentElement ? f : null;
}

/** Whether `el` has focus (or something in it, for a shadow host that
 * delegates focus). */
export function hasFocus(el: Element): boolean {
  const f = focused(el);
  return !!f && (f === el || contains(el, f));
}

/** Puts the caret at the end of `el`'s text (for `type --in` on an
 * element it just focused, as a click after the text would). */
export function caretToEnd(el: Element): void {
  if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) {
    try {
      const end = el.value.length;
      el.setSelectionRange(end, end);
    } catch {
      // Types without a selection (email, number) keep theirs.
    }
    return;
  }
  if ((el as HTMLElement).isContentEditable) {
    const range = el.ownerDocument.createRange();
    range.selectNodeContents(el);
    range.collapse(false);
    const selection = el.ownerDocument.getSelection();
    selection?.removeAllRanges();
    selection?.addRange(range);
  }
}

/** Input types whose value is set, not typed: they take no text input. */
const SET_VALUE = new Set(['color', 'date', 'time', 'datetime-local', 'month', 'range', 'week']);

/** Input types that take typed text. */
const TYPE_INTO = new Set(['', 'email', 'number', 'password', 'search', 'tel', 'text', 'url']);

/** What a value of each set-value type looks like, for errors. */
const EXAMPLES: Record<string, string> = {
  color: '#ff0000',
  date: '2026-09-27',
  time: '13:45',
  'datetime-local': '2026-09-27T13:45',
  month: '2026-09',
  week: '2026-W39',
};

const fire = (el: Element) => {
  el.dispatchEvent(new Event('input', { bubbles: true, composed: true }));
  el.dispatchEvent(new Event('change', { bubbles: true }));
};

/** Gets `el` ready for `fill value`. `{set: true}` when the value was
 * set here (types that take no typing), with `input` and `change`
 * dispatched; `{set: false}` once `el` has focus and its content is
 * selected, so inserted text replaces it; `{error}` when it cannot hold
 * `value`. */
export function fillPrepare(el: Element, value: string): { set?: boolean; error?: string; example?: string } {
  if (el instanceof HTMLInputElement) {
    const type = el.type.toLowerCase();
    if (!TYPE_INTO.has(type) && !SET_VALUE.has(type)) {
      return { error: `it is an <input type=${type}>, which takes no text` };
    }
    if (type === 'number' && value.trim() !== '' && Number.isNaN(Number(value.trim()))) {
      return { error: `it is an <input type=number>, and "${value}" is not a number`, example: el.min || '42' };
    }
    if (SET_VALUE.has(type)) {
      const want = value.trim();
      el.focus();
      el.value = want;
      if (el.value !== want) {
        const range = type === 'range';
        const like = range
          ? `a number from ${el.min || '0'} to ${el.max || '100'}${el.step && el.step !== '1' ? ` in steps of ${el.step}` : ''}`
          : `a value like ${EXAMPLES[type]}`;
        const example = range ? el.min || '0' : EXAMPLES[type];
        return { error: `it is an <input type=${type}>, which takes ${like}, not "${value}"`, example };
      }
      fire(el);
      return { set: true };
    }
    el.focus();
    el.select();
    return { set: false };
  }
  if (el instanceof HTMLTextAreaElement) {
    el.focus();
    el.select();
    return { set: false };
  }
  if (!(el as HTMLElement).isContentEditable) {
    return { error: 'it is not an input, textarea or contenteditable element' };
  }
  const range = el.ownerDocument.createRange();
  range.selectNodeContents(el);
  const selection = el.ownerDocument.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
  (el as HTMLElement).focus();
  return { set: false };
}

/** What a field holds now, as `fill` compares it: `{value, secret}`. */
export function fieldValue(el: Element): { value: string; secret: boolean } {
  if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) {
    return { value: el.value, secret: el instanceof HTMLInputElement && el.type === 'password' };
  }
  return { value: (el as HTMLElement).innerText.replace(/\n$/, ''), secret: false };
}

/** An option of a <select>, as `select` reports it. */
interface Option {
  value: string;
  label: string;
  disabled?: boolean;
}

const option = (o: HTMLOptionElement): Option => ({ value: o.value, label: norm(o.label) });

/** How many options an error lists. */
const LISTED = 20;

/** Selects the options of the <select> `el` that `values` name (each by
 * value, else by label, else by label in any case), and dispatches
 * `input` and `change`. Answers `{selected}`, or why not: `{notSelect,
 * role}` (not a <select>), `{missing, options}` (no option matches one;
 * the first few listed), `{disabled}` (the option is disabled), or
 * `{single}` (several values for a single <select>). */
export function selectOptions(el: Element, values: string[]): string {
  if (!(el instanceof HTMLSelectElement)) {
    return JSON.stringify({ notSelect: true, role: roleOf(el) || '' });
  }
  const options = Array.from(el.options);
  const picked: HTMLOptionElement[] = [];
  for (const v of values) {
    const wanted = norm(v);
    const found =
      options.find((o) => o.value === v) ||
      options.find((o) => norm(o.label) === wanted) ||
      options.find((o) => norm(o.label).toLowerCase() === wanted.toLowerCase());
    if (!found) {
      const listed = options.slice(0, LISTED).map((o) => ({ ...option(o), disabled: o.disabled || undefined }));
      return JSON.stringify({ missing: v, options: listed, more: Math.max(0, options.length - LISTED) });
    }
    if (found.disabled || (found.parentElement instanceof HTMLOptGroupElement && found.parentElement.disabled)) {
      return JSON.stringify({ disabled: v });
    }
    if (!picked.includes(found)) picked.push(found);
  }
  if (!el.multiple && picked.length > 1) return JSON.stringify({ single: picked.length });
  for (const o of options) o.selected = picked.includes(o);
  fire(el);
  return JSON.stringify({ selected: picked.map(option) });
}

/** Roles whose state is `aria-checked`. */
const CHECKABLE = new Set(['checkbox', 'radio', 'switch', 'menuitemcheckbox', 'menuitemradio']);

/** Whether a native input is hidden from the eye, not from the page:
 * transparent, clipped, or a pixel or less (styled as its label). */
function visuallyHidden(el: Element): boolean {
  if (invisible(el)) return true;
  const box = el.getBoundingClientRect();
  if (box.width <= 1 || box.height <= 1) return true;
  const s = getComputedStyle(el);
  return (s.clip !== 'auto' && s.clip !== '') || (s.clipPath !== 'none' && s.clipPath !== '');
}

/** The state `check`/`uncheck` read, as `{json, els: [control, click]}`:
 * the control is `el` or, for a <label>, the one it labels; `json` has
 * its `kind` (`native`, `aria`, or `null` when it is no checkbox, radio
 * or switch), its `role`, its `state` (`true`, `false`, `"mixed"`), and
 * `candidate` (how the output names it). `click` is what to click: the
 * control, or its label when the control is visually hidden (a native
 * input styled away behind its label) or `el` is that label. */
export function checkState(el: Element): { json: string; els: (Element | null)[] } {
  let control: Element = el;
  if (el instanceof HTMLLabelElement && el.control) control = el.control;
  let kind: 'native' | 'aria' | null = null;
  let state: boolean | 'mixed' = false;
  const role = roleOf(control) || '';
  if (control instanceof HTMLInputElement && (control.type === 'checkbox' || control.type === 'radio')) {
    kind = 'native';
    state = control.indeterminate && control.type === 'checkbox' ? 'mixed' : control.checked;
  } else if (CHECKABLE.has(role)) {
    kind = 'aria';
    const aria = control.getAttribute('aria-checked');
    state = aria === 'mixed' ? 'mixed' : aria === 'true';
  }
  let click: Element | null = control;
  if (el !== control) {
    click = el;
  } else if (kind === 'native' && visuallyHidden(control)) {
    const labels = Array.from((control as HTMLInputElement).labels || []);
    click = labels.find((l) => visible(l) && !invisible(l)) || control;
  }
  const radio = role === 'radio' || role === 'menuitemradio' || (control as HTMLInputElement).type === 'radio';
  const candidate: Candidate = describe(control);
  const disabled = control.matches(':disabled') || control.getAttribute('aria-disabled') === 'true';
  const out = { kind, state, radio, candidate, disabled, click: click && click !== control ? describe(click) : null };
  return { json: JSON.stringify(out), els: [control, click] };
}

/** Whether a click at `hit` reaches `el`: `hit` is inside it, or inside a
 * <label> of it (a click on a label clicks its control). */
export function reaches(el: Element, hit: Node): boolean {
  if (contains(el, hit)) return true;
  for (let a: Element | null = hit instanceof Element ? hit : parentOf(hit); a; a = parentOf(a)) {
    if (a instanceof HTMLLabelElement) return a.control === el;
  }
  return false;
}
