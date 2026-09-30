// Fingerprints (plan 18, milestone 18.2): what the element a `test`
// step's locator found looks like, so the runner can find it again once
// the locator no longer works. `fingerprint` reads one element before the
// step acts on it; `healCandidates` reads the elements of the page that
// could be the same one, which the engine scores against the fingerprint
// (`crate::fingerprint`), and `healStable` gives the winner's locator.
// The engine filters the class names and turns the path into a template,
// so both rules have unit tests. Their locators run in the project's
// `test`, where its `testid_attribute` holds, so a test id is always
// written `testid=…`.

import { labelsOf, nameOf, roleOf, textOf } from './aria';
import { descendants, norm, parentOf, tag, visible } from './dom';
import { Step, query } from './locator';
import { stable } from './stable';
import { Texts } from './text';

/** Longer names and texts are cut: the start is what identifies them. */
const LONGEST = 80;
/** How many class names the engine gets to filter. */
const CLASSES = 20;
/** How many elements `healCandidates` reads at most. */
const MOST = 400;

/** The roles a fingerprint's container can have: those that name a part
 * of the page a user would point at. */
const CONTAINERS = new Set(['dialog', 'alertdialog', 'form', 'listitem', 'row', 'region', 'banner', 'main',
  'navigation', 'contentinfo', 'complementary', 'search']);

/** Roles that stand in for each other after a redesign. */
const FAMILIES = [['button', 'link'], ['textbox', 'searchbox', 'combobox']];

const cut = (s: string, n: number) => (s.length > n ? s.slice(0, n) : s);

interface Container {
  role: string;
  name?: string;
}

/** What an element looks like: the fingerprint's fields but `page` and
 * `candidates`, with every class name. */
interface Features {
  role?: string;
  name?: string;
  tag: string;
  type?: string;
  testid?: string;
  id?: string;
  label?: string;
  placeholder?: string;
  text?: string;
  classes?: string[];
  container?: Container;
  index?: number;
}

/** The container of `el`, and the element that is. */
function containerOf(el: Element): [Element, Container] | null {
  for (let a = parentOf(el); a; a = parentOf(a)) {
    const role = roleOf(a);
    if (!role || !CONTAINERS.has(role)) continue;
    const name = cut(role === 'row' || role === 'listitem' ? norm(textOf(a, null, 0)) : nameOf(a, role), 40);
    return [a, name ? { role, name } : { role }];
  }
  return null;
}

function features(el: Element, testid: string, texts: Texts): Features {
  const out: Features = { tag: tag(el) };
  const role = roleOf(el);
  if (role && role !== 'generic' && role !== 'none') {
    out.role = role;
    const name = cut(norm(nameOf(el, role)), LONGEST);
    if (name) out.name = name;
  }
  const type = el.getAttribute('type');
  if (type) out.type = type.toLowerCase();
  const id = el.getAttribute(testid);
  if (id) out.testid = id;
  if (el.id) out.id = el.id;
  const label = labelsOf(el).map(norm).find((l) => l);
  if (label) out.label = cut(label, LONGEST);
  const placeholder = norm(el.getAttribute('placeholder'));
  if (placeholder) out.placeholder = cut(placeholder, LONGEST);
  const text = cut(norm(texts.of(el)), LONGEST);
  if (text) out.text = text;
  const classes = Array.from(el.classList).slice(0, CLASSES);
  if (classes.length) out.classes = classes;
  const found = containerOf(el);
  if (found) out.container = found[1];
  if (out.role) {
    const scope = found ? found[0] : el.ownerDocument;
    const index = descendants(scope).filter((e) => roleOf(e) === out.role).indexOf(el);
    if (index >= 0) out.index = index;
  }
  return out;
}

/** `el`'s fingerprint as JSON: its features, the path of its document,
 * and the stable locators that find it alone, best first. */
export function fingerprint(el: Element, testid: string): string {
  const attribute = testid || 'data-testid';
  const out: Features & { page?: string; candidates?: string[] } = features(el, attribute, new Texts());
  out.page = el.ownerDocument.location ? el.ownerDocument.location.pathname : '';
  const found = JSON.parse(stable(el, attribute, true, true)) as { candidates?: { locator: string; count: number }[] };
  const candidates = (found.candidates || []).filter((c) => c.count === 1).map((c) => c.locator).slice(0, 5);
  if (candidates.length) out.candidates = candidates;
  return JSON.stringify(out);
}

/** The elements the last `healCandidates` read, for `healStable`. */
let read: Element[] = [];

/** Whether roles `a` and `b` are one or of one family. */
function akin(a: string | null, b: string | undefined): boolean {
  if (!a || !b) return false;
  return a === b || FAMILIES.some((f) => f.includes(a) && f.includes(b));
}

/** The visible elements of the document that could be the element
 * fingerprint `fp` describes (of its role or its family, its tag, its test
 * id or its id), with their features, and how many elements the step's
 * locator (`steps`) finds now. As JSON: `{found, elements: [{i, …}]}`. */
export function healCandidates(fp: Features, testid: string, steps: Step[] | null): string {
  const attribute = testid || 'data-testid';
  const found = steps ? query(steps, null).length : 0;
  const texts = new Texts();
  read = [];
  const elements: (Features & { i: number })[] = [];
  for (const el of descendants(document)) {
    if (read.length >= MOST) break;
    const could = akin(roleOf(el), fp.role) || tag(el) === fp.tag
      || (!!fp.testid && el.getAttribute(attribute) === fp.testid) || (!!fp.id && el.id === fp.id);
    if (!could || !visible(el)) continue;
    elements.push({ i: read.length, ...features(el, attribute, texts) });
    read.push(el);
  }
  return JSON.stringify({ found, elements });
}

/** `stable` of element `i` of the last `healCandidates`. */
export function healStable(i: number, testid: string): string {
  const el = read[i];
  if (!el || !el.isConnected) return JSON.stringify({ reason: 'it is gone' });
  return stable(el, testid || 'data-testid', false, true);
}
