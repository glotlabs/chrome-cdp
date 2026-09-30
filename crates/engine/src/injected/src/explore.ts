// `explore` (plan 16, milestone 16.6): what a state of the page offers to
// follow, in document order: links, and the buttons, tabs, menu items
// and disclosure controls `--click-buttons` clicks. Each comes with the
// stable locator a scenario reaches it by (`stable.ts`), so the walk can
// find it again after going back to the state, and `explore -o` can
// write the path to it. The engine decides what is followed: the origin,
// the kind of link, a destructive name, a form's submit.

import { hiddenForAria, roleOf } from './aria';
import { descendants, tag, visible } from './dom';
import { describe } from './locator';
import { stable } from './stable';

/** Roles a click opens or switches something with. */
const CONTROL_ROLES = new Set(['button', 'tab', 'menuitem', 'menuitemcheckbox', 'menuitemradio', 'switch']);

interface Found {
  /** `link`, `button` or `submit` (a button that submits its form). */
  kind: string;
  role: string;
  name: string;
  tag: string;
  /** A link's target, absolute. */
  href?: string;
  /** A link's `target` (`_blank`). */
  target?: string;
  /** A link with `download`. */
  download?: boolean;
  /** The locator that finds it alone, or why there is none. */
  locator?: string;
  reason?: string;
}

/** Whether `el` submits a form when clicked. */
function submits(el: Element): boolean {
  if (el instanceof HTMLButtonElement) return el.type === 'submit' && !!el.form;
  if (el instanceof HTMLInputElement) return (el.type === 'submit' || el.type === 'image') && !!el.form;
  return false;
}

/** What `el` is to the walk, or `null` when it is nothing to follow. */
function kindOf(el: Element): string | null {
  const t = tag(el);
  if ((t === 'a' || t === 'area') && el.hasAttribute('href') && !(el instanceof SVGElement)) return 'link';
  if (el instanceof HTMLButtonElement || el instanceof HTMLInputElement) {
    if (el.disabled) return null;
    if (el instanceof HTMLInputElement && !['button', 'submit', 'image', 'reset'].includes(el.type)) return null;
    if (el.type === 'reset') return 'reset';
    return submits(el) ? 'submit' : 'button';
  }
  if (t === 'summary') return 'button';
  const role = el.getAttribute('role') ? roleOf(el) : null;
  if (el.getAttribute('aria-disabled') === 'true') return null;
  if ((role && CONTROL_ROLES.has(role)) || el.hasAttribute('aria-expanded') || el.hasAttribute('aria-haspopup')) {
    return 'button';
  }
  return null;
}

/** The candidates of this document, as JSON: `[Found]`. Only what shows
 * and takes clicks (not hidden, not inert behind a modal dialog). With
 * `buttons` false, only links and (for the engine to count) submit
 * buttons are listed. */
export function exploreCandidates(testid: string, buttons: boolean): string {
  const out: Found[] = [];
  for (const el of descendants(document)) {
    const kind = kindOf(el);
    if (!kind) continue;
    if (!buttons && kind === 'button') continue;
    if (!visible(el) || hiddenForAria(el) || el.closest('[inert]')) continue;
    const described = describe(el);
    const found: Found = { kind, role: described.role, name: described.name, tag: described.tag };
    if (kind === 'link') {
      const a = el as HTMLAnchorElement;
      found.href = a.href;
      if (a.target) found.target = a.target;
      if (a.hasAttribute('download')) found.download = true;
    }
    const located = JSON.parse(stable(el, testid)) as { locator?: string; reason?: string };
    if (located.locator) found.locator = located.locator;
    else found.reason = located.reason;
    out.push(found);
  }
  return JSON.stringify(out);
}
