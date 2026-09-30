// `layout` (plan 16, milestone 16.2): what looks broken on the page,
// read from geometry and computed styles. One call checks one frame's
// document (or the subtree of `root`) for the kinds the engine asks for;
// the engine moves boxes into the tab's viewport, measures contrast on
// screenshots (with the text made transparent by `hideText`), and runs
// the whole check again per viewport and with stretched text.
//
// Boxes are `[x, y, width, height]` in this frame's viewport (CSS px).

import { INTERACTIVE, openModal, outsideModal, roleOf } from './aria';
import { covering, frame, named, Named } from './actionable';
import { SKIP, contains, descendants, norm, parentOf, tag } from './dom';
import { stable } from './stable';

type Box = [number, number, number, number];

/** What the engine asks of one call. */
interface Options {
  /** The kinds to check: `covered`, `overlap`, `clipped` (and
   * `truncated`), `overflow-x`, `offscreen`, `small-target`, `contrast`. */
  kinds: string[];
  /** The least width and height of a target (WCAG 2.5.8: 24). */
  minTarget: number;
  /** The tab's main frame: the only one `overflow-x` is about. */
  main: boolean;
  /** The part of this frame's viewport the tab shows (`covered` and
   * `contrast` look only there); `null` for all of it. */
  view: Box | null;
  /** The attribute `testid=` reads, for the findings' keys. */
  testid: string;
  /** At most this many text elements are measured for contrast. */
  texts: number;
}

/** A finding: `el` and `others` are indexes into the answer's `els`. */
interface Finding {
  kind: string;
  el: number;
  box: Box;
  others?: number[];
  details: Record<string, unknown>;
}

/** Text to measure on the screenshots: `colour` is `[r, g, b, a]` with
 * the opacity of the element and its ancestors in `a`. */
interface Measure {
  box: Box;
  colour: [number, number, number, number];
  large: boolean;
}

const px = (n: number) => Math.round(n);
const boxOf = (r: DOMRect): Box => [r.x, r.y, r.width, r.height];

/** Collects the elements an answer names, each with its name and key. */
class Els {
  els: Element[] = [];
  add(el: Element): number {
    const i = this.els.indexOf(el);
    return i >= 0 ? i : this.els.push(el) - 1;
  }
}

/** The key of a finding's element: its stable locator (`stable.ts`, what
 * `history --as-scenario` writes), else a short CSS path. */
function keyOf(el: Element, testid: string): string {
  try {
    const found = JSON.parse(stable(el, testid)) as { locator?: string };
    if (found.locator) return found.locator;
  } catch {
    // Fall back to the path.
  }
  const parts: string[] = [];
  for (let a: Element | null = el; a && parts.length < 5; a = parentOf(a)) {
    const t = tag(a);
    if (t === 'html' || t === 'body') break;
    const id = a.getAttribute('id');
    if (id && /^[A-Za-z][\w-]*$/.test(id)) {
      parts.unshift(`#${id}`);
      break;
    }
    const cls = norm(a.getAttribute('class')).split(' ')[0];
    let part = cls && /^[A-Za-z_][\w-]*$/.test(cls) ? `${t}.${cls}` : t;
    const parent = a.parentElement;
    if (parent) {
      const same = Array.from(parent.children).filter((c) => tag(c) === t);
      if (same.length > 1) part += `:nth-of-type(${same.indexOf(a) + 1})`;
    }
    parts.unshift(part);
  }
  return `css=${parts.join(' > ')}`;
}

/** Elements that can be scrolled to: not `hidden` or `clip`. */
const clips = (o: string) => o === 'hidden' || o === 'clip';
const scrolls = (o: string) => o === 'auto' || o === 'scroll' || o === 'overlay';

/** An element's padding box: what its `overflow` clips to. */
function paddingBox(el: Element): Box {
  const r = el.getBoundingClientRect();
  return [r.x + el.clientLeft, r.y + el.clientTop, el.clientWidth, el.clientHeight];
}

function intersect(a: Box, b: Box): Box | null {
  const x = Math.max(a[0], b[0]);
  const y = Math.max(a[1], b[1]);
  const right = Math.min(a[0] + a[2], b[0] + b[2]);
  const bottom = Math.min(a[1] + a[3], b[1] + b[3]);
  return right > x && bottom > y ? [x, y, right - x, bottom - y] : null;
}

const area = (b: Box) => b[2] * b[3];
const centre = (b: Box): [number, number] => [b[0] + b[2] / 2, b[1] + b[3] / 2];

/** `box` less what the ancestors of `el` with `overflow` other than
 * `visible` clip away (`html` and `body` clip to the viewport, which is
 * not counted); `null` when nothing of it shows. */
function clipBox(el: Element, box: Box | null): Box | null {
  for (let a = parentOf(el); a && box; a = parentOf(a)) {
    const t = tag(a);
    if (t === 'html' || t === 'body') break;
    const s = getComputedStyle(a);
    if (s.overflowX === 'visible' && s.overflowY === 'visible') continue;
    const p = paddingBox(a);
    const limit: Box = [
      s.overflowX === 'visible' ? -1e9 : p[0],
      s.overflowY === 'visible' ? -1e9 : p[1],
      s.overflowX === 'visible' ? 2e9 : p[2],
      s.overflowY === 'visible' ? 2e9 : p[3],
    ];
    box = intersect(box, limit);
    if (s.position === 'fixed') break;
  }
  return box;
}

/** The boxes of `el` that show: an inline element that wraps has one per
 * line (its bounding box would take in what lies between), anything else
 * one; each less what ancestors clip away. Empty when none shows. */
function shownBoxes(el: Element, style: CSSStyleDeclaration): Box[] {
  const rects = style.display === 'inline' ? Array.from(el.getClientRects()) : [];
  const boxes = rects.length > 1 ? rects.map(boxOf) : [boxOf(el.getBoundingClientRect())];
  return boxes
    .filter((b) => b[2] >= 1 && b[3] >= 1)
    .map((b) => clipBox(el, b))
    .filter((b): b is Box => !!b);
}

const largest = (boxes: Box[]) => boxes.reduce((a, b) => (area(b) > area(a) ? b : a));

/** How far a point is from a box (0 inside it). */
const reach = (p: [number, number], b: Box) =>
  Math.hypot(Math.max(b[0] - p[0], 0, p[0] - b[0] - b[2]), Math.max(b[1] - p[1], 0, p[1] - b[1] - b[3]));

/** Visually hidden on purpose (the `.sr-only` pattern): 1 px or less, or
 * clipped to nothing by `clip` or `clip-path`. */
function visuallyHidden(el: Element, style: CSSStyleDeclaration): boolean {
  const r = el.getBoundingClientRect();
  if (r.width <= 1 || r.height <= 1) return true;
  const clip = style.clip.replace(/\s+/g, '');
  if (style.position === 'absolute' || style.position === 'fixed') {
    const m = /^rect\(([-\d.]+)px,?([-\d.]+)px,?([-\d.]+)px,?([-\d.]+)px\)$/.exec(clip);
    if (m && (parseFloat(m[3]) - parseFloat(m[1]) <= 1 || parseFloat(m[2]) - parseFloat(m[4]) <= 1)) return true;
  }
  const path = style.clipPath.replace(/\s+/g, '');
  return /^inset\((50|100)%\)$/.test(path) || /^circle\(0(px)?/.test(path) || /^polygon\(0(px)?0(px)?,0(px)?0(px)?/.test(path);
}

/** What a check needs to know of each element, worked out once. */
interface Seen {
  el: Element;
  style: CSSStyleDeclaration;
  role: string;
  interactive: boolean;
}

/** The rendered elements under `root`: a box of at least 1×1 px, not
 * `display: none`, `visibility: visible`, and no ancestor at
 * `opacity: 0`; with whether each is interactive (what the snapshot gives
 * refs to by default: interactive roles and clickables, not iframes). */
function rendered(root: Element | Document): Seen[] {
  const out: Seen[] = [];
  const clear = new Map<Element, boolean>();
  const transparent = (el: Element | null): boolean => {
    if (!el) return false;
    const known = clear.get(el);
    if (known !== undefined) return known;
    const value = getComputedStyle(el).opacity === '0' || transparent(parentOf(el));
    clear.set(el, value);
    return value;
  };
  const all = root instanceof Document ? descendants(root) : [root, ...descendants(root)];
  const modal = openModal(document);
  for (const el of all) {
    const t = tag(el);
    if (SKIP.has(t) || t === 'html' || t === 'body') continue;
    const style = getComputedStyle(el);
    if (style.display === 'none' || style.visibility !== 'visible') continue;
    const r = el.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) continue;
    if (transparent(el)) continue;
    const role = roleOf(el) || 'generic';
    const parentStyle = parentOf(el) ? getComputedStyle(parentOf(el) as Element) : null;
    const tabindex = parseInt(el.getAttribute('tabindex') || '', 10);
    const pointer = style.cursor === 'pointer' && !(parentStyle && parentStyle.cursor === 'pointer');
    const clickable = el.hasAttribute('onclick') || tabindex >= 0 || pointer;
    // Inert (by `inert`, or a modal dialog open elsewhere): no target.
    const inert = !!el.closest('[inert]') || outsideModal(el, modal);
    const interactive = role !== 'iframe' && !inert && (INTERACTIVE.has(role) || clickable);
    out.push({ el, style, role, interactive });
  }
  return out;
}

/** The element at a point, into open shadow roots. */
function hitAt(doc: Document, x: number, y: number): Element | null {
  let hit = doc.elementFromPoint(x, y);
  while (hit && hit.shadowRoot) {
    const inner = hit.shadowRoot.elementFromPoint(x, y);
    if (!inner || inner === hit) break;
    hit = inner;
  }
  return hit;
}

/** The element's own text: its text node children with anything but
 * white space. */
function ownText(el: Element): Text[] {
  const nodes: Text[] = [];
  for (let n = el.firstChild; n; n = n.nextSibling) {
    if (n.nodeType === Node.TEXT_NODE && norm((n as Text).data)) nodes.push(n as Text);
  }
  return nodes;
}

/** The box of the element's own text: the union of its text nodes'. */
function textBox(el: Element): Box | null {
  const nodes = ownText(el);
  if (!nodes.length) return null;
  let left = Infinity;
  let top = Infinity;
  let right = -Infinity;
  let bottom = -Infinity;
  const range = document.createRange();
  for (const node of nodes) {
    range.selectNodeContents(node);
    for (const r of Array.from(range.getClientRects())) {
      if (r.width < 1 || r.height < 1) continue;
      left = Math.min(left, r.left);
      top = Math.min(top, r.top);
      right = Math.max(right, r.right);
      bottom = Math.max(bottom, r.bottom);
    }
  }
  range.detach();
  return right > left && bottom > top ? [left, top, right - left, bottom - top] : null;
}

/** `rgb(…)`/`rgba(…)` as `[r, g, b, a]`. */
function colour(s: string): [number, number, number, number] | null {
  const m = /^rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)(?:\s*[,/]\s*([\d.]+%?))?\s*\)$/.exec(s.trim());
  if (!m) return null;
  let a = m[4] === undefined ? 1 : parseFloat(m[4]);
  if (m[4] && m[4].endsWith('%')) a /= 100;
  return [parseFloat(m[1]), parseFloat(m[2]), parseFloat(m[3]), a];
}

/** The product of the element's and its ancestors' opacity. */
function opacity(el: Element): number {
  let o = 1;
  for (let a: Element | null = el; a; a = parentOf(a)) o *= parseFloat(getComputedStyle(a).opacity) || 0;
  return o;
}

/** Form controls whose size the browser picks when the page does not
 * style them. */
const NATIVE = new Set(['button', 'input', 'select', 'textarea']);

/** Unstyled controls to compare with: made on demand in a closed shadow
 * root, where the page's rules do not reach, and removed after. */
class Native {
  host: HTMLElement | null = null;
  root: ShadowRoot | null = null;
  sizes = new Map<string, DOMRect>();

  /** Whether `el`'s small sides (under `min`) are the size an unstyled
   * control of its kind has: the browser sized them (WCAG 2.5.8's
   * exception), as it does the options of a native select. */
  unstyled(el: Element, box: Box, min: number): boolean {
    const t = tag(el);
    if (t === 'option') return !!el.closest('select');
    if (!NATIVE.has(t)) return false;
    const ref = this.size(el);
    if (!ref) return false;
    const width = box[2] >= min || Math.abs(box[2] - ref.width) <= 1;
    const height = box[3] >= min || Math.abs(box[3] - ref.height) <= 1;
    return width && height;
  }

  size(el: Element): DOMRect | null {
    const t = tag(el);
    const type = t === 'input' ? (el as HTMLInputElement).type : '';
    const multiple = t === 'select' ? `${(el as HTMLSelectElement).multiple}:${(el as HTMLSelectElement).size}` : '';
    const text = t === 'button' ? norm(el.textContent) : '';
    const key = `${t}:${type}:${multiple}:${text}`;
    const known = this.sizes.get(key);
    if (known) return known;
    if (!this.root) {
      this.host = document.createElement('div');
      this.host.style.cssText = 'position: absolute; left: -10000px; top: 0; visibility: hidden';
      this.root = this.host.attachShadow({ mode: 'closed' });
      (document.body || document.documentElement).append(this.host);
    }
    const copy = document.createElement(t);
    if (type) copy.setAttribute('type', type);
    if (text) copy.textContent = text;
    if (t === 'select') {
      const select = el as HTMLSelectElement;
      (copy as HTMLSelectElement).multiple = select.multiple;
      (copy as HTMLSelectElement).size = select.size;
      for (const o of Array.from(select.options)) copy.append(new Option(o.text));
    }
    this.root.append(copy);
    const size = copy.getBoundingClientRect();
    this.sizes.set(key, size);
    return size;
  }

  remove(): void {
    this.host?.remove();
    this.host = null;
    this.root = null;
  }
}

/** The elements `measure` returned for contrast, until `textNames`. */
let measured: Element[] = [];

/** Checks one frame for the kinds in `options`, under `root` (the whole
 * document when `null`). `{json, els}`: `json` holds `findings`, the
 * `texts` to measure for contrast (with `skipped`, how many were past the
 * limit), the frame's `scroll`, and per element of `els` its
 * `names` and `keys`. */
export function layoutCheck(root: Element | null, options: Options): { json: string; els: Element[] } {
  const kinds = new Set(options.kinds);
  const els = new Els();
  const findings: Finding[] = [];
  const seen = rendered(root || document);
  const view: Box = options.view || [0, 0, innerWidth, innerHeight];
  const inView = (b: Box) => {
    const [x, y] = centre(b);
    return x >= view[0] && y >= view[1] && x < view[0] + view[2] && y < view[1] + view[3];
  };
  const targets = seen.filter((s) => s.interactive && !visuallyHidden(s.el, s.style));
  const shown = new Map<Element, Box[]>();
  const shownOf = (s: Seen) => {
    if (!shown.has(s.el)) shown.set(s.el, shownBoxes(s.el, s.style));
    return shown.get(s.el) as Box[];
  };

  if (kinds.has('covered')) {
    for (const t of targets) {
      const box = shownOf(t).find(inView);
      if (!box) continue;
      const [x, y] = centre(box);
      const hit = hitAt(t.el.ownerDocument, x, y);
      if (!hit || contains(t.el, hit) || contains(hit, t.el)) continue;
      const labels = (t.el as HTMLInputElement).labels;
      if (labels && Array.from(labels).some((l) => contains(l, hit))) continue;
      const found = covering(t.el, hit);
      const top = found.els[0];
      if (!top) continue;
      const others = [els.add(top)];
      if (found.els[1]) others.push(els.add(found.els[1]));
      findings.push({
        kind: 'covered',
        el: els.add(t.el),
        box: boxOf(t.el.getBoundingClientRect()),
        others,
        details: { at: [px(x), px(y)], dismiss: !!found.els[1] },
      });
    }
  }

  if (kinds.has('overlap')) {
    const boxes = targets
      .map((t) => {
        const parts = shownOf(t);
        const left = Math.min(...parts.map((b) => b[0]));
        const right = Math.max(...parts.map((b) => b[0] + b[2]));
        const size = parts.reduce((sum, b) => sum + area(b), 0);
        return { el: t.el, parts, left, right, size };
      })
      .filter((b) => b.parts.length)
      .sort((a, b) => a.left - b.left);
    for (let i = 0; i < boxes.length; i++) {
      const a = boxes[i];
      for (let j = i + 1; j < boxes.length && boxes[j].left < a.right; j++) {
        const b = boxes[j];
        let both = 0;
        let width = 0;
        let height = 0;
        for (const pa of a.parts) {
          for (const pb of b.parts) {
            const part = intersect(pa, pb);
            if (!part) continue;
            both += area(part);
            width = Math.max(width, part[2]);
            height = Math.max(height, part[3]);
          }
        }
        if (both <= 4 || both < 0.1 * Math.min(a.size, b.size)) continue;
        if (contains(a.el, b.el) || contains(b.el, a.el)) continue;
        // The first in document order is the one named.
        const [first, second] =
          a.el.compareDocumentPosition(b.el) & Node.DOCUMENT_POSITION_FOLLOWING ? [a, b] : [b, a];
        findings.push({
          kind: 'overlap',
          el: els.add(first.el),
          box: boxOf(first.el.getBoundingClientRect()),
          others: [els.add(second.el)],
          details: { width: px(width), height: px(height) },
        });
      }
    }
  }

  if (kinds.has('clipped') || kinds.has('truncated')) {
    for (const s of seen) {
      if (visuallyHidden(s.el, s.style)) continue;
      const text = textBox(s.el);
      if (!text) continue;
      for (let a: Element | null = s.el; a; a = parentOf(a)) {
        const t = tag(a);
        if (t === 'html' || t === 'body') break;
        const style = a === s.el ? s.style : getComputedStyle(a);
        const cutX = clips(style.overflowX);
        const cutY = clips(style.overflowY);
        if (!cutX && !cutY) {
          if (style.position === 'fixed') break;
          continue;
        }
        const p = paddingBox(a);
        const x = cutX && (text[0] < p[0] - 1 || text[0] + text[2] > p[0] + p[2] + 1);
        const y = cutY && (text[1] < p[1] - 1 || text[1] + text[3] > p[1] + p[3] + 1);
        if (!x && !y) {
          // A scroller shows the rest; an ancestor may still cut it.
          if (scrolls(style.overflowX) || scrolls(style.overflowY)) break;
          continue;
        }
        const both = intersect(text, p);
        // Nothing of it shows: hidden (a carousel's other slide), not cut.
        if (!both) break;
        const axis = x ? 'x' : 'y';
        const full = axis === 'x' ? text[2] : text[3];
        const part = axis === 'x' ? both[2] : both[3];
        const clamp = (st: CSSStyleDeclaration) =>
          ((st as unknown as { webkitLineClamp?: string }).webkitLineClamp || 'none') !== 'none';
        const how = axis === 'x' && style.textOverflow === 'ellipsis'
          ? 'ellipsis'
          : axis === 'y' && (clamp(style) || clamp(s.style)) ? 'line-clamp' : null;
        const kind = how ? 'truncated' : 'clipped';
        if (!kinds.has(kind)) break;
        const details: Record<string, unknown> = {
          axis,
          shown: px(part),
          full: px(full),
          overflow: axis === 'x' ? style.overflowX : style.overflowY,
          own: a === s.el,
        };
        if (how) {
          details.how = how;
          const whole = norm(s.el.textContent);
          details.text = whole.length > 200 ? `${whole.slice(0, 199)}…` : whole;
        }
        findings.push({
          kind,
          el: els.add(s.el),
          box: boxOf(s.el.getBoundingClientRect()),
          others: a === s.el ? [] : [els.add(a)],
          details,
        });
        break;
      }
    }
  }

  const doc = document.documentElement;
  const htmlStyle = getComputedStyle(doc);
  const bodyStyle = document.body ? getComputedStyle(document.body) : null;
  // `overflow` on <html>, or on <body> when <html> has none, is the
  // viewport's.
  const viewportOverflow = (axis: 'overflowX' | 'overflowY') =>
    htmlStyle[axis] !== 'visible' ? htmlStyle[axis] : bodyStyle ? bodyStyle[axis] : 'visible';
  const sideways = !clips(viewportOverflow('overflowX'));
  const downwards = !clips(viewportOverflow('overflowY'));

  if (kinds.has('overflow-x') && options.main && sideways && doc.scrollWidth > doc.clientWidth + 1) {
    const past = (s: Seen) => {
      const r = s.el.getBoundingClientRect();
      return r.right + scrollX > doc.clientWidth + 1;
    };
    const within = (el: Element) => {
      for (let a = parentOf(el); a; a = parentOf(a)) {
        const t = tag(a);
        if (t === 'html' || t === 'body') return false;
        const st = getComputedStyle(a);
        if (st.overflowX !== 'visible' || st.position === 'fixed') return true;
      }
      return false;
    };
    const wide = seen.filter((s) => s.style.position !== 'fixed' && past(s) && !within(s.el));
    const outer = wide.filter((s) => !wide.some((o) => o !== s && contains(o.el, s.el)));
    const named = outer.slice(0, 5);
    if (named.length) {
      findings.push({
        kind: 'overflow-x',
        el: els.add(named[0].el),
        box: boxOf(named[0].el.getBoundingClientRect()),
        others: named.map((s) => els.add(s.el)),
        details: {
          width: doc.scrollWidth,
          viewport: doc.clientWidth,
          ends: named.map((s) => px(s.el.getBoundingClientRect().right + scrollX)),
        },
      });
    }
  }

  if (kinds.has('offscreen')) {
    const width = sideways ? doc.scrollWidth : doc.clientWidth;
    const height = downwards ? doc.scrollHeight : doc.clientHeight;
    for (const t of targets) {
      const r = t.el.getBoundingClientRect();
      const fixed = t.style.position === 'fixed';
      const x = r.left + (fixed ? 0 : scrollX);
      const y = r.top + (fixed ? 0 : scrollY);
      const [w, h] = fixed ? [doc.clientWidth, doc.clientHeight] : [width, height];
      if (!(x + r.width <= 0 || y + r.height <= 0 || x >= w || y >= h)) continue;
      // A skip link: it comes into view when it has focus.
      if (tag(t.el) === 'a' && (t.el.getAttribute('href') || '').startsWith('#')) continue;
      // Clipped away by a container (a carousel's other slide): hidden.
      if (!shownOf(t).length) continue;
      findings.push({
        kind: 'offscreen',
        el: els.add(t.el),
        box: boxOf(r),
        details: { x: px(x), y: px(y), width: px(w), height: px(h) },
      });
    }
  }

  if (kinds.has('small-target')) {
    const min = options.minTarget;
    const boxes = targets
      .map((t) => ({ t, parts: shownOf(t) }))
      .filter((b) => b.parts.length)
      .map((b) => ({ ...b, box: largest(b.parts) }));
    const small = (b: Box) => b[2] < min - 0.01 || b[3] < min - 0.01;
    const native = new Native();
    try {
      for (const { t, box, parts } of boxes) {
        if (!small(box)) continue;
        // A link in a line of text: its block has other text.
        if (t.role === 'link' && t.style.display === 'inline') {
          let block: Element | null = parentOf(t.el);
          while (block && getComputedStyle(block).display.startsWith('inline')) block = parentOf(block);
          if (block && norm(block.textContent).length > norm(t.el.textContent).length) continue;
        }
        // Sized by the browser, not the page (an unstyled control).
        if (native.unstyled(t.el, box, min)) continue;
        // The spacing exception: a circle of `min` px around its centre
        // reaches no other target, nor the circle of another small one.
        const c = centre(box);
        const crowded = boxes.some((o) => {
          if (o.t === t || contains(o.t.el, t.el) || contains(t.el, o.t.el)) return false;
          if (o.parts.some((b) => reach(c, b) < min / 2)) return true;
          return small(o.box) && Math.hypot(c[0] - centre(o.box)[0], c[1] - centre(o.box)[1]) < min;
        });
        if (!crowded) continue;
        findings.push({
          kind: 'small-target',
          el: els.add(t.el),
          box: parts.length > 1 ? box : boxOf(t.el.getBoundingClientRect()),
          details: { width: px(box[2]), height: px(box[3]), min },
        });
      }
    } finally {
      native.remove();
    }
  }

  const texts: Measure[] = [];
  let skipped = 0;
  measured = [];
  if (kinds.has('contrast')) {
    for (const s of seen) {
      if (tag(s.el) === 'input' || tag(s.el) === 'textarea' || tag(s.el) === 'select') continue;
      if (visuallyHidden(s.el, s.style)) continue;
      // The text of what is disabled needs no contrast (WCAG 1.4.3).
      if (s.el.closest(':disabled, [aria-disabled="true"]')) continue;
      const own = textBox(s.el);
      if (!own) continue;
      const shownText = clipBox(s.el, own);
      const box = shownText && intersect(shownText, view);
      if (!box || box[2] < 1 || box[3] < 1) continue;
      const c = colour(s.style.webkitTextFillColor || s.style.color);
      if (!c) continue;
      const a = c[3] * opacity(s.el);
      if (a <= 0.01) continue;
      if (texts.length >= options.texts) {
        skipped++;
        continue;
      }
      const size = parseFloat(s.style.fontSize) || 16;
      const weight = parseInt(s.style.fontWeight, 10) || 400;
      texts.push({ box, colour: [c[0], c[1], c[2], a], large: size >= 24 || (size >= 18.66 && weight >= 700) });
      measured.push(s.el);
    }
  }

  return answer(els, options.testid, {
    findings,
    texts,
    skipped,
    scroll: [scrollX, scrollY],
  });
}

/** `{json, els}`: `json` with the name and key of each element of `els`,
 * taken from the text as it was before `stretchText`, so a finding has
 * the same name and key with the text stretched or not. */
function answer(els: Els, testid: string, json: object): { json: string; els: Element[] } {
  for (const [node] of stretched) node.data = originals.get(node) ?? node.data;
  try {
    const names: Named[] = els.els.map(named);
    const keys = els.els.map((el) => keyOf(el, testid));
    return { json: JSON.stringify({ ...json, names, keys }), els: els.els };
  } finally {
    for (const [node, data] of stretched) node.data = data;
  }
}

/** The elements of `layoutCheck`'s texts at `indexes`, named and keyed,
 * for the contrast findings. */
export function textNames(indexes: number[], testid: string): { json: string; els: Element[] } {
  const els = new Els();
  const at = indexes.map((i) => (measured[i] ? els.add(measured[i]) : -1));
  return answer(els, testid, { at });
}

/** The sheet that makes text transparent, while it is in. */
let hidden: CSSStyleSheet | null = null;
const HIDE_ROOTS: (Document | ShadowRoot)[] = [];

/** Makes all text in the frame transparent (a style sheet in the document
 * and every open shadow root, with a selector no page rule outranks), so
 * a screenshot shows only what is behind it; then waits for a frame. */
export async function hideText(): Promise<void> {
  showText();
  const sheet = new CSSStyleSheet();
  const all = ':is(*, #_#_#_#_#_#_)';
  sheet.replaceSync(`${all}, ${all}::before, ${all}::after, ${all}::first-letter, ${all}::first-line,
    ${all}::placeholder, ${all}::marker {
    color: transparent !important; -webkit-text-fill-color: transparent !important;
    -webkit-text-stroke-color: transparent !important; text-shadow: none !important;
    text-decoration-color: transparent !important; caret-color: transparent !important;
    transition: none !important; }`);
  hidden = sheet;
  const roots: (Document | ShadowRoot)[] = [document];
  for (const el of descendants(document)) if (el.shadowRoot) roots.push(el.shadowRoot);
  for (const root of roots) {
    root.adoptedStyleSheets = [...root.adoptedStyleSheets, sheet];
    HIDE_ROOTS.push(root);
  }
  await frame();
  await frame();
}

/** Takes `hideText`'s sheet out again. */
export function showText(): void {
  const sheet = hidden;
  hidden = null;
  for (const root of HIDE_ROOTS.splice(0)) {
    if (sheet) root.adoptedStyleSheets = root.adoptedStyleSheets.filter((s) => s !== sheet);
  }
}

/** The text nodes `stretchText` changed, with the text it gave them;
 * their text before is in `originals`. */
let stretched: [Text, string][] = [];
const originals = new WeakMap<Text, string>();

/** Makes every text node `percent`% longer, as a translation would: its
 * own words repeated until it is long enough, so lines wrap as real text
 * does. Form values and `<script>`/`<style>` are left alone. How many
 * nodes it changed. */
export async function stretchText(percent: number): Promise<number> {
  unstretchText();
  const roots: Node[] = [document.documentElement];
  for (const el of descendants(document)) if (el.shadowRoot) roots.push(el.shadowRoot);
  const nodes: Text[] = [];
  for (const root of roots) {
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let n = walker.nextNode(); n; n = walker.nextNode()) nodes.push(n as Text);
  }
  for (const node of nodes) {
    const parent = node.parentElement;
    if (!parent || SKIP.has(tag(parent)) || tag(parent) === 'textarea' || tag(parent) === 'option') continue;
    const data = node.data;
    const words = data.trim().split(/\s+/).filter(Boolean);
    if (!words.length) continue;
    const want = Math.ceil(data.trim().length * (1 + percent / 100));
    let text = data.trim();
    for (let i = 0; text.length < want; i++) text += ` ${words[i % words.length]}`;
    const lead = /^\s*/.exec(data)?.[0] || '';
    const trail = /\s*$/.exec(data)?.[0] || '';
    originals.set(node, data);
    node.data = lead + text + trail;
    stretched.push([node, node.data]);
  }
  await frame();
  return stretched.length;
}

/** Puts back the text `stretchText` changed (unless the page changed it
 * since). */
export function unstretchText(): void {
  for (const [node, data] of stretched) {
    const original = originals.get(node);
    if (original !== undefined && node.data === data) node.data = original;
  }
  stretched = [];
}

/** Scrolls the frame's page to `x, y` and waits for a frame; where it
 * got to. */
export async function scrollPage(x: number, y: number): Promise<[number, number]> {
  scrollTo({ left: x, top: y, behavior: 'instant' as ScrollBehavior });
  await frame();
  return [scrollX, scrollY];
}

/** How far the frame's page scrolls: `[scrollHeight, innerHeight, scrollX, scrollY]`. */
export function pageExtent(): [number, number, number, number] {
  const doc = document.scrollingElement || document.documentElement;
  return [doc.scrollHeight, innerHeight, scrollX, scrollY];
}

/** The content box of an `<iframe>` in its frame's viewport: where its
 * document's viewport is, and how big it is. */
export function contentBox(el: Element): Box {
  const r = el.getBoundingClientRect();
  const s = getComputedStyle(el);
  const n = (v: string) => parseFloat(v) || 0;
  const x = r.x + n(s.borderLeftWidth) + n(s.paddingLeft);
  const y = r.y + n(s.borderTopWidth) + n(s.paddingTop);
  const w = r.width - n(s.borderLeftWidth) - n(s.borderRightWidth) - n(s.paddingLeft) - n(s.paddingRight);
  const h = r.height - n(s.borderTopWidth) - n(s.borderBottomWidth) - n(s.paddingTop) - n(s.paddingBottom);
  return [x, y, Math.max(w, 0), Math.max(h, 0)];
}
