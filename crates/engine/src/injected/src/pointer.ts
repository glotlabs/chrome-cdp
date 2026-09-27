// Mouse and pointer actions (plan 05, milestone 5.2): what the engine asks
// the page about scrolling and dragging. The events themselves are real
// input (Input.dispatchMouseEvent and friends), sent by the engine.

import { parentOf } from './dom';

/** Whether a wheel can scroll the element in some direction. */
function scrollable(el: Element): boolean {
  const s = getComputedStyle(el);
  const can = (overflow: string) => /^(auto|scroll|overlay)$/.test(overflow);
  return (can(s.overflowY) && el.scrollHeight > el.clientHeight) || (can(s.overflowX) && el.scrollWidth > el.clientWidth);
}

/** The scroll container a wheel over `el` scrolls first: `el` or its
 * nearest scrollable ancestor (across shadow roots); `null` for the
 * page. */
function container(el: Element | null): Element | null {
  const root = document.scrollingElement || document.documentElement;
  for (let a = el; a; a = parentOf(a)) {
    if (a === root || a === document.body) return null;
    if (scrollable(a)) return a;
  }
  return null;
}

/** A scroll container's offset and the largest it can have, in whole CSS
 * pixels. */
export interface Scroll {
  x: number;
  y: number;
  maxX: number;
  maxY: number;
}

function measure(c: Element | null): Scroll {
  const s = c || document.scrollingElement || document.documentElement;
  const x = c ? c.scrollLeft : scrollX;
  const y = c ? c.scrollTop : scrollY;
  return {
    x: Math.round(x),
    y: Math.round(y),
    maxX: Math.max(0, Math.round(s.scrollWidth - s.clientWidth)),
    maxY: Math.max(0, Math.round(s.scrollHeight - s.clientHeight)),
  };
}

/** Where the scroll container of `el` (the page without one) is:
 * `{json: Scroll + page, els: [container]}`. */
export function scrollState(el: Element | null): { json: string; els: (Element | null)[] } {
  const c = container(el);
  return { json: JSON.stringify({ ...measure(c), page: !c }), els: [c] };
}

/** Waits until a wheel's scroll is over: the offset of `el`'s container
 * moved away from `from` and then held for two frames, or it did not move
 * for `idle` frames (nothing to scroll, or the page ignored the wheel).
 * At most `ms` milliseconds. */
export function scrollSettled(
  el: Element | null,
  from: { x: number; y: number },
  idle: number,
  ms: number,
): Promise<Scroll> {
  const c = container(el);
  return new Promise((resolve) => {
    let last = measure(c);
    let moved = false;
    let same = 0;
    let frames = 0;
    const done = setTimeout(() => resolve(measure(c)), ms);
    const tick = () => {
      const now = measure(c);
      if (now.x !== from.x || now.y !== from.y) moved = true;
      same = now.x === last.x && now.y === last.y ? same + 1 : 0;
      last = now;
      frames += 1;
      if ((moved && same >= 2) || (!moved && frames >= idle)) {
        clearTimeout(done);
        resolve(now);
      } else {
        requestAnimationFrame(tick);
      }
    };
    requestAnimationFrame(tick);
  });
}

/** Whether pressing on `el` and moving can start an HTML5 drag: it or an
 * ancestor is draggable (`draggable="true"`, a link, an image), and no
 * nearer one says `draggable="false"`. */
export function draggable(el: Element): boolean {
  for (let a: Element | null = el; a; a = parentOf(a)) {
    if (a.getAttribute('draggable') === 'false') return false;
    if (a instanceof HTMLElement && a.draggable) return true;
  }
  return false;
}
