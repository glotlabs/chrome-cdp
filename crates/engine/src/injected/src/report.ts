// The post-action report (plan 05, milestone 5.5): whether an action
// moved focus somewhere the agent should know about. Before the action
// the engine marks where focus is (`focusMark`) in every frame; every
// element the action names goes through `acted`; afterwards `focusMoved`
// returns the element focus went to if it is in another form or frame
// than before and the action did not act on it (a dialog or the next
// step of a flow took it). Focus in an iframe is followed by the engine:
// both return the focused `<iframe>`, and it asks that frame next.

import { contains } from './dom';
import { focused } from './form';
import { Candidate, describe } from './locator';

/** Where focus was: `away` when it was in another document. */
let before: { el: Element | null; form: Element | null; away: boolean } | null = null;
let actedOn: Element[] = [];

const isFrame = (el: Element | null): el is HTMLIFrameElement | HTMLFrameElement =>
  el instanceof HTMLIFrameElement || el instanceof HTMLFrameElement;

/** The form `el` belongs to: its form owner, else the <form> around it. */
function formOf(el: Element | null): Element | null {
  if (!el) return null;
  const owner = (el as HTMLInputElement).form;
  return owner instanceof HTMLFormElement ? owner : el.closest('form');
}

/** Marks where focus is now, before an action. `held`: whether focus is
 * in this document, as far as the engine knows (the main frame, or a
 * frame whose `<iframe>` has focus in its parent). Returns the focused
 * `<iframe>` when focus is further down, in its frame. */
export function focusMark(held = true): Element | null {
  const el = focused();
  const here = held && !isFrame(el);
  before = { el: here ? el : null, form: here ? formOf(el) : null, away: !here };
  actedOn = [];
  return held && isFrame(el) ? el : null;
}

/** Names `el` (as `describe` does) and remembers the action acted on it. */
export function acted(el: Element): Candidate {
  if (before) actedOn.push(el);
  return describe(el);
}

/** The element focus moved to since `focusMark`, if it is in another
 * form (or came from another frame, into another form than the action
 * acted in) and is not what the action acted on (or in it, or the
 * control of a label it clicked); `null` otherwise.
 * When focus is in an iframe, that `<iframe>`: the engine asks its frame. */
export function focusMoved(): Element | null {
  // A document with no mark (an iframe the action added) did not hold
  // focus before.
  const mark = before || { el: null, form: null, away: true };
  const wasMarked = !!before;
  before = null;
  const el = focused();
  if (isFrame(el)) return el;
  if (!wasMarked) actedOn = [];
  if (!el || el === mark.el) return null;
  for (const a of actedOn) {
    if (a === el || contains(a, el) || (a instanceof HTMLLabelElement && a.control === el)) return null;
  }
  // From another frame: news, unless the action acted in the same form.
  if (mark.away) return actedOn.some((a) => formOf(a) === formOf(el)) ? null : el;
  return formOf(el) === mark.form ? null : el;
}
