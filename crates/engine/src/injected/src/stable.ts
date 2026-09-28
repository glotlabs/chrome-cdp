// The most robust locator that finds one element and nothing else (plan
// 11, milestone 11.8): what `history --as-scenario` writes in place of a
// ref, which lives only as long as its session. Tried in order: role and
// name, label, test id, placeholder, text; then the first of those that
// is unique inside a named container (a list item, a row, a dialog, …);
// then `>> nth=` as a last resort. Each is checked with the locator
// engine itself, so it finds this element when the scenario runs.

import { labelsOf, nameOf, roleOf } from './aria';
import { norm, parentOf } from './dom';
import { CONTEXT_ROLES, Step, query } from './locator';
import { Texts } from './text';

/** Longer names and texts make brittle locators. */
const LONGEST = 80;

interface Candidate {
  steps: Step[];
  locator: string;
}

/** A quoted locator value: exact, with `"` and `\` escaped. */
const quoted = (s: string) => '"' + s.replace(/[\\"]/g, (m) => '\\' + m) + '"';

/** Whether `s` can be an unquoted locator value (a substring). */
const bare = (s: string) => !!s && !/^["'/]|>>|[\[\]]/.test(s);

function candidates(el: Element, testid: string, texts: Texts): Candidate[] {
  const out: Candidate[] = [];
  const role = roleOf(el);
  if (role) {
    const name = nameOf(el, role);
    if (name && name.length <= LONGEST) {
      out.push({
        steps: [{ kind: 'role', role, name: { mode: 'exact', value: name } }],
        locator: `role=${role}[name=${quoted(name)}]`,
      });
    }
  }
  for (const label of labelsOf(el)) {
    if (label.length <= LONGEST) {
      out.push({
        steps: [{ kind: 'label', text: { mode: 'exact', value: label } }],
        locator: `label=${quoted(label)}`,
      });
    }
  }
  const id = el.getAttribute(testid);
  if (id && !/\s|>>|"|'/.test(id)) {
    const locator = testid === 'data-testid' ? `testid=${id}` : `css=[${testid}=${quoted(id)}]`;
    out.push({ steps: [{ kind: 'testid', value: id, attribute: testid }], locator });
  }
  const placeholder = norm(el.getAttribute('placeholder'));
  if (placeholder && placeholder.length <= LONGEST) {
    out.push({
      steps: [{ kind: 'placeholder', text: { mode: 'exact', value: placeholder } }],
      locator: `placeholder=${quoted(placeholder)}`,
    });
  }
  const text = norm(texts.of(el));
  if (text && text.length <= LONGEST) {
    out.push({
      steps: [{ kind: 'text', text: { mode: 'exact', value: text } }],
      locator: `text=${quoted(text)}`,
    });
  }
  return out;
}

/** Whether `steps` find `el` and nothing else. */
function only(steps: Step[], el: Element): boolean {
  const found = query(steps, null);
  return found.length === 1 && found[0] === el;
}

/** The first bit of text in `el` (a product's name in its list item). */
function firstText(el: Element): string {
  const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const text = norm((node as Text).data);
    if (text) return text.length > 40 ? text.slice(0, 40).replace(/\s+\S*$/, '') : text;
  }
  return '';
}

/** Locators of container `a` that find it alone. */
function containers(a: Element, role: string): Candidate[] {
  const out: Candidate[] = [];
  const name = nameOf(a, role);
  if (name && name.length <= LONGEST) {
    out.push({
      steps: [{ kind: 'role', role, name: { mode: 'exact', value: name } }],
      locator: `role=${role}[name=${quoted(name)}]`,
    });
  }
  const text = firstText(a);
  if (bare(text)) {
    out.push({
      steps: [{ kind: 'role', role }, { kind: 'has-text', text: { mode: 'substring', value: text } }],
      locator: `role=${role} >> has-text=${text}`,
    });
  }
  return out.filter((c) => only(c.steps, a));
}

/** `{"locator": "…", "how": "role|label|…|scoped|nth"}`, or
 * `{"reason": "…"}` when there is none. */
export function stable(el: Element, testid: string): string {
  if (window !== window.top) {
    return JSON.stringify({ reason: 'it is in a frame, and frame locators are not written yet' });
  }
  const texts = new Texts();
  const own = candidates(el, testid || 'data-testid', texts);
  for (const c of own) {
    if (only(c.steps, el)) return JSON.stringify({ locator: c.locator, how: c.steps[0].kind });
  }
  if (own.length === 0) {
    return JSON.stringify({ reason: 'it has no role and name, label, test id, placeholder or text' });
  }
  for (let a = parentOf(el); a; a = parentOf(a)) {
    const role = roleOf(a);
    if (!role || !CONTEXT_ROLES.has(role)) continue;
    for (const outer of containers(a, role)) {
      for (const c of own) {
        const steps = [...outer.steps, ...c.steps];
        if (only(steps, el)) {
          return JSON.stringify({ locator: `${outer.locator} >> ${c.locator}`, how: 'scoped' });
        }
      }
    }
  }
  const first = own[0];
  const index = query(first.steps, null).indexOf(el);
  if (index < 0) return JSON.stringify({ reason: 'no locator finds it (is it hidden?)' });
  return JSON.stringify({ locator: `${first.locator} >> nth=${index}`, how: 'nth' });
}
