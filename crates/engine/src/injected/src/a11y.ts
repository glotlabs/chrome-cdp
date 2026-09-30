// `expect --a11y` (plan 09, milestone 9.1; more rules in plan 17,
// milestone 17.3): a built-in set of accessibility rules, checked in the
// page's DOM. It is not axe-core (see plan 09's decisions log); rule ids
// are axe's, so results read the same. Each rule has its WCAG success
// criterion and level; `--level A` checks the level A rules, `--level AA`
// (the default) all of them. Where only Chrome knows the answer (is a
// table a layout table, what is a heading's name), a rule lists
// candidates and names what the engine asks Chrome's accessibility tree
// (`ax`) to keep them.

import { disabled } from './actionable';
import {
  INTERACTIVE, activeElement, hiddenForAria, implicitRole, level as headingLevel, nameOf, roleOf,
} from './aria';
import {
  ATTRS, REQUIRED_ATTRS, REQUIRED_CHILDREN, REQUIRED_PARENT, ROLES, badAutocomplete, badValue, closest,
} from './ariaspec';
import { descendants, norm, parentOf, tag, visible } from './dom';
import { Candidate, describe } from './locator';

interface Rule {
  id: string;
  wcag: string;
  level: 'A' | 'AA';
  /** What is wrong, for the output: `has no alt text`. */
  help: string;
  /** Page rules check the document once, not elements. */
  page?: (doc: Document, ctx: Context) => string | null;
  check?: (el: Element, ctx: Context) => string | boolean;
  /** What Chrome's accessibility tree must say for a listed element to
   * stay a violation (the engine asks it): `data-table`, a table Chrome
   * does not take for a layout table; `no-name`, an empty name. */
  ax?: 'data-table' | 'no-name';
}

/** What several rules need of the whole check, worked out once, when a
 * rule first asks. */
class Context {
  private memo = new Map<string, unknown>();

  constructor(readonly all: Element[], readonly hidden: Map<Element, boolean>) {}

  private once<T>(key: string, make: () => T): T {
    if (!this.memo.has(key)) this.memo.set(key, make());
    return this.memo.get(key) as T;
  }

  /** Elements shown to assistive tech. */
  shown(): Element[] {
    return this.once('shown', () => this.all.filter((el) => !this.hidden.get(el)));
  }

  /** The elements that `aria-owns` moves, and their owner. */
  owners(): Map<Element, Element> {
    return this.once('owners', () => {
      const out = new Map<Element, Element>();
      for (const el of this.all) {
        const ids = el.getAttribute('aria-owns');
        if (!ids) continue;
        for (const owned of idsIn(el, ids)) out.set(owned, el);
      }
      return out;
    });
  }

  /** Each heading's previous heading's level (in document order). */
  previousLevels(): Map<Element, number> {
    return this.once('headings', () => {
      const out = new Map<Element, number>();
      let previous = 0;
      for (const el of this.shown()) {
        if (roleOf(el) !== 'heading') continue;
        if (previous) out.set(el, previous);
        previous = headingLevel(el, 'heading') || 2;
      }
      return out;
    });
  }

  /** A root's elements by id, and which ids something refers to (and
   * how). */
  ids(root: Document | ShadowRoot): { byId: Map<string, Element[]>; referred: Map<string, string> } {
    const key = root instanceof ShadowRoot ? root : root.documentElement;
    let roots = this.memo.get('ids') as Map<Node, ReturnType<Context['ids']>> | undefined;
    if (!roots) this.memo.set('ids', (roots = new Map()));
    let found = roots.get(key);
    if (found) return found;
    found = { byId: new Map(), referred: new Map() };
    for (const el of Array.from(root.querySelectorAll('*'))) {
      const id = el.id;
      if (id) {
        const list = found.byId.get(id);
        if (list) list.push(el);
        else found.byId.set(id, [el]);
      }
      if (tag(el) === 'label' && el.getAttribute('for')) {
        const id = el.getAttribute('for') as string;
        if (!found.referred.has(id)) found.referred.set(id, 'a <label for>');
      }
      for (const name of IDREFS) {
        const ids = el.getAttribute(name);
        if (!ids) continue;
        for (const id of ids.trim().split(/\s+/)) if (!found.referred.has(id)) found.referred.set(id, name);
      }
    }
    roots.set(key, found);
    return found;
  }

  /** The pointer targets shown: their box and whether it is under
   * 24×24, by element. */
  targets(): Map<Element, Target> {
    return this.once('targets', () => new Map(this.shown()
      .filter((el) => TARGETS.has(roleOf(el) || '') && !disabled(el) && visible(el))
      .map((el) => {
        const box = el.getBoundingClientRect();
        return [el, { el, box, small: box.width < 24 - 0.01 || box.height < 24 - 0.01 }];
      })));
  }

  /** How many elements `focus-visible` focused. */
  focused = 0;
}

interface Target {
  el: Element;
  box: DOMRect;
  small: boolean;
}

/** The attributes that refer to elements by id. */
const IDREFS = Object.keys(ATTRS).filter((name) => ATTRS[name] === 'idref' || ATTRS[name] === 'idrefs');

/** The elements of `el`'s root that `ids` names. */
function idsIn(el: Element, ids: string): Element[] {
  const root = el.getRootNode() as Document | ShadowRoot;
  return ids.trim().split(/\s+/).map((id) => root.getElementById(id)).filter((e): e is HTMLElement => !!e);
}

/** Whether an element with `id` is in `el`'s root. */
function hasId(el: Element, id: string): boolean {
  return !!(el.getRootNode() as Document | ShadowRoot).getElementById(id);
}

/** The role the `role` attribute gives (its first token), lower-case,
 * or `null` when it has none. */
function explicitRole(el: Element): string | null {
  const first = (el.getAttribute('role') || '').trim().toLowerCase().split(/\s+/)[0];
  return first || null;
}

/** Roles that hold others and are not a role of their own to the
 * structure rules: an owner is looked for through them. */
const TRANSPARENT = new Set(['none', 'presentation', 'generic']);

/** The element that owns `el` in the accessibility tree: the one whose
 * `aria-owns` names it, else its parent. */
function owner(el: Element, ctx: Context): Element | null {
  return ctx.owners().get(el) || parentOf(el);
}

/** `a tab`, `an option`. */
function a(role: string): string {
  return /^[aeiou]/.test(role) ? `an ${role}` : `a ${role}`;
}

/** `tab`, `tab or group`, `row, rowgroup or caption`. */
function either(roles: string[]): string {
  return roles.length < 2 ? roles.join('') : `${roles.slice(0, -1).join(', ')} or ${roles[roles.length - 1]}`;
}

/** What `el` owns, looking through presentational and generic elements
 * (hidden ones and scripts left out). */
function ownedOf(el: Element, ctx: Context): Element[] {
  const out: Element[] = [];
  const visit = (parent: Element, depth: number) => {
    const kids = [...Array.from((parent.shadowRoot || parent).children)];
    if (parent === el) {
      const ids = el.getAttribute('aria-owns');
      if (ids) kids.push(...idsIn(el, ids));
    }
    for (const kid of kids) {
      if (['script', 'template', 'style'].includes(tag(kid)) || ctx.hidden.get(kid) || hiddenForAria(kid)) continue;
      const role = roleOf(kid);
      if (!role) continue;
      if (TRANSPARENT.has(role) && depth < 20) visit(kid, depth + 1);
      else out.push(kid);
    }
  };
  visit(el, 0);
  return out;
}

/** Pointer targets, for `target-size`. */
const TARGETS = new Set([...INTERACTIVE].filter((r) => r !== 'iframe'));

/** Why a pointer target is too small, or `null`: under 24×24 CSS px
 * and a 24px circle on its centre reaches another target (or another
 * small target's circle). A target in a line of text and a control the
 * browser sized (an unstyled checkbox or button) are exempt, as WCAG
 * 2.5.8 says. */
function targetSize(el: Element, ctx: Context): string | null {
  const me = ctx.targets().get(el);
  if (!me || !me.small) return null;
  const style = getComputedStyle(el);
  if (style.display === 'inline') {
    const parent = el.parentElement;
    if (parent && norm(parent.textContent).length > norm(el.textContent).length) return null;
  }
  if (uaSized(el, style)) return null;
  const cx = me.box.left + me.box.width / 2;
  const cy = me.box.top + me.box.height / 2;
  for (const other of ctx.targets().values()) {
    if (other.el === el || other.el.contains(el) || el.contains(other.el)) continue;
    const b = other.box;
    const dx = Math.max(b.left - cx, 0, cx - b.right);
    const dy = Math.max(b.top - cy, 0, cy - b.bottom);
    let near = Math.hypot(dx, dy) < 12;
    if (!near && other.small) {
      near = Math.hypot(b.left + b.width / 2 - cx, b.top + b.height / 2 - cy) < 24;
    }
    if (near) return `${Math.round(me.box.width)}×${Math.round(me.box.height)}px, ${describeShort(other.el)} is within 12px of its centre`;
  }
  return null;
}

/** Whether the browser sized a form control: it draws it (`appearance`
 * is not `none`) in its own font size for controls, which an author who
 * styles a control nearly always changes. */
function uaSized(el: Element, style: CSSStyleDeclaration): boolean {
  if (!['input', 'button', 'select', 'textarea'].includes(tag(el))) return false;
  return style.appearance !== 'none' && Math.abs(parseFloat(style.fontSize) - 40 / 3) < 0.01;
}

/** `button "Next"`, for a note about another element. */
function describeShort(el: Element): string {
  const c = describe(el);
  return c.name ? `${c.role} "${c.name}"` : c.role;
}

/** How many elements `focus-visible` focuses at most. */
const FOCUS_CHECKED = 300;

/** What focus could change of an element's look: its outline, shadow,
 * borders, colours and underline, its `::before` and `::after`, its
 * nearest ancestors (`:focus-within`) and its first descendants. */
function look(el: Element): string {
  const parts: string[] = [];
  const add = (e: Element, pseudo?: string) => {
    const s = getComputedStyle(e, pseudo);
    if (pseudo && (s.content === 'none' || s.content === 'normal')) return;
    const outline = s.outlineStyle === 'none' || parseFloat(s.outlineWidth) === 0
      ? 'none' : `${s.outlineStyle} ${s.outlineWidth} ${s.outlineColor} ${s.outlineOffset}`;
    const borders = ['Top', 'Right', 'Bottom', 'Left'].map((side) => {
      const get = (p: string) => s.getPropertyValue(`border-${side.toLowerCase()}-${p}`);
      return get('style') === 'none' || parseFloat(get('width')) === 0 ? '-' : `${get('width')} ${get('style')} ${get('color')}`;
    });
    parts.push(outline, s.boxShadow, borders.join(','), s.backgroundColor, s.backgroundImage, s.color,
      s.textDecorationLine, s.opacity, s.transform, pseudo ? s.content : '');
  };
  add(el);
  add(el, '::before');
  add(el, '::after');
  let up = parentOf(el);
  for (let i = 0; up && i < 3; i++, up = parentOf(up)) add(up);
  descendants(el).slice(0, 10).forEach((d) => add(d));
  return parts.join('|');
}

/** Whether `el` looks the same with keyboard focus as without: `true`
 * (a violation), or `false` when it changes or cannot be judged (it
 * takes no focus, or the document has none to give). */
function noFocusChange(el: Element, ctx: Context): boolean {
  if (!focusable(el) || tag(el) === 'iframe' || !visible(el) || ctx.focused >= FOCUS_CHECKED) return false;
  ctx.focused++;
  const before = look(el);
  (el as HTMLElement).focus({ preventScroll: true, focusVisible: true } as FocusOptions);
  if (activeElement() !== el) return false;
  if (!el.matches(':focus-visible')) {
    (el as HTMLElement).blur();
    return false;
  }
  // A transition started: something changes, over time.
  const same = !el.getAnimations().length && look(el) === before;
  (el as HTMLElement).blur();
  return same;
}

/** A form control that needs a name (not a button or a hidden input). */
function field(el: Element): boolean {
  const t = tag(el);
  if (t === 'select' || t === 'textarea') return true;
  if (t !== 'input') return false;
  const type = ((el as HTMLInputElement).type || 'text').toLowerCase();
  return !['hidden', 'button', 'submit', 'reset', 'image'].includes(type);
}

/** Whether keyboard focus can reach the element. */
function focusable(el: Element): boolean {
  const h = el as HTMLElement;
  if (h.tabIndex < 0 || disabled(el)) return false;
  if (h.tabIndex > 0 || el.hasAttribute('tabindex')) return true;
  const t = tag(el);
  if (t === 'a' || t === 'area') return el.hasAttribute('href');
  return ['button', 'input', 'select', 'textarea', 'iframe', 'summary'].includes(t) || h.isContentEditable;
}

type Rgba = [number, number, number, number];

function parseColour(s: string): Rgba | null {
  const m = /^rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)(?:\s*[,/]\s*([\d.]+%?))?\s*\)$/.exec(s.trim());
  if (!m) return null;
  let a = m[4] === undefined ? 1 : parseFloat(m[4]);
  if (m[4] && m[4].endsWith('%')) a /= 100;
  return [parseFloat(m[1]), parseFloat(m[2]), parseFloat(m[3]), a];
}

/** `top` drawn over the opaque colour `under`. */
function over(top: Rgba, under: Rgba): Rgba {
  const a = top[3];
  return [0, 1, 2].map((i) => top[i] * a + under[i] * (1 - a)).concat(1) as Rgba;
}

function luminance(c: Rgba): number {
  const [r, g, b] = [0, 1, 2].map((i) => {
    const v = c[i] / 255;
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** The opaque colour behind `el`'s text, or `null` when it cannot be
 * known from styles alone (a background image or gradient). */
function background(el: Element): Rgba | null {
  const layers: Rgba[] = [];
  for (let a: Element | null = el; a; a = a.parentElement) {
    const style = getComputedStyle(a);
    if (style.backgroundImage !== 'none') return null;
    const c = parseColour(style.backgroundColor);
    if (c && c[3] > 0) {
      layers.push(c);
      if (c[3] >= 1) break;
    }
  }
  let colour: Rgba = [255, 255, 255, 1];
  for (let i = layers.length - 1; i >= 0; i--) colour = over(layers[i], colour);
  return colour;
}

/** How much `opacity` fades `el`'s text: the product of its and its
 * ancestors' opacity (as axe does; a faded background is taken to fade
 * into a lighter page, like the text). */
function fade(el: Element): number {
  let out = 1;
  for (let a: Element | null = el; a; a = a.parentElement) out *= parseFloat(getComputedStyle(a).opacity);
  return out;
}

/** Text too faint for its size: `null`, or why (the ratio found). */
function contrast(el: Element): string | null {
  const own = Array.from(el.childNodes).some((n) => n.nodeType === Node.TEXT_NODE && (n as Text).data.trim());
  if (!own || !visible(el) || disabled(el)) return null;
  const style = getComputedStyle(el);
  // A shadow may be what makes the text readable: it cannot be judged
  // from styles.
  if (style.textShadow !== 'none') return null;
  const fg = parseColour(style.color);
  const bg = background(el);
  if (!fg || !bg) return null;
  const text = over([fg[0], fg[1], fg[2], fg[3] * fade(el)], bg);
  const [l1, l2] = [luminance(text), luminance(bg)].sort((x, y) => y - x);
  const ratio = (l1 + 0.05) / (l2 + 0.05);
  const size = parseFloat(style.fontSize);
  const large = size >= 24 || (size >= 18.66 && parseInt(style.fontWeight, 10) >= 700);
  const needed = large ? 3 : 4.5;
  if (ratio >= needed) return null;
  return `contrast ${ratio.toFixed(2)}:1, needs ${needed}:1`;
}

const RULES: Rule[] = [
  {
    id: 'document-title', wcag: '2.4.2', level: 'A', help: 'the page has no <title>',
    page: (doc) => (doc.title.trim() ? null : 'the page has no <title>'),
  },
  {
    id: 'html-has-lang', wcag: '3.1.1', level: 'A', help: '<html> has no lang attribute',
    page: (doc) => (doc.documentElement.getAttribute('lang')?.trim() ? null : '<html> has no lang attribute'),
  },
  {
    id: 'image-alt', wcag: '1.1.1', level: 'A', help: 'has no alt text',
    check: (el) => tag(el) === 'img' && roleOf(el) === 'img' && !el.hasAttribute('alt') && !nameOf(el, 'img'),
  },
  {
    id: 'input-image-alt', wcag: '1.1.1', level: 'A', help: 'is an image button with no alt text',
    check: (el) => tag(el) === 'input' && (el as HTMLInputElement).type === 'image'
      && !(el.getAttribute('alt') || el.getAttribute('aria-label') || el.getAttribute('title') || '').trim(),
  },
  {
    id: 'button-name', wcag: '4.1.2', level: 'A', help: 'is a button with no name',
    check: (el) => roleOf(el) === 'button' && tag(el) !== 'input' && !nameOf(el, 'button'),
  },
  {
    id: 'link-name', wcag: '2.4.4', level: 'A', help: 'is a link with no name',
    check: (el) => roleOf(el) === 'link' && !nameOf(el, 'link'),
  },
  {
    id: 'label', wcag: '4.1.2', level: 'A', help: 'is a form field with no label',
    check: (el) => field(el) && !nameOf(el, roleOf(el) || 'textbox'),
  },
  {
    id: 'frame-title', wcag: '4.1.2', level: 'A', help: 'is a frame with no title',
    check: (el) => (tag(el) === 'iframe' || tag(el) === 'frame') && !nameOf(el, 'iframe'),
  },
  {
    id: 'aria-hidden-focus', wcag: '4.1.2', level: 'A', help: 'can take focus inside aria-hidden="true"',
    check: (el) => focusable(el) && !!el.closest('[aria-hidden="true"]'),
  },
  {
    id: 'aria-valid-role', wcag: '4.1.2', level: 'A', help: 'has a role that is not an ARIA role',
    check: (el) => {
      const role = explicitRole(el);
      if (!role || ROLES.has(role)) return false;
      const near = closest(role, ROLES);
      return `role="${el.getAttribute('role')}"${near ? ` (did you mean ${near}?)` : ''}`;
    },
  },
  {
    id: 'aria-valid-attr', wcag: '4.1.2', level: 'A', help: 'has an aria-* attribute that does not exist',
    check: (el) => {
      const bad = el.getAttributeNames().find((n) => n.startsWith('aria-') && !(n in ATTRS));
      if (!bad) return false;
      const near = closest(bad, Object.keys(ATTRS));
      return `${bad}${near ? ` (did you mean ${near}?)` : ''}`;
    },
  },
  {
    id: 'aria-valid-attr-value', wcag: '4.1.2', level: 'A', help: 'has an aria-* attribute with a wrong value',
    check: (el) => {
      for (const name of el.getAttributeNames()) {
        if (!(name in ATTRS)) continue;
        // What a collapsed control controls may not be there yet.
        const lenient = name === 'aria-controls' && el.getAttribute('aria-expanded') === 'false';
        const why = badValue(name, el.getAttribute(name) || '', (id) => lenient || hasId(el, id));
        if (why) return why;
      }
      return false;
    },
  },
  {
    id: 'aria-required-attr', wcag: '4.1.2', level: 'A', help: 'has a role without an attribute it needs',
    check: (el) => {
      const role = explicitRole(el);
      const needs = role ? REQUIRED_ATTRS[role] : undefined;
      if (!role || !needs) return false;
      // Native elements have the state themselves: a checkbox's checked,
      // a range's value, an <h2>'s level, a <select>'s expanded.
      const t = tag(el);
      const type = ((el as HTMLInputElement).type || '').toLowerCase();
      if (t === 'input' && ['checkbox', 'radio', 'range', 'number'].includes(type)) return false;
      if (['select', 'meter', 'progress', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6'].includes(t)) return false;
      const missing = needs.filter((attr) => !el.hasAttribute(attr));
      return missing.length ? `role=${role} needs ${missing.join(' and ')}` : false;
    },
  },
  {
    id: 'aria-required-children', wcag: '1.3.1', level: 'A', help: 'owns elements its role does not allow',
    check: (el, ctx) => {
      const role = explicitRole(el);
      const allowed = role ? REQUIRED_CHILDREN[role] : undefined;
      // A native element with its own role (<ul role=list>) is the
      // `list` rule's, or Chrome's.
      if (!role || !allowed || roleOf(el) !== role || implicitRole(el) === role) return false;
      const bad = ownedOf(el, ctx).find((o) => !allowed.includes(roleOf(o) || ''));
      if (!bad) return false;
      return `owns ${describeShort(bad).replace(/^(\w+)/, (r) => a(r))}; ${a(role)} owns ${either(allowed)}`;
    },
  },
  {
    id: 'aria-required-parent', wcag: '1.3.1', level: 'A', help: 'is not inside the role it needs',
    check: (el, ctx) => {
      const role = explicitRole(el) || (tag(el) === 'li' ? 'listitem' : null);
      const allowed = role ? REQUIRED_PARENT[role] : undefined;
      if (!role || !allowed || roleOf(el) !== role) return false;
      const parent = el.parentElement;
      if (!explicitRole(el) && parent && ['ul', 'ol', 'menu'].includes(tag(parent))) return false;
      for (let o = owner(el, ctx); o; o = owner(o, ctx)) {
        const r = roleOf(o);
        if (!r || TRANSPARENT.has(r)) continue;
        return allowed.includes(r) ? false : `${a(role)} needs ${a(either(allowed))} parent, not ${a(r)}`;
      }
      return `${a(role)} needs ${a(either(allowed))} parent`;
    },
  },
  {
    id: 'duplicate-id-referenced', wcag: '4.1.2', level: 'A',
    help: 'has the id of an earlier element, which something refers to',
    check: (el, ctx) => {
      const id = el.id;
      if (!id) return false;
      const { byId, referred } = ctx.ids(el.getRootNode() as Document | ShadowRoot);
      const same = byId.get(id) || [];
      if (same.length < 2 || same[0] === el || !referred.has(id)) return false;
      return `id "${id}" (on ${same.length} elements; ${referred.get(id)} refers to it)`;
    },
  },
  {
    id: 'list', wcag: '1.3.1', level: 'A', help: 'is a list with children that are not <li>',
    check: (el) => {
      if ((tag(el) !== 'ul' && tag(el) !== 'ol') || roleOf(el) !== 'list') return false;
      const bad = Array.from(el.children).find((c) => !['li', 'script', 'template'].includes(tag(c)));
      return bad ? `has a <${tag(bad)}>` : false;
    },
  },
  {
    id: 'table-headers', wcag: '1.3.1', level: 'A', help: 'is a data table with no header cells', ax: 'data-table',
    check: (el) => {
      if (tag(el) !== 'table' || roleOf(el) !== 'table') return false;
      const mine = (e: Element) => e.closest('table') === el;
      if (!Array.from(el.querySelectorAll('td')).some(mine)) return false;
      return !Array.from(el.querySelectorAll('th, [role=columnheader], [role=rowheader]')).some(mine);
    },
  },
  {
    id: 'heading-order', wcag: '1.3.1', level: 'A', help: 'skips a heading level',
    check: (el, ctx) => {
      const previous = ctx.previousLevels().get(el);
      const level = headingLevel(el, 'heading') || 2;
      return previous !== undefined && level > previous + 1 ? `level ${previous} → ${level}` : false;
    },
  },
  {
    id: 'empty-heading', wcag: '1.3.1', level: 'A', help: 'is a heading with no text', ax: 'no-name',
    check: (el) => roleOf(el) === 'heading' && !nameOf(el, 'heading'),
  },
  {
    id: 'landmark-one-main', wcag: '1.3.1', level: 'A', help: 'the page needs one main landmark',
    page: (_doc, ctx) => {
      const mains = ctx.shown().filter((el) => roleOf(el) === 'main' && visible(el));
      if (mains.length === 1) return null;
      return mains.length ? `the page has ${mains.length} visible main landmarks` : 'the page has no main landmark';
    },
  },
  {
    id: 'meta-viewport', wcag: '1.4.4', level: 'AA', help: 'the viewport meta tag stops zooming',
    page: (doc) => {
      const meta = doc.querySelector('meta[name="viewport"]');
      const content = (meta?.getAttribute('content') || '').toLowerCase().replace(/\s/g, '');
      if (/user-scalable=(no|0)(,|$)/.test(content)) return 'the viewport meta tag has user-scalable=no';
      const max = /maximum-scale=([\d.]+)/.exec(content);
      if (max && parseFloat(max[1]) < 2) return `the viewport meta tag has maximum-scale=${max[1]}`;
      return null;
    },
  },
  {
    id: 'color-contrast', wcag: '1.4.3', level: 'AA', help: 'has text with too little contrast',
    check: (el) => contrast(el) || false,
  },
  {
    id: 'autocomplete-valid', wcag: '1.3.5', level: 'AA', help: 'has an autocomplete value browsers do not know',
    check: (el) => {
      if (!['input', 'select', 'textarea'].includes(tag(el))) return false;
      const value = el.getAttribute('autocomplete');
      return value ? badAutocomplete(value) || false : false;
    },
  },
  {
    id: 'target-size', wcag: '2.5.8', level: 'AA', help: 'is a pointer target under 24×24px, close to another',
    check: (el, ctx) => targetSize(el, ctx) || false,
  },
  {
    id: 'focus-visible', wcag: '2.4.7', level: 'AA', help: 'looks the same when it has keyboard focus',
    check: (el, ctx) => noFocusChange(el, ctx),
  },
];

/** How many elements a violation lists; for a rule Chrome's tree has
 * the last word on, how many candidates; and how many in all (the
 * engine's `PER_RULE`, `AX_LISTED` and `ELEMENTS`). */
const LISTED = 5;
const AX_LISTED = 20;
const ELEMENTS = 200;

interface Violation {
  rule: string;
  wcag: string;
  level: string;
  help: string;
  count: number;
  /** Page rules: what is wrong. */
  page?: string;
  /** What Chrome's accessibility tree must confirm (see `Rule.ax`). */
  ax?: string;
  nodes: { candidate: Candidate; why?: string; index: number }[];
}

/** The focused element before the last check of a page's document, to
 * give focus back to (`a11yRefocus`) once its iframes are checked too. */
let focusBefore: Element | null = null;

/** Gives focus back to `el`, or to nothing. */
function refocus(el: Element | null) {
  if (el && el !== document.body && el !== document.documentElement) {
    (el as HTMLElement).focus({ preventScroll: true });
  } else if (document.activeElement && document.activeElement !== document.body) {
    (document.activeElement as HTMLElement).blur();
  }
}

/** Checks the rules of `level` in `root` (the document when `null`):
 * `{json, els}` with `json` `{rules, violations}` and the listed
 * elements in `els` (each node's `index`), for refs. Page rules (the
 * title, `lang`, the viewport, the main landmark) only check a page's
 * document (`page`), not an iframe's. `focus-visible` focuses elements
 * one by one; focus goes back where it was. Each rule lists up to
 * `perRule` elements (`audit` asks for more than `expect --a11y`). */
export function a11y(
  root: Element | null, level: 'A' | 'AA', page = true, perRule = LISTED,
): { json: string; els: Element[] } {
  const rules = RULES.filter((r) => level === 'AA' || r.level === 'A');
  const violations: Violation[] = [];
  const els: Element[] = [];
  const scope = root || document.documentElement;
  const all = [scope, ...descendants(scope)];
  const hidden = new Map(all.map((el) => [el, hiddenForAria(el)]));
  const ctx = new Context(all, hidden);
  const before = activeElement();
  if (page) focusBefore = before;
  for (const rule of rules) {
    const v: Violation = { rule: rule.id, wcag: rule.wcag, level: rule.level, help: rule.help, count: 0, nodes: [] };
    if (rule.ax) v.ax = rule.ax;
    if (rule.page) {
      if (root || !page) continue;
      const why = rule.page(document, ctx);
      if (why) violations.push({ ...v, count: 1, page: why });
      continue;
    }
    const listed = rule.ax ? Math.max(AX_LISTED, perRule) : perRule;
    for (const el of all) {
      // Only aria-hidden-focus looks at what is hidden from assistive tech.
      if (rule.id === 'aria-hidden-focus' ? !hidden.get(el) || !visible(el) : hidden.get(el)) continue;
      const found = rule.check?.(el, ctx);
      if (!found) continue;
      v.count++;
      if (v.nodes.length < listed && els.length < ELEMENTS) {
        v.nodes.push({ candidate: describe(el), why: typeof found === 'string' ? found : undefined, index: els.length });
        els.push(el);
      }
    }
    if (v.count) violations.push(v);
  }
  if (ctx.focused) refocus(before);
  return { json: JSON.stringify({ rules: rules.length, violations }), els };
}

/** Gives focus back to where it was before the page's document was
 * checked: checking an iframe's `focus-visible` moves the page's focus
 * into the iframe. */
export function a11yRefocus() {
  refocus(focusBefore);
}
