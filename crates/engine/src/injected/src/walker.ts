// chrome-cdp's DOM walker (plan 04, milestone 4.3, engine B). It walks
// the frame's document (open shadow roots and slotted content included),
// works out each visible node's ARIA role, accessible name, value and
// states, and returns
//   {json: '{"title","url","nodes":[…]}', els: [Element, …]}
// `nodes` is flat, in document order; each has `p` (its parent's index,
// -1 at the top). Nodes the engine must address (controls, clickable and
// scrollable elements, iframes, and `scope`) point into `els` with `e`, so
// the engine can get their backendNodeIds with DOM.describeNode.

import {
  INTERACTIVE, States, activeElement, hidden, level, roleOf, shownName, statesOf, valueOf,
} from './aria';
import { SKIP, attr, childrenOf, tag } from './dom';

interface Flat {
  p: number;
  t?: 1;
  r?: string;
  n?: string;
  v?: string;
  l?: number;
  h?: string;
  w?: 1;
  s?: States;
  i?: 1;
  c?: 1;
  x?: 1;
  e?: number;
}

function scrolls(el: Element, style: CSSStyleDeclaration): boolean {
  if (tag(el) === 'html' || tag(el) === 'body') return false;
  const over = (v: string) => v === 'auto' || v === 'scroll' || v === 'overlay';
  return (over(style.overflowY) && el.scrollHeight - el.clientHeight > 1)
    || (over(style.overflowX) && el.scrollWidth - el.clientWidth > 1);
}

export function walk(scope: Element | null): { json: string; els: Element[] } {
  const view = document.defaultView as Window;
  const nodes: Flat[] = [];
  const els: Element[] = [];
  const active = activeElement();

  // `invisible`: text right here is not shown (its element is visibility:
  // hidden). `mute`: text anywhere below is not shown: it is in a label
  // that names its control, so the control's name says it.
  function visit(node: Node, parent: number, parentStyle: CSSStyleDeclaration | null, invisible: boolean, mute: boolean) {
    if (nodes.length > 200000) return;
    if (node.nodeType === Node.TEXT_NODE) {
      const data = (node as Text).data;
      if (!invisible && !mute && data) nodes.push({ p: parent, t: 1, n: data });
      return;
    }
    if (node.nodeType !== Node.ELEMENT_NODE) return;
    const el = node as Element;
    if (SKIP.has(tag(el))) return;
    if (tag(el) === 'br') {
      if (!invisible && !mute) nodes.push({ p: parent, t: 1, n: '\n' });
      return;
    }
    const style = view.getComputedStyle(el);
    if (hidden(el, style)) return;
    const role = roleOf(el);
    if (role === null) return;
    const vis = style.visibility !== 'hidden' && style.visibility !== 'collapse';
    const index = nodes.length;
    // An element that is visibility: hidden is not shown, but a child
    // that is visible again is.
    if (!vis) {
      for (const child of childrenOf(el)) visit(child, parent, style, true, mute);
      return;
    }
    const n: Flat = { p: parent, r: role };
    const name = shownName(el, role);
    if (name) n.n = name;
    const value = valueOf(el, role);
    if (value != null && value !== '') n.v = String(value);
    const lv = level(el, role);
    if (lv) n.l = lv;
    const href = (el as HTMLAnchorElement).href as unknown;
    if (role === 'link' && href) {
      const svg = href as SVGAnimatedString;
      n.h = String(svg.baseVal !== undefined ? svg.baseVal : href);
    }
    if (tag(el) === 'input' && (el as HTMLInputElement).type === 'password') n.w = 1;
    const s = statesOf(el, role, active);
    delete s.pm;
    if (Object.keys(s).length) n.s = s;
    if (style.display === 'inline' || style.display === 'contents') n.i = 1;
    const tabindex = parseInt(attr(el, 'tabindex') || '', 10);
    const pointerRoot = style.cursor === 'pointer' && !(parentStyle && parentStyle.cursor === 'pointer');
    if (tag(el) !== 'html' && tag(el) !== 'body'
      && (el.hasAttribute('onclick') || tabindex >= 0 || pointerRoot)) n.c = 1;
    if (scrolls(el, style)) n.x = 1;
    // Long lists get a key too: their collapsed tail names them.
    const long = el.childElementCount > 30
      || Array.from(el.children).some((c) => c.childElementCount > 30);
    if (INTERACTIVE.has(role) || n.c || n.x || el === scope || role === 'textbox' || long) {
      n.e = els.length;
      els.push(el);
    }
    nodes.push(n);
    if (role === 'iframe' || tag(el) === 'svg' || role === 'img') return;
    const muted = mute || (tag(el) === 'label' && !!(el as HTMLLabelElement).control);
    for (const child of childrenOf(el)) visit(child, index, style, false, muted);
  }

  if (document.documentElement) visit(document.documentElement, -1, null, false, false);
  return {
    json: JSON.stringify({ title: document.title, url: location.href, nodes }),
    els,
  };
}
