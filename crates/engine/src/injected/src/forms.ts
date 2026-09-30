// Forms (plan 16, milestone 16.4): the forms of a document as units, with
// each field's name, kind, state and validation message, for `forms`,
// and the options `fill-form` picks in custom comboboxes and listboxes.

import { named, Named } from './actionable';
import { byIds, hiddenForAria, nameOf, roleOf } from './aria';
import { attr, contains, descendants, norm, parentOf, tag, visible } from './dom';

/** Roles whose elements are forms. */
const UNITS = new Set(['form', 'search']);

/** Roles a custom widget counts as a field by. */
const WIDGETS = new Set(['combobox', 'listbox', 'checkbox', 'switch', 'radiogroup', 'textbox', 'searchbox',
  'slider', 'spinbutton', 'radio']);

/** Custom widgets whose text an `<input>` inside takes (ARIA 1.1's
 * combobox wraps its input). */
const WRAPS_INPUT = new Set(['combobox', 'textbox', 'searchbox']);

/** How many options a field lists. */
const OPTIONS = 10;

/** How long a value is shown. */
const VALUE = 80;

const cut = (s: string, n: number) => (s.length > n ? s.slice(0, n - 1) + '…' : s);

/** A radio button of a group. */
interface Radio {
  el: number;
  name: string;
  value?: string;
  checked: boolean;
  disabled?: true;
}

/** A field, as `forms` lists it. `el` indexes `names`. */
interface Field {
  el: number;
  /** An input's type (`email`, `file`), `select`, `textarea`,
   * `contenteditable`, a button's type, or a custom widget's role. */
  kind: string;
  role: string;
  /** Not a native control: a custom widget. */
  custom?: true;
  required?: true;
  disabled?: true;
  readonly?: true;
  value?: string;
  secret?: true;
  empty?: true;
  checked?: boolean | 'mixed';
  files?: string[];
  accept?: string;
  multiple?: true;
  options?: string[];
  more?: number;
  selected?: string[];
  invalid?: true;
  message?: string;
  /** Its `name`, `id` and `placeholder` attributes. */
  html?: string;
  id?: string;
  placeholder?: string;
  radios?: Radio[];
  /** A button, and whether it submits its form. */
  button?: true;
  submit?: true;
}

/** A form, `role=form`, `role=search`, the element a locator named when
 * it is none of these (`scope`), or the fields outside any (`outside`). */
interface Unit {
  el?: number;
  outside?: true;
  scope?: true;
  method?: string;
  action?: string;
  fields: Field[];
}

const isUnit = (el: Element) => tag(el) === 'form' || UNITS.has(roleOf(el) || '');

/** The unit around `el`, not `el` itself. */
function unitAbove(el: Element): Element | null {
  for (let a = parentOf(el); a; a = parentOf(a)) if (isUnit(a)) return a;
  return null;
}

/** The unit a control belongs to: its form owner when `form=` puts it
 * in a form it is not inside, else the innermost unit around it. */
function ownerOf(el: Element): Element | null {
  const owner = (el as HTMLInputElement).form;
  if (owner && !contains(owner, el)) return owner;
  return unitAbove(el);
}

/** An input, `<select>`, `<textarea>`, `<button>` or contenteditable
 * root's kind; `null` for other elements (and hidden inputs). */
function nativeKind(el: Element): string | null {
  const t = tag(el);
  if (el instanceof HTMLInputElement) {
    const type = el.type.toLowerCase();
    if (type === 'hidden') return null;
    return type === 'image' ? 'submit' : type;
  }
  if (t === 'select') return 'select';
  if (t === 'textarea') return 'textarea';
  if (el instanceof HTMLButtonElement) return el.type;
  const html = el as HTMLElement;
  if (html.isContentEditable && !(el.parentElement && el.parentElement.isContentEditable)) return 'contenteditable';
  return null;
}

/** Shown to a user: in the accessibility tree and with a box, or (a
 * checkbox, radio or file input styled away) with a label that shows. */
function shown(el: Element): boolean {
  if (!hiddenForAria(el) && visible(el)) return true;
  const labels = (el as HTMLInputElement).labels;
  return !!labels && Array.from(labels).some((l) => !hiddenForAria(l) && visible(l));
}

/** The element's text or value, whitespace collapsed and cut. */
const shownValue = (s: string) => cut(norm(s), VALUE);

/** Why a field is invalid: its constraint validation message (not for
 * a required field that is only empty: `required` says that), else, when
 * `aria-invalid` is set, the text `aria-errormessage` or
 * `aria-describedby` points to. */
function problem(el: Element): { invalid?: true; message?: string } {
  const native = el as HTMLInputElement;
  if (typeof native.willValidate === 'boolean' && native.willValidate && !native.validity.valid) {
    const v = native.validity;
    const other = v.typeMismatch || v.patternMismatch || v.tooLong || v.tooShort || v.rangeUnderflow ||
      v.rangeOverflow || v.stepMismatch || v.badInput || v.customError;
    if (other) return { invalid: true, message: cut(norm(native.validationMessage), 120) || undefined };
  }
  const aria = attr(el, 'aria-invalid');
  if (!aria || aria === 'false') return {};
  for (const name of ['aria-errormessage', 'aria-describedby']) {
    const ids = attr(el, name);
    if (!ids) continue;
    const text = norm(byIds(el, ids).map((e) => (e as HTMLElement).innerText || e.textContent || '').join(' '));
    if (text) return { invalid: true, message: cut(text, 120) };
  }
  return { invalid: true };
}

/** The options of a custom listbox (or the one a combobox controls):
 * visible `role=option` elements, as `[element, name]`. */
function customOptions(list: Element): [Element, string][] {
  const out: [Element, string][] = [];
  const owned = attr(list, 'aria-owns');
  const els = [...descendants(list), ...(owned ? byIds(list, owned) : [])];
  for (const o of els) {
    if (roleOf(o) !== 'option' || hiddenForAria(o)) continue;
    out.push([o, nameOf(o, 'option')]);
  }
  return out;
}

/** The listboxes a combobox shows its options in: the ones it controls or
 * owns, and the one its active option is in. */
function listsOf(el: Element): Element[] {
  const ids = [attr(el, 'aria-controls'), attr(el, 'aria-owns')].filter(Boolean).join(' ');
  const lists = ids ? byIds(el, ids) : [];
  const active = attr(el, 'aria-activedescendant');
  if (active) {
    for (const o of byIds(el, active)) {
      for (let a = parentOf(o); a; a = parentOf(a)) {
        if (roleOf(a) === 'listbox') {
          lists.push(a);
          break;
        }
      }
    }
  }
  const list = (el as HTMLInputElement).list;
  if (list) lists.push(list);
  return Array.from(new Set(lists));
}

/** Lists `names` (first `OPTIONS`) and how many more there are. */
function listed(field: Field, names: string[]) {
  if (!names.length) return;
  field.options = names.slice(0, OPTIONS).map((n) => cut(n, 40));
  if (names.length > OPTIONS) field.more = names.length - OPTIONS;
}

/** The state of the control `el` as the field `field` shows it. */
function fill(field: Field, el: Element, role: string) {
  const input = el as HTMLInputElement;
  const aria = (name: string) => attr(el, name) === 'true';
  if ((input.required && !(el instanceof HTMLButtonElement)) || aria('aria-required')) field.required = true;
  if (el.matches(':disabled') || el.closest('[aria-disabled="true"]')) field.disabled = true;
  if ((input.readOnly && (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement)) || aria('aria-readonly')) {
    field.readonly = true;
  }
  if (attr(el, 'name')) field.html = attr(el, 'name') as string;
  if (el.id) field.id = el.id;
  if (attr(el, 'placeholder')) field.placeholder = norm(attr(el, 'placeholder'));
  Object.assign(field, problem(el));
  const kind = field.kind;
  if (field.button) {
    if (kind === 'submit') field.submit = true;
    return;
  }
  if (el instanceof HTMLInputElement && (kind === 'checkbox' || kind === 'radio')) {
    field.checked = el.indeterminate && kind === 'checkbox' ? 'mixed' : el.checked;
    if (!el.checked) field.empty = true;
    return;
  }
  if (el instanceof HTMLInputElement && kind === 'file') {
    field.files = Array.from(el.files || []).map((f) => f.name);
    if (el.accept) field.accept = el.accept;
    if (el.multiple) field.multiple = true;
    if (!field.files.length) field.empty = true;
    return;
  }
  if (el instanceof HTMLSelectElement) {
    const options = Array.from(el.options);
    listed(field, options.map((o) => norm(o.label)));
    const selected = options.filter((o) => o.selected).map((o) => norm(o.label));
    if (el.multiple) {
      field.multiple = true;
      field.selected = selected;
    } else {
      field.value = shownValue(selected[0] || '');
    }
    if (el.value === '') field.empty = true;
    return;
  }
  if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) {
    if (input.type === 'password') {
      field.secret = true;
    } else {
      field.value = shownValue(input.value);
    }
    if (input.value === '') field.empty = true;
    if (role === 'combobox') listed(field, listsOf(el).flatMap((l) => l instanceof HTMLDataListElement
      ? Array.from(l.options).map((o) => norm(o.label || o.value))
      : customOptions(l).map(([, n]) => n)));
    return;
  }
  // Custom widgets and contenteditable roots.
  if (role === 'checkbox' || role === 'switch' || role === 'radio') {
    const c = attr(el, 'aria-checked');
    field.checked = c === 'mixed' ? 'mixed' : c === 'true';
    if (c !== 'true') field.empty = true;
    return;
  }
  if (role === 'listbox') {
    const options = customOptions(el);
    listed(field, options.map(([, n]) => n));
    field.selected = options.filter(([o]) => attr(o, 'aria-selected') === 'true').map(([, n]) => n);
    if (aria('aria-multiselectable')) field.multiple = true;
    else if (field.selected.length) field.value = shownValue(field.selected[0]);
    if (!field.selected.length) field.empty = true;
    return;
  }
  if (role === 'slider' || role === 'spinbutton') {
    field.value = shownValue(attr(el, 'aria-valuetext') || attr(el, 'aria-valuenow') || '');
    return;
  }
  if (role === 'combobox') listed(field, listsOf(el).flatMap((l) => customOptions(l).map(([, n]) => n)));
  const text = (el as HTMLElement).innerText || '';
  field.value = shownValue(text);
  if (!norm(text)) field.empty = true;
}

/** The forms of the document as units, each with its fields in
 * document order, as `{json, els}`: every `<form>`, `role=form` and
 * `role=search`, and the controls in none as one unit (`outside`). With
 * `root`, only the unit it is or is inside, else the units in it, else
 * `root` itself as a unit of the controls in it (`scope`). Radios are
 * one field per group (by `radiogroup`, else by `name`); buttons are
 * listed in units, not outside them. */
export function formsList(root: Element | null): { json: string; els: Element[] } {
  const doc = root ? root.ownerDocument : document;
  const names: Named[] = [];
  const els: Element[] = [];
  const index = new Map<Element, number>();
  const ref = (el: Element): number => {
    let i = index.get(el);
    if (i === undefined) {
      i = els.length;
      index.set(el, i);
      els.push(el);
      names.push(named(el));
    }
    return i;
  };
  const all = descendants(doc);
  let wanted: Element[] | null = null;
  let scope: Element | null = null;
  if (root) {
    const up = isUnit(root) ? root : unitAbove(root);
    if (up) wanted = [up];
    else {
      const inside = all.filter((e) => contains(root, e) && e !== root && isUnit(e));
      if (inside.length) wanted = inside;
      else scope = root;
    }
  }
  const units = new Map<Element | null, Unit>();
  const unitOf = (owner: Element | null): Unit | null => {
    if (scope) owner = scope;
    else if (wanted && (!owner || !wanted.includes(owner))) return null;
    let unit = units.get(owner);
    if (!unit) {
      unit = { fields: [] };
      if (owner) unit.el = ref(owner);
      else unit.outside = true;
      if (scope) unit.scope = true;
      if (owner instanceof HTMLFormElement && (owner.hasAttribute('action') || owner.hasAttribute('method'))) {
        unit.method = owner.method.toUpperCase();
        const action = new URL(owner.action, doc.baseURI);
        const here = new URL(doc.URL);
        unit.action = action.origin === here.origin ? action.pathname + action.search : action.href;
      }
      units.set(owner, unit);
    }
    return unit;
  };
  // Every unit wanted is listed, even without fields.
  for (const e of all) {
    if (!isUnit(e)) continue;
    if (scope ? false : wanted ? wanted.includes(e) : true) unitOf(e);
  }
  if (scope) unitOf(scope);
  const groups = new Map<Element | string, Field>();
  const inputRole = new Map<Element, string>();
  const claimed: Element[] = [];
  // A combobox's listbox is part of it, not a field.
  const popups = new Set(all.filter((e) => roleOf(e) === 'combobox').flatMap(listsOf));
  for (const el of all) {
    if (scope && !contains(scope, el)) continue;
    while (claimed.length && !contains(claimed[claimed.length - 1], el)) claimed.pop();
    if (claimed.length) continue;
    let role = inputRole.get(el) || roleOf(el) || '';
    // A button with a widget's role (`<button role=switch>`) is that
    // widget.
    const native = el instanceof HTMLButtonElement && WIDGETS.has(role) ? null : nativeKind(el);
    if (!native && !WIDGETS.has(role)) continue;
    if (popups.has(el)) {
      claimed.push(el);
      continue;
    }
    if (!native && WRAPS_INPUT.has(role) && el.querySelector('input:not([type=hidden]), textarea')) {
      const inner = el.querySelector('input:not([type=hidden]), textarea') as Element;
      inputRole.set(inner, role);
      continue;
    }
    const unit = unitOf(ownerOf(el));
    if (!unit) continue;
    const isRadio = native === 'radio' || (!native && role === 'radio');
    if (role === 'radiogroup' && !native) {
      if (!shown(el)) continue;
      const group: Field = { el: ref(el), kind: 'radiogroup', role, custom: true, radios: [] };
      fill(group, el, role);
      delete group.empty;
      groups.set(el, group);
      unit.fields.push(group);
      continue;
    }
    if (isRadio) {
      if (!shown(el)) continue;
      let around: Element | null = null;
      for (let a = parentOf(el); a && a !== doc.body; a = parentOf(a)) {
        if (roleOf(a) === 'radiogroup') {
          around = a;
          break;
        }
      }
      // Native radios group by their form owner and name.
      let key: Element | string = around || el;
      if (!around && native && attr(el, 'name')) {
        const owner = ownerOf(el);
        key = `${owner ? ref(owner) : ''}|${attr(el, 'name')}`;
      }
      let group = groups.get(key);
      if (!group) {
        // A group of native radios is named by the fieldset around it.
        const fieldset = el.closest('fieldset');
        group = { el: ref(fieldset && !around ? fieldset : el), kind: 'radiogroup', role: 'radiogroup', radios: [] };
        if (native && attr(el, 'name')) group.html = attr(el, 'name') as string;
        groups.set(key, group);
        unit.fields.push(group);
      }
      const input = el as HTMLInputElement;
      const checked = native ? input.checked : attr(el, 'aria-checked') === 'true';
      const radio: Radio = { el: ref(el), name: nameOf(el, 'radio'), checked };
      if (native) radio.value = input.value;
      if (el.matches(':disabled') || el.closest('[aria-disabled="true"]')) radio.disabled = true;
      if ((native && input.required) || attr(el, 'aria-required') === 'true') group.required = true;
      group.radios!.push(radio);
      if (!group.invalid) Object.assign(group, problem(el));
      continue;
    }
    if (!shown(el)) continue;
    const button = !!native && (el instanceof HTMLButtonElement || ['submit', 'reset', 'button'].includes(native));
    if (button && unit.outside) continue;
    // A native control keeps an explicit widget role (`<input
    // role=combobox>`); a file input is a `file`, not a button.
    if (native === 'file') role = 'file';
    const field: Field = { el: ref(el), kind: native || role, role };
    if (!native) field.custom = true;
    if (button) field.button = true;
    fill(field, el, role);
    unit.fields.push(field);
    claimed.push(el);
  }
  for (const group of groups.values()) {
    if (group.radios && !group.radios.some((r) => r.checked)) group.empty = true;
    if (group.radios) group.value = group.radios.find((r) => r.checked)?.name;
  }
  const out = Array.from(units.values()).filter((u) => !u.outside || u.fields.length);
  // The outside unit last.
  out.sort((a, b) => Number(!!a.outside) - Number(!!b.outside));
  return { json: JSON.stringify({ units: out, names }), els };
}

/** The option named `want` that `el` offers: in the listbox `el` is, or
 * (a combobox) the ones it controls, else any listbox shown. Matched by
 * name ignoring case, else by the one name that starts with it, else by
 * the one that has it. As `{json, els: [option]}`: `found`, how many
 * options there were, and the first few names. */
export function pickOption(el: Element, want: string): { json: string; els: Element[] } {
  let lists = roleOf(el) === 'listbox' ? [el] : listsOf(el).filter((l) => !(l instanceof HTMLDataListElement));
  if (!lists.length && roleOf(el) !== 'listbox') {
    lists = descendants(el.ownerDocument).filter((l) => roleOf(l) === 'listbox' && !hiddenForAria(l) && visible(l)
      && tag(l) !== 'select');
  }
  const options = lists.flatMap(customOptions).filter(([o]) => visible(o));
  const w = norm(want).toLowerCase();
  const by = (test: (n: string) => boolean) => options.filter(([, n]) => test(norm(n).toLowerCase()));
  let found = by((n) => n === w);
  if (found.length !== 1) {
    const starts = by((n) => n.startsWith(w));
    found = found.length > 1 ? found : starts.length === 1 ? starts : by((n) => n.includes(w));
  }
  const pick = found.length === 1 || (found.length > 1 && norm(found[0][1]).toLowerCase() === w) ? found[0] : null;
  const out = {
    found: !!pick,
    count: options.length,
    options: options.slice(0, OPTIONS).map(([, n]) => cut(n, 40)),
    selected: pick ? attr(pick[0], 'aria-selected') === 'true' : false,
    name: pick ? named(pick[0]) : null,
  };
  return { json: JSON.stringify(out), els: pick ? [pick[0]] : [] };
}
