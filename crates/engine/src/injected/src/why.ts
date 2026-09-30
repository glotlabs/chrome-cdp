// `why <loc>` (plan 16, milestone 16.1): the actionability checks of
// `actionable.ts`, every one of them at once rather than up to the first
// that fails, each with the element that causes a failure. The engine
// adds scrolling and the hit test, as the pipeline does.

import {
  Cause,
  disabledCause,
  frame,
  inertCause,
  invisibleCause,
  named,
  Named,
  pointerCause,
  readOnly,
} from './actionable';
import { parentOf } from './dom';

/** Which checks the action needs besides visible and stable. */
export interface Needs {
  enabled: boolean;
  editable: boolean;
}

/** One check's verdict: `text` says what it saw, with `{el}` where
 * `els[el]` is named. */
interface Verdict {
  name: string;
  ok: boolean;
  text?: string;
  el?: number;
  final?: boolean;
}

/** Collects the elements a report names; each is its index in `els`. */
class Els {
  els: Element[] = [];
  add(el: Element | undefined | null): number | undefined {
    if (!el) return undefined;
    const i = this.els.indexOf(el);
    return i >= 0 ? i : this.els.push(el) - 1;
  }
  /** `{json, els}`, with each element's name in `names`. */
  answer(json: object): { json: string; els: Element[] } {
    const names: Named[] = this.els.map(named);
    return { json: JSON.stringify({ ...json, names }), els: this.els };
  }
}

const px = (n: number) => Math.round(n);
const size = (r: DOMRect) => `${px(r.width)}×${px(r.height)}`;

/** How an animation is named: its CSS name, the property a transition
 * animates, or a script animation's id. */
function animationName(a: Animation): string {
  if (typeof CSSAnimation !== 'undefined' && a instanceof CSSAnimation) return `animation "${a.animationName}"`;
  if (typeof CSSTransition !== 'undefined' && a instanceof CSSTransition) return `transition of ${a.transitionProperty}`;
  return a.id ? `animation "${a.id}"` : 'a script animation';
}

/** The running animations on `el` and on its nearest ancestor that has
 * any: `animation "pulse" on it; transition of transform on {el}`, and
 * that ancestor's index in `els`. */
function animations(el: Element, els: Els): { text: string; el?: number } {
  const running = (a: Element) =>
    a.getAnimations().filter((x) => x.playState === 'running' && (x.effect as KeyframeEffect | null)?.target === a);
  const parts: string[] = [];
  let index: number | undefined;
  const own = running(el);
  if (own.length) parts.push(`${own.slice(0, 2).map(animationName).join(', ')} on it`);
  for (let a = parentOf(el); a; a = parentOf(a)) {
    const theirs = running(a);
    if (!theirs.length) continue;
    index = els.add(a);
    parts.push(`${theirs.slice(0, 2).map(animationName).join(', ')} on {el}`);
    break;
  }
  return { text: parts.join('; '), el: index };
}

/** Checks 2-5 of the pipeline on `el`, all of them (the pipeline stops at
 * the first failed one), each with its cause; and what keeps pointer
 * events from it, for the hit test. `{json, els}`: `json` holds
 * `{checks, pointer, own, names}` (`own` names `el`). */
export async function explainActionable(el: Element, needs: Needs): Promise<{ json: string; els: Element[] }> {
  const els = new Els();
  const checks: Verdict[] = [];
  const add = (name: string, cause: Cause | null, ok?: string) => {
    if (!cause) {
      checks.push({ name, ok: true, text: ok });
      return;
    }
    const el = els.add(cause.el);
    checks.push({ name, ok: false, text: cause.what, el });
  };
  if (!el.isConnected) {
    checks.push({ name: 'visible', ok: false, text: 'it was removed from the page' });
    return els.answer({ checks });
  }
  const box = el.getBoundingClientRect();
  add('visible', invisibleCause(el), `${size(box)} at ${px(box.x)},${px(box.y)}`);
  await frame();
  const before = el.getBoundingClientRect();
  await frame();
  const after = el.getBoundingClientRect();
  const moved = Math.hypot(after.x - before.x, after.y - before.y);
  const resized = before.width !== after.width || before.height !== after.height;
  if (moved || resized) {
    const what = moved
      ? `moved ${moved < 1 ? 'less than 1' : px(moved)}px between two frames`
      : `changed size from ${size(before)} to ${size(after)} between two frames`;
    const running = animations(el, els);
    const text = running.text ? `${what} (${running.text})` : what;
    checks.push({ name: 'stable', ok: false, text, el: running.el });
  } else {
    checks.push({ name: 'stable', ok: true });
  }
  if (needs.enabled || needs.editable) add('enabled', disabledCause(el) || inertCause(el));
  if (needs.editable) {
    const cause = readOnly(el);
    if (cause) {
      checks.push({ name: 'editable', ok: false, text: cause.why.replace(/^it has /, ''), final: cause.final });
    } else {
      checks.push({ name: 'editable', ok: true });
    }
  }
  const none = pointerCause(el);
  const pointer = none ? { text: none.what, el: els.add(none.el) } : null;
  return els.answer({ checks, pointer, own: named(el) });
}

/** Why `el` is outside what can be scrolled into view, or `null`: an
 * ancestor with `overflow: hidden` or `clip` that it lies outside of, or
 * a negative offset in the document. */
export function clipped(el: Element): { json: string; els: Element[] } {
  const els = new Els();
  const r = el.getBoundingClientRect();
  for (let a = parentOf(el); a; a = parentOf(a)) {
    const s = getComputedStyle(a);
    const clips = (o: string) => o === 'hidden' || o === 'clip';
    const b = a.getBoundingClientRect();
    const outX = clips(s.overflowX) && (r.right <= b.left || r.left >= b.right);
    const outY = clips(s.overflowY) && (r.bottom <= b.top || r.top >= b.bottom);
    if (outX || outY) {
      const overflow = outX ? s.overflowX : s.overflowY;
      return els.answer({ text: `clipped out of {el} (overflow: ${overflow})`, el: els.add(a) });
    }
  }
  const view = el.ownerDocument.defaultView || window;
  const x = r.left + view.scrollX;
  const y = r.top + view.scrollY;
  if (r.width && r.height && (x + r.width <= 0 || y + r.height <= 0)) {
    return els.answer({ text: `it lies at a negative offset (x=${px(x)} y=${px(y)}), where no scroll reaches` });
  }
  return els.answer({});
}

/** The scroll offsets `scrollMark` took, by container (`null` for the
 * page). */
let marks = new Map<Element | null, [number, number]>();

const offsetOf = (a: Element | null): [number, number] => (a ? [a.scrollLeft, a.scrollTop] : [scrollX, scrollY]);

/** Notes where `el`'s scroll containers and the page are scrolled to, so
 * that `scrolledSince` can say which scrolled to show it. */
export function scrollMark(el: Element): void {
  marks = new Map();
  const root = document.scrollingElement || document.documentElement;
  for (let a = parentOf(el); a && a !== root && a !== document.body; a = parentOf(a)) {
    if (a.scrollHeight > a.clientHeight || a.scrollWidth > a.clientWidth) marks.set(a, offsetOf(a));
  }
  marks.set(null, offsetOf(null));
}

/** Which containers scrolled since `scrollMark`, innermost first, and by
 * how much: `{json: {moved: [{dx, dy, el?}], names}, els}` (no `el` for
 * the page). */
export function scrolledSince(): { json: string; els: Element[] } {
  const els = new Els();
  const moved: { dx: number; dy: number; el?: number }[] = [];
  for (const [a, [x, y]] of marks) {
    const [nx, ny] = offsetOf(a);
    const dx = px(nx - x);
    const dy = px(ny - y);
    if (dx || dy) moved.push({ dx, dy, el: els.add(a) });
  }
  marks = new Map();
  return els.answer({ moved });
}
