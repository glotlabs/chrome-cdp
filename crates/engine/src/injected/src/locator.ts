// Evaluates a locator's steps in one frame (plan 04, milestone 4.4). The
// engine resolves refs and `frame=` itself and sends the rest: each step
// either finds elements inside the current ones (`A >> B`: B inside A) or
// filters them (`nth=`, `visible=`, `has-text=`, `has-not-text=`, and,
// with an inner locator, `has=`, `has-not=`, `and=`; `or=` adds to them).

import { INTERACTIVE, activeElement, hiddenForAria, labelsOf, level, nameOf, roleOf, shownName, statesOf, textOf } from './aria';
import { css } from './css';
import { descendants, inOrder, norm, parentOf, tag, visible } from './dom';
import { LocatorError } from './errors';
import { TextMatch, Texts, matcher } from './text';

type Tri = 'true' | 'false' | 'mixed';

/** One step, as the engine sends it (`Step` in locator/parse.rs). */
export type Step =
  | { kind: 'css'; value: string }
  | { kind: 'xpath'; value: string }
  | { kind: 'text'; text: TextMatch }
  | {
    kind: 'role'; role: string; name?: TextMatch; checked?: Tri; disabled?: boolean;
    expanded?: boolean; level?: number; pressed?: Tri; selected?: boolean; includeHidden?: boolean;
  }
  | { kind: 'label'; text: TextMatch }
  | { kind: 'placeholder'; text: TextMatch }
  | { kind: 'alt'; text: TextMatch }
  | { kind: 'title'; text: TextMatch }
  | { kind: 'testid'; value: string; attribute: string }
  | { kind: 'nth'; index: number }
  | { kind: 'visible'; visible: boolean }
  | { kind: 'has-text'; text: TextMatch }
  | { kind: 'has-not-text'; text: TextMatch }
  | { kind: 'has' | 'has-not' | 'and' | 'or'; steps: Step[] };

type Scope = Document | Element;

function xpath(expression: string, scope: Scope): Element[] {
  // In a chain, `//x` means "under the scope", as in Playwright.
  const expr = scope instanceof Document || !expression.startsWith('/') ? expression : '.' + expression;
  let found: XPathResult;
  try {
    found = document.evaluate(expr, scope, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);
  } catch (e) {
    throw new LocatorError(`${expression} is not a valid XPath expression`);
  }
  const out: Element[] = [];
  for (let i = 0; i < found.snapshotLength; i++) {
    const node = found.snapshotItem(i);
    if (node instanceof Element) out.push(node);
  }
  return out;
}

/** `text=`: the innermost visible elements whose text matches: an
 * element counts unless a visible child element matches too. */
function byText(m: TextMatch, scope: Scope, texts: Texts): Element[] {
  const test = matcher(m);
  const matches = (el: Element) => test(texts.of(el));
  const out: Element[] = [];
  for (const el of descendants(scope)) {
    if (!matches(el) || !visible(el)) continue;
    const kids = Array.from((el.shadowRoot || el).children);
    if (kids.some((c) => matches(c) && visible(c))) continue;
    out.push(el);
  }
  return out;
}

function byRole(step: Extract<Step, { kind: 'role' }>, scope: Scope): Element[] {
  const name = step.name ? matcher(step.name) : null;
  const active = activeElement();
  const out: Element[] = [];
  for (const el of descendants(scope)) {
    const role = roleOf(el);
    if (role !== step.role) continue;
    if (!step.includeHidden && hiddenForAria(el)) continue;
    if (step.checked !== undefined || step.disabled !== undefined || step.expanded !== undefined
      || step.pressed !== undefined || step.selected !== undefined) {
      const s = statesOf(el, role, active);
      if (step.checked !== undefined && (s.c || 'false') !== step.checked) continue;
      if (step.disabled !== undefined && !!s.d !== step.disabled) continue;
      if (step.expanded !== undefined && s.x !== step.expanded) continue;
      if (step.selected !== undefined && !!s.s !== step.selected) continue;
      if (step.pressed !== undefined && (s.pm ? 'mixed' : s.p ? 'true' : 'false') !== step.pressed) continue;
    }
    if (step.level !== undefined && level(el, role) !== step.level) continue;
    if (name && !name(nameOf(el, role))) continue;
    out.push(el);
  }
  return out;
}

function byAttribute(name: string, m: TextMatch, scope: Scope): Element[] {
  const test = matcher(m);
  return descendants(scope).filter((el) => {
    const value = el.getAttribute(name);
    return value !== null && test(value);
  });
}

function find(step: Step, scope: Scope, texts: Texts): Element[] {
  switch (step.kind) {
    case 'css': return css(step.value, scope);
    case 'xpath': return xpath(step.value, scope);
    case 'text': return byText(step.text, scope, texts);
    case 'role': return byRole(step, scope);
    case 'label': {
      const test = matcher(step.text);
      return descendants(scope).filter((el) => labelsOf(el).some(test));
    }
    case 'placeholder': return byAttribute('placeholder', step.text, scope);
    case 'alt': return byAttribute('alt', step.text, scope);
    case 'title': return byAttribute('title', step.text, scope);
    case 'testid': return descendants(scope).filter((el) => el.getAttribute(step.attribute) === step.value);
    default: return [];
  }
}

/** The elements `steps` match under `root` (the document if `null`), in
 * document order. With no steps, `root` itself. */
export function query(steps: Step[], root: Element | null): Element[] {
  if (root && !root.isConnected) return [];
  return run(steps, root || document, new Texts());
}

/** The elements an inner locator matches in each of `scopes`, as a set. */
function inScopes(steps: Step[], scopes: Scope[], texts: Texts): Set<Element> {
  const out = new Set<Element>();
  for (const scope of scopes) for (const el of run(steps, scope, texts)) out.add(el);
  return out;
}

function run(steps: Step[], top: Scope, texts: Texts): Element[] {
  let current: Scope[] = [top];
  // Where the last finder step looked: `and=` and `or=` look there too.
  let scopes: Scope[] = [top];
  for (const step of steps) {
    const els = current.filter((s): s is Element => s instanceof Element);
    switch (step.kind) {
      case 'nth': {
        const i = step.index < 0 ? els.length + step.index : step.index;
        current = i >= 0 && i < els.length ? [els[i]] : [];
        break;
      }
      case 'visible':
        current = els.filter((el) => visible(el) === step.visible);
        break;
      case 'has-text':
      case 'has-not-text': {
        const test = matcher(step.text);
        const want = step.kind === 'has-text';
        current = els.filter((el) => test(texts.of(el)) === want);
        break;
      }
      case 'has':
      case 'has-not': {
        const want = step.kind === 'has';
        current = els.filter((el) => (run(step.steps, el, texts).length > 0) === want);
        break;
      }
      case 'and': {
        const also = inScopes(step.steps, scopes, texts);
        current = els.filter((el) => also.has(el));
        break;
      }
      case 'or':
        current = inOrder([...els, ...inScopes(step.steps, scopes, texts)], top);
        break;
      default: {
        const found: Element[] = [];
        for (const scope of current) found.push(...find(step, scope, texts));
        scopes = current;
        current = current.length > 1 ? inOrder(found, top) : found;
      }
    }
  }
  return current.filter((s): s is Element => s instanceof Element);
}

export const CONTEXT_ROLES = new Set(['row', 'listitem', 'article', 'dialog', 'alertdialog', 'form',
  'region', 'navigation', 'group', 'tabpanel', 'menu', 'listbox', 'radiogroup', 'tablist',
  'toolbar', 'banner', 'contentinfo', 'complementary', 'main', 'search', 'table', 'list']);

const cut = (s: string, n: number) => (s.length > n ? s.slice(0, n - 1) + '…' : s);

/** What an error shows of a candidate: its role and name, and the
 * nearest named container it is in (a row, a list item, a dialog, …). */
export interface Candidate {
  role: string;
  name: string;
  tag: string;
  context?: string;
  visible: boolean;
}

export function describe(el: Element): Candidate {
  const role = roleOf(el) || 'generic';
  // Without a name, what is not a control is known by its text.
  const name = cut(shownName(el, role) || (INTERACTIVE.has(role) ? '' : norm(el.textContent)), 60);
  const out: Candidate = { role, name, tag: tag(el), visible: visible(el) };
  for (let a = parentOf(el); a; a = parentOf(a)) {
    const r = roleOf(a);
    if (!r || !CONTEXT_ROLES.has(r)) continue;
    const n = cut(r === 'row' || r === 'listitem' ? norm(textOf(a, null, 0)) : nameOf(a, r), 40);
    if (!n) continue;
    out.context = `${r} "${n.replace(/"/g, '\\"')}"`;
    break;
  }
  return out;
}

/** How many candidates an ambiguity error lists. */
const LISTED = 5;

/** The one element `steps` match under `root`: the element itself, or,
 * when there is not exactly one, `{json, els}` with the count and the
 * first few matches, described. */
export function one(steps: Step[], root: Element | null): Element | { json: string; els: Element[] } {
  const found = query(steps, root);
  if (found.length === 1) return found[0];
  const els = found.slice(0, LISTED);
  const out: { count: number; candidates: Candidate[]; beforeHas?: number } = {
    count: found.length, candidates: els.map(describe),
  };
  // None found by a last `has=`: how many it had to choose from.
  if (found.length === 0 && steps.length > 1 && steps[steps.length - 1].kind === 'has') {
    out.beforeHas = query(steps.slice(0, -1), root).length;
  }
  return { json: JSON.stringify(out), els };
}
