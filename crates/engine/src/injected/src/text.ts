// Matching text as locators do: whitespace collapsed, a case-insensitive
// substring by default, the whole text when quoted, or a regex.

import { SKIP, childrenOf, norm, tag } from './dom';
import { LocatorError } from './errors';

/** A text to match, as the engine sends it. */
export interface TextMatch {
  mode: 'substring' | 'exact' | 'regex';
  value: string;
  flags?: string;
}

export type Matcher = (text: string) => boolean;

export function matcher(m: TextMatch): Matcher {
  if (m.mode === 'regex') {
    let re: RegExp;
    try {
      re = new RegExp(m.value, (m.flags || '').replace(/[gy]/g, ''));
    } catch (e) {
      throw new LocatorError(`/${m.value}/${m.flags || ''} is not a valid regex: ${(e as Error).message}`);
    }
    return (text) => re.test(norm(text));
  }
  const want = norm(m.value);
  if (m.mode === 'exact') return (text) => norm(text) === want;
  const lower = want.toLowerCase();
  return (text) => norm(text).toLowerCase().includes(lower);
}

/** The text of elements, as `text=` and `has-text=` read it: the text
 * content of the composed tree (open shadow roots and slots), without
 * scripts and styles, and a button-like input's value. Cached per query. */
export class Texts {
  private cache = new Map<Element, string>();

  of(el: Element): string {
    let text = this.cache.get(el);
    if (text !== undefined) return text;
    text = this.compute(el);
    this.cache.set(el, text);
    return text;
  }

  private compute(el: Element): string {
    if (SKIP.has(tag(el))) return '';
    if (tag(el) === 'input') {
      const input = el as HTMLInputElement;
      return ['button', 'submit', 'reset'].includes(input.type) ? input.value : '';
    }
    let out = '';
    for (const child of childrenOf(el)) {
      if (child.nodeType === Node.TEXT_NODE) out += (child as Text).data;
      else if (child instanceof Element) out += tag(child) === 'br' ? '\n' : this.of(child);
    }
    return out;
  }
}
