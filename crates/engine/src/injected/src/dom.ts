// Walking the DOM the way a user sees it: into open shadow roots and
// slots, in composed-tree order.

/** Elements whose content is never shown or read. */
export const SKIP = new Set(['script', 'style', 'template', 'noscript', 'head', 'meta', 'link', 'title']);

/** Collapses whitespace, as rendered text does. */
export const norm = (s: unknown): string => String(s == null ? '' : s).replace(/\s+/g, ' ').trim();

export const tag = (el: Element): string => el.localName;

export const attr = (el: Element, name: string): string | null => el.getAttribute(name);

/** What an element shows: its shadow root's nodes, a slot's assigned
 * nodes, a closed `<details>`'s summary, else its children. */
export function childrenOf(el: Element): Node[] {
  if (el.shadowRoot) return Array.from(el.shadowRoot.childNodes);
  if (el instanceof HTMLSlotElement) {
    const assigned = el.assignedNodes({ flatten: true });
    return assigned.length ? assigned : Array.from(el.childNodes);
  }
  if (el instanceof HTMLDetailsElement && !el.open) {
    return Array.from(el.children).filter((c) => tag(c) === 'summary').slice(0, 1);
  }
  return Array.from(el.childNodes);
}

/** Every element under `scope` (not `scope` itself), in document order,
 * including the light DOM and open shadow roots of every host (the shadow
 * root first). Unlike `childrenOf`, slotted nodes are visited where they
 * are in the light DOM, once. */
export function descendants(scope: Node): Element[] {
  const out: Element[] = [];
  const visit = (node: Node) => {
    if (out.length > 500000) return;
    if (node instanceof Element) {
      if (node.shadowRoot) visit(node.shadowRoot);
    }
    for (let child = node.firstChild; child; child = child.nextSibling) {
      if (child.nodeType === Node.ELEMENT_NODE) {
        out.push(child as Element);
        visit(child);
      }
    }
  };
  visit(scope);
  return out;
}

/** The parent across shadow boundaries (a shadow root's host). */
export function parentOf(node: Node): Element | null {
  if (node.parentElement) return node.parentElement;
  const parent = node.parentNode;
  if (parent instanceof ShadowRoot) return parent.host;
  return null;
}

/** Whether `inner` is `outer` or inside it, across shadow boundaries. */
export function contains(outer: Node, inner: Node): boolean {
  for (let n: Node | null = inner; n; n = n instanceof Element ? parentOf(n) : n.parentNode) {
    if (n === outer) return true;
  }
  return false;
}

/** Visible: a non-empty box and not `visibility: hidden` (Playwright's
 * definition); `display: none` has no box. */
export function visible(el: Element): boolean {
  const box = el.getBoundingClientRect();
  if (box.width <= 0 || box.height <= 0) {
    // `display: contents` has no box of its own; its children show.
    const style = el.ownerDocument.defaultView?.getComputedStyle(el);
    if (style?.display === 'contents') return childrenOf(el).some((c) => c instanceof Element && visible(c));
    return false;
  }
  return el.checkVisibility ? el.checkVisibility({ visibilityProperty: true } as CheckVisibilityOptions) : true;
}

/** Sorts elements in document order (composed tree), dropping repeats. */
export function inOrder(els: Element[], root: Node): Element[] {
  const unique = Array.from(new Set(els));
  if (unique.length < 2) return unique;
  const order = new Map<Element, number>();
  descendants(root).forEach((el, i) => order.set(el, i));
  if (root instanceof Element) order.set(root, -1);
  return unique.sort((a, b) => (order.get(a) ?? 1e9) - (order.get(b) ?? 1e9));
}
