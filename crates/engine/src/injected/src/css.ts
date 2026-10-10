// CSS selectors that pierce open shadow roots: a
// descendant combinator (a space) may cross into a shadow root, so
// `#app .item` finds `.item` inside `#app`'s shadow tree. Other
// combinators (`>`, `+`, `~`) stay inside one tree.

import { parentOf } from './dom';
import { LocatorError } from './errors';

/** Splits `text` at `sep` characters outside quotes, brackets and
 * parentheses. With `sep` = ' ', runs of spaces next to `>`, `+` or `~`
 * do not split. */
function split(text: string, sep: string): string[] {
  const parts: string[] = [];
  let depth = 0;
  let quote = '';
  let current = '';
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quote) {
      current += c;
      if (c === '\\') current += text[++i] ?? '';
      else if (c === quote) quote = '';
      continue;
    }
    if (c === '"' || c === "'") quote = c;
    else if (c === '[' || c === '(') depth++;
    else if (c === ']' || c === ')') depth--;
    else if (c === '\\') {
      current += c + (text[++i] ?? '');
      continue;
    }
    const isSep = depth === 0 && (sep === ' ' ? /\s/.test(c) : c === sep);
    if (!isSep) {
      current += c;
      continue;
    }
    if (sep === ' ') {
      let j = i;
      while (j + 1 < text.length && /\s/.test(text[j + 1])) j++;
      const next = text[j + 1] ?? '';
      const prev = current.trimEnd().slice(-1);
      i = j;
      if (!current.trim() || '>+~'.includes(next) || '>+~'.includes(prev) || next === '') {
        current += ' ';
        continue;
      }
    }
    parts.push(current.trim());
    current = '';
  }
  parts.push(current.trim());
  return parts.filter(Boolean);
}

/** The open shadow roots under `scope`, nested ones too. */
function shadowRoots(scope: Document | Element | ShadowRoot): ShadowRoot[] {
  const out: ShadowRoot[] = [];
  const visit = (root: Document | Element | ShadowRoot) => {
    if (root instanceof Element && root.shadowRoot) {
      out.push(root.shadowRoot);
      visit(root.shadowRoot);
    }
    for (const el of Array.from(root.querySelectorAll('*'))) {
      if (el.shadowRoot) {
        out.push(el.shadowRoot);
        visit(el.shadowRoot);
      }
    }
  };
  visit(scope);
  return out;
}

/** Whether `el`'s ancestors (across shadow boundaries) match `parts`, the
 * compounds before its own, innermost last. */
function ancestorsMatch(el: Element, parts: string[]): boolean {
  let j = parts.length - 1;
  for (let a = parentOf(el); a && j >= 0; a = parentOf(a)) {
    if (a.matches(parts[j])) j--;
  }
  return j < 0;
}

/** The elements under `scope` that `selector` matches. */
export function css(selector: string, scope: Document | Element | ShadowRoot): Element[] {
  let light: Element[];
  try {
    light = Array.from(scope.querySelectorAll(selector));
  } catch (e) {
    throw new LocatorError(`${selector} is not a valid CSS selector`);
  }
  const roots = shadowRoots(scope);
  if (!roots.length) return light;
  const out = new Set(light);
  for (const alternative of split(selector, ',')) {
    const parts = split(alternative, ' ');
    const last = parts.pop();
    if (!last) continue;
    for (const root of roots) {
      for (const el of Array.from(root.querySelectorAll(last))) {
        if (ancestorsMatch(el, parts)) out.add(el);
      }
    }
  }
  return Array.from(out);
}
