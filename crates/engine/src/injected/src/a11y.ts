// `expect --a11y` (plan 09, milestone 9.1): a small built-in set of
// accessibility rules, checked in the page's DOM. It is not axe-core (see
// plan 09's decisions log); rule ids are axe's, so results read the same.
// Each rule has its WCAG success criterion and level; `--level A` checks
// the level A rules, `--level AA` (the default) all of them.

import { disabled } from './actionable';
import { hiddenForAria, nameOf, roleOf } from './aria';
import { descendants, tag, visible } from './dom';
import { Candidate, describe } from './locator';

interface Rule {
  id: string;
  wcag: string;
  level: 'A' | 'AA';
  /** What is wrong, for the output: `has no alt text`. */
  help: string;
  /** Page rules check the document once, not elements. */
  page?: (doc: Document) => string | null;
  check?: (el: Element) => string | boolean;
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

/** Text too faint for its size: `null`, or why (the ratio found). */
function contrast(el: Element): string | null {
  const own = Array.from(el.childNodes).some((n) => n.nodeType === Node.TEXT_NODE && (n as Text).data.trim());
  if (!own || !visible(el) || disabled(el)) return null;
  const style = getComputedStyle(el);
  const fg = parseColour(style.color);
  const bg = background(el);
  if (!fg || !bg) return null;
  const text = over(fg, bg);
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
];

/** How many elements a violation lists. */
const LISTED = 5;

interface Violation {
  rule: string;
  wcag: string;
  level: string;
  help: string;
  count: number;
  /** Page rules: what is wrong. */
  page?: string;
  nodes: { candidate: Candidate; why?: string; index: number }[];
}

/** Checks the rules of `level` in `root` (the document when `null`):
 * `{json, els}` with `json` `{rules, violations}` and the listed
 * elements in `els` (each node's `index`), for refs. Page rules (the
 * title, `lang`, the viewport) only check a page's document (`page`),
 * not an iframe's. */
export function a11y(root: Element | null, level: 'A' | 'AA', page = true): { json: string; els: Element[] } {
  const rules = RULES.filter((r) => level === 'AA' || r.level === 'A');
  const violations: Violation[] = [];
  const els: Element[] = [];
  const scope = root || document.documentElement;
  const all = [scope, ...descendants(scope)];
  const hidden = new Map(all.map((el) => [el, hiddenForAria(el)]));
  for (const rule of rules) {
    const v: Violation = { rule: rule.id, wcag: rule.wcag, level: rule.level, help: rule.help, count: 0, nodes: [] };
    if (rule.page) {
      if (root || !page) continue;
      const why = rule.page(document);
      if (why) violations.push({ ...v, count: 1, page: why });
      continue;
    }
    for (const el of all) {
      // Only aria-hidden-focus looks at what is hidden from assistive tech.
      if (rule.id === 'aria-hidden-focus' ? !hidden.get(el) || !visible(el) : hidden.get(el)) continue;
      const found = rule.check?.(el);
      if (!found) continue;
      v.count++;
      if (v.nodes.length < LISTED) {
        v.nodes.push({ candidate: describe(el), why: typeof found === 'string' ? found : undefined, index: els.length });
        els.push(el);
      }
    }
    if (v.count) violations.push(v);
  }
  return { json: JSON.stringify({ rules: rules.length, violations }), els };
}
