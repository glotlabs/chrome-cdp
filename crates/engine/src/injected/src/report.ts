// The post-action report (plan 05, milestone 5.5): whether an action
// moved focus somewhere the agent should know about. Before the action
// the engine marks where focus is (`focusMark`); every element the
// action names goes through `acted`; afterwards `focusMoved` returns the
// element focus went to if it is in another form than before and the
// action did not act on it (a dialog or the next step of a flow took it).

import { contains } from './dom';
import { focused } from './form';
import { Candidate, describe } from './locator';

let before: { el: Element | null; form: Element | null } | null = null;
let actedOn: Element[] = [];

/** The form `el` belongs to: its form owner, else the <form> around it. */
function formOf(el: Element | null): Element | null {
  if (!el) return null;
  const owner = (el as HTMLInputElement).form;
  return owner instanceof HTMLFormElement ? owner : el.closest('form');
}

/** Marks where focus is now, before an action. */
export function focusMark(): void {
  const el = focused();
  before = { el, form: formOf(el) };
  actedOn = [];
}

/** Names `el` (as `describe` does) and remembers the action acted on it. */
export function acted(el: Element): Candidate {
  if (before) actedOn.push(el);
  return describe(el);
}

/** The element focus moved to since `focusMark`, if it is in another
 * form than before and not what the action acted on (or in it, or the
 * control of a label it clicked); `null` otherwise. Focus inside an
 * iframe counts as not moved (its document is another). */
export function focusMoved(): Element | null {
  const mark = before;
  before = null;
  const el = focused();
  if (!mark || !el || el === mark.el || el instanceof HTMLIFrameElement) return null;
  for (const a of actedOn) {
    if (a === el || contains(a, el) || (a instanceof HTMLLabelElement && a.control === el)) return null;
  }
  return formOf(el) === mark.form ? null : el;
}
