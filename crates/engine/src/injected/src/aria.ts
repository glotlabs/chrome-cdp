// ARIA roles, accessible names (a compact take on accname 1.2) and
// states, shared by the snapshot walker (engine B) and `role=` locators.

import { SKIP, attr, childrenOf, norm, parentOf, tag } from './dom';

export const INTERACTIVE = new Set(['button', 'link', 'textbox', 'searchbox', 'checkbox', 'radio',
  'switch', 'combobox', 'listbox', 'option', 'menuitem', 'menuitemcheckbox', 'menuitemradio',
  'tab', 'slider', 'spinbutton', 'colorwell', 'treeitem', 'iframe']);
const FROM_CONTENT = new Set(['button', 'link', 'heading', 'tab', 'option', 'menuitem',
  'menuitemcheckbox', 'menuitemradio', 'treeitem', 'checkbox', 'radio', 'switch', 'cell',
  'gridcell', 'columnheader', 'rowheader', 'tooltip', 'row']);
const KNOWN = new Set([...INTERACTIVE, ...FROM_CONTENT, 'alert', 'alertdialog', 'application',
  'article', 'banner', 'blockquote', 'caption', 'code', 'complementary', 'contentinfo',
  'definition', 'deletion', 'dialog', 'directory', 'document', 'emphasis', 'feed', 'figure',
  'form', 'generic', 'grid', 'group', 'img', 'image', 'insertion', 'list', 'listitem', 'log',
  'main', 'marquee', 'math', 'menu', 'menubar', 'meter', 'navigation', 'none', 'note',
  'paragraph', 'presentation', 'progressbar', 'radiogroup', 'region', 'rowgroup', 'scrollbar',
  'search', 'separator', 'status', 'strong', 'subscript', 'superscript', 'table', 'tablist',
  'tabpanel', 'term', 'time', 'timer', 'toolbar', 'tree', 'treegrid']);
const INPUT: Record<string, string> = {
  button: 'button', submit: 'button', reset: 'button', image: 'button', file: 'button',
  checkbox: 'checkbox', radio: 'radio', range: 'slider', number: 'spinbutton',
  search: 'searchbox', color: 'colorwell',
};

const ariaTrue = (el: Element, name: string) => attr(el, name) === 'true';
const scoped = (el: Element) => el.closest('article, aside, main, nav, section');
const styleOf = (el: Element) => (el.ownerDocument.defaultView as Window).getComputedStyle(el);

function implicitRole(el: Element): string | null {
  const t = tag(el);
  switch (t) {
    case 'a': case 'area': return el.hasAttribute('href') ? 'link' : 'generic';
    case 'article': return 'article';
    case 'aside': return 'complementary';
    case 'button': return 'button';
    case 'datalist': return 'listbox';
    case 'dd': return 'definition';
    case 'dl': return 'descriptionlist';
    case 'details': return 'group';
    case 'dfn': case 'dt': return 'term';
    case 'dialog': return 'dialog';
    case 'fieldset': case 'optgroup': return 'group';
    case 'figure': return 'figure';
    case 'figcaption': return 'figcaption';
    case 'footer': return scoped(el.parentElement || el) ? 'generic' : 'contentinfo';
    case 'header': return scoped(el.parentElement || el) ? 'generic' : 'banner';
    case 'form': return 'form';
    case 'h1': case 'h2': case 'h3': case 'h4': case 'h5': case 'h6': return 'heading';
    case 'hr': return 'separator';
    case 'iframe': case 'frame': return 'iframe';
    case 'img': return attr(el, 'alt') === '' && !el.hasAttribute('title') ? 'presentation' : 'img';
    case 'input': {
      const type = (attr(el, 'type') || 'text').toLowerCase();
      if (type === 'hidden') return null;
      if (el.hasAttribute('list') && !INPUT[type]) return 'combobox';
      return INPUT[type] || 'textbox';
    }
    case 'label': return 'label';
    case 'legend': return 'legend';
    case 'li': return 'listitem';
    case 'main': return 'main';
    case 'menu': case 'ol': case 'ul': return 'list';
    case 'meter': return 'meter';
    case 'nav': return 'navigation';
    case 'option': return 'option';
    case 'output': return 'status';
    case 'p': return 'paragraph';
    case 'progress': return 'progressbar';
    case 'search': return 'search';
    case 'section': return el.hasAttribute('aria-label') || el.hasAttribute('aria-labelledby') ? 'region' : 'generic';
    case 'select': {
      const select = el as HTMLSelectElement;
      return select.multiple || select.size > 1 ? 'listbox' : 'combobox';
    }
    case 'summary': return 'button';
    case 'table': return 'table';
    case 'tbody': case 'thead': case 'tfoot': return 'rowgroup';
    case 'td': return el.closest('table[role=grid], [role=grid]') ? 'gridcell' : 'cell';
    case 'th': return attr(el, 'scope') === 'row' ? 'rowheader' : 'columnheader';
    case 'textarea': return 'textbox';
    case 'tr': return 'row';
    case 'caption': return 'caption';
    case 'blockquote': return 'blockquote';
    case 'code': return 'code';
    case 'em': return 'emphasis';
    case 'strong': return 'strong';
    case 'time': return 'time';
    case 'svg': return 'img';
    case 'math': return 'math';
    case 'html': case 'body': return 'none';
    default: {
      const html = el as HTMLElement;
      if (html.isContentEditable && !(el.parentElement && el.parentElement.isContentEditable)) return 'textbox';
      return 'generic';
    }
  }
}

/** The element's ARIA role (`null` for elements that have none, like
 * `<input type=hidden>`). */
export function roleOf(el: Element): string | null {
  const explicit = (attr(el, 'role') || '').trim().toLowerCase().split(/\s+/).find((r) => KNOWN.has(r));
  if (explicit === 'presentation' || explicit === 'none') {
    // Focusable elements keep their role (ARIA's conflict resolution).
    return (el as HTMLElement).tabIndex >= 0 && el.hasAttribute('tabindex') ? implicitRole(el) : 'none';
  }
  if (explicit === 'image') return 'img';
  return explicit || implicitRole(el);
}

export function hidden(el: Element, style: CSSStyleDeclaration): boolean {
  return style.display === 'none' || ariaTrue(el, 'aria-hidden') || el.hasAttribute('inert');
}

/** The open modal `<dialog>` of the element's document, if any: it makes
 * the rest of the page inert. */
export function openModal(doc: Document): Element | null {
  return doc.querySelector('dialog:modal');
}

/** Whether an open modal dialog makes `el` inert: it is neither in the
 * dialog nor around it. */
export function outsideModal(el: Element, modal: Element | null): boolean {
  return !!modal && !modal.contains(el) && !el.contains(modal);
}

/** Whether the element is left out of the accessibility tree: it or an
 * ancestor is `display: none`, `aria-hidden` or inert (also by an open
 * modal dialog), or it is `visibility: hidden`. */
export function hiddenForAria(el: Element): boolean {
  const style = styleOf(el);
  if (style.visibility === 'hidden' || style.visibility === 'collapse') return true;
  if (outsideModal(el, openModal(el.ownerDocument))) return true;
  for (let e: Element | null = el; e; e = parentOf(e)) {
    if (ariaTrue(e, 'aria-hidden') || e.hasAttribute('inert')) return true;
    if (styleOf(e).display === 'none') return true;
  }
  return false;
}

/** The text an element contributes to a name (accname's step 2F/2G).
 * A tree item's name leaves out its nested group (as Chrome does). */
export function textOf(node: Node, skip: Element | null, depth: number, treeitem?: boolean): string {
  if (depth > 30) return '';
  if (node.nodeType === Node.TEXT_NODE) return (node as Text).data;
  if (node.nodeType !== Node.ELEMENT_NODE && node.nodeType !== Node.DOCUMENT_FRAGMENT_NODE) return '';
  if (node.nodeType === Node.ELEMENT_NODE) {
    const el = node as Element;
    if (el === skip || SKIP.has(tag(el))) return '';
    if (treeitem && (attr(el, 'role') === 'group' || attr(el, 'role') === 'treeitem')) return '';
    const style = styleOf(el);
    if (hidden(el, style) || style.visibility === 'hidden') return '';
    const label = attr(el, 'aria-label');
    if (label && label.trim()) return ' ' + label + ' ';
    const t = tag(el);
    if (t === 'img' || (t === 'input' && attr(el, 'type') === 'image')) return ' ' + (attr(el, 'alt') || '') + ' ';
    if (t === 'input' || t === 'textarea') {
      const role = roleOf(el);
      const value = (el as HTMLInputElement).value;
      if (role === 'textbox' || role === 'searchbox' || role === 'spinbutton' || role === 'slider') return ' ' + value + ' ';
      if (role === 'button') return ' ' + (value || '') + ' ';
      return '';
    }
    if (t === 'select') {
      const opt = (el as HTMLSelectElement).selectedOptions[0];
      return opt ? ' ' + opt.text + ' ' : '';
    }
    if (t === 'br') return ' ';
    if (t === 'svg') return ' ' + svgTitle(el) + ' ';
    const block = style.display !== 'inline' && style.display !== 'contents';
    let out = '';
    for (const child of childrenOf(el)) out += textOf(child, skip, depth + 1, treeitem);
    return block ? ' ' + out + ' ' : out;
  }
  let out = '';
  for (const child of Array.from(node.childNodes)) out += textOf(child, skip, depth + 1, treeitem);
  return out;
}

function svgTitle(el: Element): string {
  const title = Array.from(el.children).find((c) => tag(c) === 'title');
  return title ? title.textContent || '' : '';
}

export function byIds(el: Element, ids: string): Element[] {
  const root = el.getRootNode() as Document | ShadowRoot;
  return ids.split(/\s+/)
    .map((id) => (root.getElementById ? root.getElementById(id) : el.ownerDocument.getElementById(id)))
    .filter((e): e is HTMLElement => !!e);
}

/** The texts that label a control: its `aria-labelledby` elements, its
 * `aria-label` and its `<label>`s (for `label=` locators). */
export function labelsOf(el: Element): string[] {
  const out: string[] = [];
  const labelledby = attr(el, 'aria-labelledby');
  if (labelledby) out.push(norm(byIds(el, labelledby).map((l) => textOf(l, null, 0)).join(' ')));
  const label = attr(el, 'aria-label');
  if (label) out.push(norm(label));
  const labels = (el as HTMLInputElement).labels;
  if (labels) for (const l of Array.from(labels)) out.push(norm(textOf(l, el, 0)));
  return out.filter(Boolean);
}

/** The element's accessible name, for its `role`. */
export function nameOf(el: Element, role: string): string {
  const labelledby = attr(el, 'aria-labelledby');
  if (labelledby) {
    const text = norm(byIds(el, labelledby).map((l) => textOf(l, null, 0)).join(' '));
    if (text) return text;
  }
  const label = norm(attr(el, 'aria-label'));
  if (label) return label;
  const t = tag(el);
  if (t === 'input' || t === 'textarea' || t === 'select' || t === 'meter' || t === 'progress' || t === 'output') {
    const type = (attr(el, 'type') || '').toLowerCase();
    const value = (el as HTMLInputElement).value;
    if (t === 'input' && ['button', 'submit', 'reset'].includes(type)) {
      return norm(value) || (type === 'submit' ? 'Submit' : type === 'reset' ? 'Reset' : '');
    }
    if (t === 'input' && type === 'image') return norm(attr(el, 'alt')) || 'Submit';
    const labels = (el as HTMLInputElement).labels ? Array.from((el as HTMLInputElement).labels as NodeListOf<HTMLLabelElement>) : [];
    const text = norm(labels.map((l) => textOf(l, el, 0)).join(' '));
    if (text) return text;
    return norm(attr(el, 'title')) || norm(attr(el, 'placeholder'));
  }
  if (t === 'img' || t === 'area') return norm(attr(el, 'alt')) || norm(attr(el, 'title'));
  if (t === 'svg') return norm(svgTitle(el)) || norm(attr(el, 'title'));
  if (t === 'fieldset') {
    const legend = Array.from(el.children).find((c) => tag(c) === 'legend');
    if (legend) return norm(textOf(legend, null, 0));
  }
  if (t === 'table') {
    const caption = (el as HTMLTableElement).caption;
    if (caption) return norm(textOf(caption, null, 0));
  }
  if (t === 'iframe') return norm(attr(el, 'title'));
  if (FROM_CONTENT.has(role) && role !== 'row') {
    let text = '';
    for (const child of childrenOf(el)) text += textOf(child, null, 1, role === 'treeitem');
    text = norm(text);
    if (text) return text;
  }
  return norm(attr(el, 'title'));
}

/** The name as a snapshot line shows it: generic containers and
 * paragraphs are named only by `aria-label`. A row, which has no name
 * of its own in the snapshot, is named by its text. */
export function shownName(el: Element, role: string): string {
  if (role === 'none' || role === 'generic' || role === 'paragraph') return norm(attr(el, 'aria-label'));
  return nameOf(el, role);
}

export function valueOf(el: Element, role: string): string | null {
  const now = attr(el, 'aria-valuetext') || attr(el, 'aria-valuenow');
  const t = tag(el);
  if (t === 'input' || t === 'textarea') return (el as HTMLInputElement).value;
  if (t === 'select') {
    const opt = (el as HTMLSelectElement).selectedOptions[0];
    return opt ? opt.text : '';
  }
  if (role === 'textbox' && (el as HTMLElement).isContentEditable) return (el as HTMLElement).innerText;
  return now;
}

/** Short state keys, as the walker reports them: c(hecked), (e)x(panded),
 * s(elected), d(isabled), r(eadonly), (re)q(uired), i(nvalid),
 * f(ocused), p(ressed). */
export interface States {
  c?: string;
  x?: boolean;
  s?: 1;
  d?: 1;
  r?: 1;
  q?: 1;
  i?: 1;
  f?: 1;
  p?: 1;
  /** `aria-pressed="mixed"`, for `role=…[pressed=mixed]`. */
  pm?: 1;
}

/** The focused element, inside shadow roots too. */
export function activeElement(): Element | null {
  let a = document.activeElement;
  while (a && a.shadowRoot && a.shadowRoot.activeElement) a = a.shadowRoot.activeElement;
  return a;
}

export function statesOf(el: Element, role: string, active: Element | null): States {
  const s: States = {};
  const t = tag(el);
  const input = el as HTMLInputElement;
  if (t === 'input' && (input.type === 'checkbox' || input.type === 'radio')) {
    s.c = input.indeterminate ? 'mixed' : input.checked ? 'true' : 'false';
  } else if (['checkbox', 'radio', 'switch', 'menuitemcheckbox', 'menuitemradio'].includes(role)) {
    const c = attr(el, 'aria-checked');
    s.c = c === 'true' || c === 'mixed' ? c : 'false';
  }
  if (el.hasAttribute('aria-expanded')) s.x = ariaTrue(el, 'aria-expanded');
  else if (t === 'summary' && el.parentElement && tag(el.parentElement) === 'details') s.x = (el.parentElement as HTMLDetailsElement).open;
  else if (t === 'select' && role === 'combobox') s.x = false;
  if (t === 'option' ? (el as HTMLOptionElement).selected : ariaTrue(el, 'aria-selected')) s.s = 1;
  if ((el.matches && el.matches(':disabled')) || el.closest('[aria-disabled="true"]')) s.d = 1;
  if (input.readOnly || ariaTrue(el, 'aria-readonly')) s.r = 1;
  if (input.required || ariaTrue(el, 'aria-required')) s.q = 1;
  const invalid = attr(el, 'aria-invalid');
  if (invalid && invalid !== 'false') s.i = 1;
  if (el === active && el !== document.body) s.f = 1;
  if (ariaTrue(el, 'aria-pressed')) s.p = 1;
  if (attr(el, 'aria-pressed') === 'mixed') s.pm = 1;
  return s;
}

export function level(el: Element, role: string): number | undefined {
  if (role !== 'heading') return undefined;
  const aria = parseInt(attr(el, 'aria-level') || '', 10);
  if (aria > 0) return aria;
  const m = /^h([1-6])$/.exec(tag(el));
  return m ? Number(m[1]) : 2;
}
