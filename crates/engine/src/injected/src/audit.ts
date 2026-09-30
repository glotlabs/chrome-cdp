// `audit` (plan 17, milestone 17.4): what its `meta` and `security`
// categories need of the page's document. The engine decides what is a
// finding; this only reads: the title, `lang`, the viewport and robots
// meta tags, whether a CSP comes in a meta tag, password fields, and each
// form's method and resolved action (listed, for refs).

import { Candidate, describe } from './locator';

/** At most this many forms are listed. */
const FORMS = 50;

interface Form {
  candidate: Candidate;
  method: string;
  action: string;
  index: number;
}

/** The content of the first `<meta name=…>` (any case), or `null`. */
function metaNamed(doc: Document, name: string): string | null {
  for (const meta of Array.from(doc.querySelectorAll('meta[name]'))) {
    if ((meta.getAttribute('name') || '').trim().toLowerCase() === name) {
      return meta.getAttribute('content') || '';
    }
  }
  return null;
}

/** The document's facts, as `{json, els}`: `json` holds the title, `lang`,
 * the viewport, description and robots meta contents, whether a meta tag
 * sets a CSP, whether there is a password field, and the forms, each
 * listed with its element in `els` (its `index`). With `root`, the forms
 * and password fields inside it only. */
export function auditPage(root: Element | null): { json: string; els: Element[] } {
  const doc = document;
  const scope: ParentNode = root || doc;
  const csp = Array.from(doc.querySelectorAll('meta[http-equiv]')).some(
    (m) => (m.getAttribute('http-equiv') || '').trim().toLowerCase() === 'content-security-policy',
  );
  const els: Element[] = [];
  const forms: Form[] = [];
  const all: Element[] = Array.from(scope.querySelectorAll('form'));
  if (root && root.tagName.toLowerCase() === 'form') all.unshift(root);
  for (const form of all.slice(0, FORMS)) {
    const f = form as HTMLFormElement;
    forms.push({
      candidate: describe(f),
      method: (f.getAttribute('method') || 'get').toLowerCase(),
      // `form.action` resolves against the base URL; an input named
      // `action` shadows it.
      action: typeof f.action === 'string' ? f.action : new URL(f.getAttribute('action') || '', doc.baseURI).href,
      index: els.length,
    });
    els.push(f);
  }
  const password = scope.querySelector('input[type=password i]') !== null;
  const title = doc.querySelector('title') ? doc.title.trim() : null;
  const html = doc.documentElement;
  const json = {
    title,
    lang: html && html.hasAttribute('lang') ? (html.getAttribute('lang') || '').trim() : null,
    viewport: metaNamed(doc, 'viewport'),
    description: metaNamed(doc, 'description'),
    robots: metaNamed(doc, 'robots'),
    csp_meta: csp,
    password,
    forms,
  };
  return { json: JSON.stringify(json), els };
}
