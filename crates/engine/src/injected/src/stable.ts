// The most robust locator that finds one element and nothing else (plan
// 11, milestone 11.8): what `history --as-scenario` writes in place of a
// ref, which lives only as long as its session. Tried in order: role and
// name, label, test id, placeholder, text; then the first of those that
// is unique inside a named container (a list item, a row, a dialog, …);
// then inside a container picked by the exact text of something in it
// (`role=row >> has=text="Invoice 12" >> …`, plan 14); then `>> nth=` as
// a last resort. Each is checked with the locator engine itself, so it
// finds this element when the scenario runs. `locator --all` lists them.

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

function candidates(el: Element, testid: string, texts: Texts, own: boolean): Candidate[] {
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
    const locator = testid === 'data-testid' || own ? `testid=${id}` : `css=[${testid}=${quoted(id)}]`;
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

/** The first bit of text in `a`, outside `el`, that an element holds
 * whole: `text="…"` finds that element, and `has=` the container around
 * it. */
function leafText(a: Element, el: Element, texts: Texts): string {
  for (const leaf of Array.from(a.querySelectorAll('*'))) {
    if (leaf.childElementCount > 0 || el.contains(leaf) || leaf.contains(el)) continue;
    const text = norm(texts.of(leaf));
    if (text) return text.length <= LONGEST ? text : '';
  }
  return '';
}

/** Every locator `stable` tries for `el`, best first; `how` says which
 * kind it is. Their own candidates first, then scoped ones, then `nth=`. */
function* tried(el: Element, testid: string, texts: Texts, short: boolean): Generator<Candidate & { how: string }> {
  const own = candidates(el, testid, texts, short);
  for (const c of own) yield { ...c, how: c.steps[0].kind };
  if (own.length === 0) return;
  const around: [Element, string][] = [];
  for (let a = parentOf(el); a; a = parentOf(a)) {
    const role = roleOf(a);
    if (role && CONTEXT_ROLES.has(role)) around.push([a, role]);
  }
  for (const [a, role] of around) {
    for (const outer of containers(a, role)) {
      for (const c of own) {
        yield { steps: [...outer.steps, ...c.steps], locator: `${outer.locator} >> ${c.locator}`, how: 'scoped' };
      }
    }
  }
  for (const [a, role] of around) {
    const text = leafText(a, el, texts);
    if (!text) continue;
    const outer: Step[] = [
      { kind: 'role', role },
      { kind: 'has', steps: [{ kind: 'text', text: { mode: 'exact', value: text } }] },
    ];
    if (!only(outer, a)) continue;
    for (const c of own) {
      yield {
        steps: [...outer, ...c.steps],
        locator: `role=${role} >> has=text=${quoted(text)} >> ${c.locator}`,
        how: 'has',
      };
    }
  }
  const first = own[0];
  const found = query(first.steps, null);
  const index = found.indexOf(el);
  if (index >= 0 && found.length > 1) {
    yield { steps: [...first.steps, { kind: 'nth', index }], locator: `${first.locator} >> nth=${index}`, how: 'nth' };
  }
}

/** `{"locator": "…", "how": "role|label|…|scoped|has|nth"}`, or
 * `{"reason": "…"}` when there is none. With `all`, also `candidates`:
 * every locator tried that finds `el`, with how many elements it finds
 * (`1`: it finds `el` alone), the usable ones first: its own locators,
 * and the scoped ones only when none of those is usable. In an iframe,
 * the locators are the frame's own; the engine puts the `frame=` step
 * before them. With `short`, a test id of an attribute other than
 * `data-testid` is written `testid=…` too: the locator runs where that
 * attribute is configured (a project's `test`), else `css=[attr="…"]`. */
export function stable(el: Element, testid: string, all?: boolean, short?: boolean): string {
  const texts = new Texts();
  const usable: { locator: string; count: number }[] = [];
  const others: { locator: string; count: number }[] = [];
  let best: { locator: string; how: string } | null = null;
  let any = false;
  const own = new Set(['role', 'label', 'testid', 'placeholder', 'text']);
  for (const c of tried(el, testid || 'data-testid', texts, !!short)) {
    // Scoped forms are only tried when none of the element's own is usable.
    if (best && own.has(best.how) && !own.has(c.how)) break;
    any = true;
    const found = query(c.steps, null);
    if (found.length === 1 && found[0] === el) {
      if (!best) best = { locator: c.locator, how: c.how };
      if (!all) break;
      usable.push({ locator: c.locator, count: 1 });
    } else if (all && c.how !== 'scoped' && c.how !== 'has' && found.includes(el)) {
      others.push({ locator: c.locator, count: found.length });
    }
  }
  const candidates = all ? { candidates: [...usable, ...others] } : {};
  if (best) return JSON.stringify({ ...best, ...candidates });
  const reason = any
    ? 'no locator finds it (is it hidden?)'
    : 'it has no role and name, label, test id, placeholder or text';
  return JSON.stringify({ reason, ...candidates });
}
