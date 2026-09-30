// `compare --visual` (plan 16, milestone 16.5): names what is where a
// region of changed pixels is. For each point (in this frame's viewport,
// CSS px), the smallest visible element whose box holds it.

import { named, Named } from './actionable';
import { descendants, tag, visible } from './dom';

/** `{json, els}`: `json` holds `names` (one per element of `els`) and
 * `at`, the index into them of each point's element (-1 for none). */
export function elementsAt(points: [number, number][]): { json: string; els: Element[] } {
  const boxes: [Element, DOMRect][] = [];
  for (const el of descendants(document)) {
    const t = tag(el);
    if (t === 'html' || t === 'head' || t === 'script' || t === 'style') continue;
    const r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) continue;
    if (!points.some(([x, y]) => x >= r.left && x < r.right && y >= r.top && y < r.bottom)) continue;
    if (!visible(el)) continue;
    boxes.push([el, r]);
  }
  const els: Element[] = [];
  const at = points.map(([x, y]) => {
    let best: Element | null = null;
    let area = Infinity;
    for (const [el, r] of boxes) {
      if (x < r.left || x >= r.right || y < r.top || y >= r.bottom) continue;
      // Of the same size, the innermost (it comes later in the walk).
      if (r.width * r.height <= area) {
        best = el;
        area = r.width * r.height;
      }
    }
    if (!best) return -1;
    const i = els.indexOf(best);
    return i >= 0 ? i : els.push(best) - 1;
  });
  const names: Named[] = els.map(named);
  return { json: JSON.stringify({ names, at }), els };
}
