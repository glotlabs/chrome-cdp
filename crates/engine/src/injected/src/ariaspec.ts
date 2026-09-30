// What ARIA 1.2 and HTML define, for the accessibility rules of
// `a11y.ts` (plan 17, milestone 17.3): the roles, the `aria-*`
// attributes and their values, the roles' required attributes, owned
// elements and context, and the `autocomplete` tokens.

/** ARIA 1.2's concrete roles, the DPUB and Graphics modules' roles, and
 * what Chrome already knows of ARIA 1.3 (`image`, `mark`, `comment`,
 * `suggestion`). Abstract roles (`widget`, `landmark`, …) are not here:
 * they are not for authors. */
export const ROLES = new Set([
  'alert', 'alertdialog', 'application', 'article', 'banner', 'blockquote', 'button', 'caption',
  'cell', 'checkbox', 'code', 'columnheader', 'combobox', 'complementary', 'contentinfo',
  'definition', 'deletion', 'dialog', 'directory', 'document', 'emphasis', 'feed', 'figure', 'form',
  'generic', 'grid', 'gridcell', 'group', 'heading', 'img', 'insertion', 'link', 'list', 'listbox',
  'listitem', 'log', 'main', 'marquee', 'math', 'menu', 'menubar', 'menuitem', 'menuitemcheckbox',
  'menuitemradio', 'meter', 'navigation', 'none', 'note', 'option', 'paragraph', 'presentation',
  'progressbar', 'radio', 'radiogroup', 'region', 'row', 'rowgroup', 'rowheader', 'scrollbar',
  'search', 'searchbox', 'separator', 'slider', 'spinbutton', 'status', 'strong', 'subscript',
  'superscript', 'switch', 'tab', 'table', 'tablist', 'tabpanel', 'term', 'textbox', 'time', 'timer',
  'toolbar', 'tooltip', 'tree', 'treegrid', 'treeitem',
  'image', 'mark', 'comment', 'suggestion',
  'graphics-document', 'graphics-object', 'graphics-symbol',
  'doc-abstract', 'doc-acknowledgments', 'doc-afterword', 'doc-appendix', 'doc-backlink',
  'doc-biblioentry', 'doc-bibliography', 'doc-biblioref', 'doc-chapter', 'doc-colophon',
  'doc-conclusion', 'doc-cover', 'doc-credit', 'doc-credits', 'doc-dedication', 'doc-endnote',
  'doc-endnotes', 'doc-epigraph', 'doc-epilogue', 'doc-errata', 'doc-example', 'doc-footnote',
  'doc-foreword', 'doc-glossary', 'doc-glossref', 'doc-index', 'doc-introduction', 'doc-noteref',
  'doc-notice', 'doc-pagebreak', 'doc-pagefooter', 'doc-pageheader', 'doc-pagelist', 'doc-part',
  'doc-preface', 'doc-prologue', 'doc-pullquote', 'doc-qna', 'doc-subtitle', 'doc-tip', 'doc-toc',
]);

/** How an attribute's value is checked. */
type Value =
  | 'bool' // true, false
  | 'bool-undefined' // true, false, undefined
  | 'int'
  | 'number'
  | 'string'
  | 'idref'
  | 'idrefs'
  | string[] // one of these tokens
  | { list: string[] }; // any of these tokens, space-separated

/** Every ARIA 1.2 state and property (and 1.3's that Chrome knows), with
 * how its value is checked. */
export const ATTRS: Record<string, Value> = {
  'aria-activedescendant': 'idref',
  'aria-atomic': 'bool',
  'aria-autocomplete': ['inline', 'list', 'both', 'none'],
  'aria-braillelabel': 'string',
  'aria-brailleroledescription': 'string',
  'aria-busy': 'bool',
  'aria-checked': ['true', 'false', 'mixed', 'undefined'],
  'aria-colcount': 'int',
  'aria-colindex': 'int',
  'aria-colindextext': 'string',
  'aria-colspan': 'int',
  'aria-controls': 'idrefs',
  'aria-current': ['page', 'step', 'location', 'date', 'time', 'true', 'false'],
  'aria-describedby': 'idrefs',
  'aria-description': 'string',
  'aria-details': 'idrefs',
  'aria-disabled': 'bool',
  'aria-dropeffect': { list: ['copy', 'execute', 'link', 'move', 'none', 'popup'] },
  'aria-errormessage': 'idrefs',
  'aria-expanded': 'bool-undefined',
  'aria-flowto': 'idrefs',
  'aria-grabbed': 'bool-undefined',
  'aria-haspopup': ['false', 'true', 'menu', 'listbox', 'tree', 'grid', 'dialog'],
  'aria-hidden': 'bool-undefined',
  // Any other value counts as `true`, by the spec: nothing is wrong.
  'aria-invalid': 'string',
  'aria-keyshortcuts': 'string',
  'aria-label': 'string',
  'aria-labelledby': 'idrefs',
  'aria-level': 'int',
  'aria-live': ['assertive', 'off', 'polite'],
  'aria-modal': 'bool',
  'aria-multiline': 'bool',
  'aria-multiselectable': 'bool',
  'aria-orientation': ['horizontal', 'vertical', 'undefined'],
  'aria-owns': 'idrefs',
  'aria-placeholder': 'string',
  'aria-posinset': 'int',
  'aria-pressed': ['true', 'false', 'mixed', 'undefined'],
  'aria-readonly': 'bool',
  'aria-relevant': { list: ['additions', 'removals', 'text', 'all'] },
  'aria-required': 'bool',
  'aria-roledescription': 'string',
  'aria-rowcount': 'int',
  'aria-rowindex': 'int',
  'aria-rowindextext': 'string',
  'aria-rowspan': 'int',
  'aria-selected': 'bool-undefined',
  'aria-setsize': 'int',
  'aria-sort': ['ascending', 'descending', 'none', 'other'],
  'aria-valuemax': 'number',
  'aria-valuemin': 'number',
  'aria-valuenow': 'number',
  'aria-valuetext': 'string',
};

/** Why `value` is wrong for the attribute `name`, or `null`. Ids are
 * looked up with `exists`. */
export function badValue(name: string, value: string, exists: (id: string) => boolean): string | null {
  const kind = ATTRS[name];
  const v = value.trim();
  // An empty value is the attribute left out.
  if (!kind || !v || kind === 'string') return null;
  const shown = `${name}="${value}"`;
  const tokens = (allowed: string[]) => `${shown}: not ${allowed.map((t) => `"${t}"`).join(', ').replace(/, ([^,]*)$/, ' or $1')}`;
  const lower = v.toLowerCase();
  if (kind === 'bool') return ['true', 'false'].includes(lower) ? null : tokens(['true', 'false']);
  if (kind === 'bool-undefined') {
    return ['true', 'false', 'undefined'].includes(lower) ? null : tokens(['true', 'false', 'undefined']);
  }
  if (kind === 'int') return /^-?\d+$/.test(v) ? null : `${shown}: not a whole number`;
  if (kind === 'number') return /^-?(\d+\.?\d*|\.\d+)([eE][-+]?\d+)?$/.test(v) ? null : `${shown}: not a number`;
  if (kind === 'idref' || kind === 'idrefs') {
    const ids = v.split(/\s+/);
    if (kind === 'idref' && ids.length > 1) return `${shown}: takes one id`;
    const missing = ids.filter((id) => !exists(id));
    if (!missing.length) return null;
    return `${shown}: no element has id ${missing.map((id) => `"${id}"`).join(', ')}`;
  }
  if (Array.isArray(kind)) return kind.includes(lower) ? null : tokens(kind);
  const bad = lower.split(/\s+/).find((t) => !kind.list.includes(t));
  return bad === undefined ? null : `${shown}: "${bad}" is not ${kind.list.map((t) => `"${t}"`).join(', ').replace(/, ([^,]*)$/, ' or $1')}`;
}

/** The attributes a role needs when its element has no native state
 * (ARIA 1.2's required states and properties). */
export const REQUIRED_ATTRS: Record<string, string[]> = {
  checkbox: ['aria-checked'],
  combobox: ['aria-expanded'],
  heading: ['aria-level'],
  menuitemcheckbox: ['aria-checked'],
  menuitemradio: ['aria-checked'],
  meter: ['aria-valuenow'],
  radio: ['aria-checked'],
  scrollbar: ['aria-controls', 'aria-valuenow'],
  slider: ['aria-valuenow'],
  switch: ['aria-checked'],
};

/** The roles a role must own at least one of (ARIA 1.2's required owned
 * elements); nothing else may be among its owned elements. */
export const REQUIRED_CHILDREN: Record<string, string[]> = {
  feed: ['article'],
  grid: ['row', 'rowgroup'],
  list: ['listitem'],
  listbox: ['option', 'group'],
  menu: ['menuitem', 'menuitemcheckbox', 'menuitemradio', 'group', 'separator'],
  menubar: ['menuitem', 'menuitemcheckbox', 'menuitemradio', 'group', 'separator'],
  row: ['cell', 'columnheader', 'gridcell', 'rowheader'],
  rowgroup: ['row'],
  table: ['row', 'rowgroup', 'caption'],
  tablist: ['tab'],
  tree: ['treeitem', 'group'],
  treegrid: ['row', 'rowgroup'],
};

/** The roles one of which a role must be owned by (ARIA 1.2's required
 * context roles). */
export const REQUIRED_PARENT: Record<string, string[]> = {
  cell: ['row'],
  columnheader: ['row'],
  gridcell: ['row'],
  listitem: ['list', 'directory'],
  menuitem: ['menu', 'menubar', 'group'],
  menuitemcheckbox: ['menu', 'menubar', 'group'],
  menuitemradio: ['menu', 'menubar', 'group'],
  option: ['listbox', 'group'],
  row: ['grid', 'rowgroup', 'table', 'treegrid'],
  rowgroup: ['grid', 'table', 'treegrid'],
  rowheader: ['row'],
  tab: ['tablist'],
  treeitem: ['tree', 'group'],
};

/** HTML's autofill field names. */
const FIELDS = new Set([
  'name', 'honorific-prefix', 'given-name', 'additional-name', 'family-name', 'honorific-suffix',
  'nickname', 'username', 'new-password', 'current-password', 'one-time-code', 'organization-title',
  'organization', 'street-address', 'address-line1', 'address-line2', 'address-line3',
  'address-level4', 'address-level3', 'address-level2', 'address-level1', 'country', 'country-name',
  'postal-code', 'cc-name', 'cc-given-name', 'cc-additional-name', 'cc-family-name', 'cc-number',
  'cc-exp', 'cc-exp-month', 'cc-exp-year', 'cc-csc', 'cc-type', 'transaction-currency',
  'transaction-amount', 'language', 'bday', 'bday-day', 'bday-month', 'bday-year', 'sex', 'url',
  'photo',
]);
/** The field names that may have a `home`, `work`, … before them. */
const CONTACT = new Set([
  'tel', 'tel-country-code', 'tel-national', 'tel-area-code', 'tel-local', 'tel-local-prefix',
  'tel-local-suffix', 'tel-extension', 'email', 'impp',
]);

/** Why an `autocomplete` value is not one HTML defines, or `null`:
 * `on`, `off`, or `[section-*] [shipping|billing] [home|work|mobile|fax|pager]
 * <field> [webauthn]`. */
export function badAutocomplete(value: string): string | null {
  const tokens = value.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (!tokens.length) return null;
  if (tokens.length === 1 && (tokens[0] === 'on' || tokens[0] === 'off')) return null;
  let i = 0;
  if (tokens[i]?.startsWith('section-')) i++;
  if (tokens[i] === 'shipping' || tokens[i] === 'billing') i++;
  const contact = ['home', 'work', 'mobile', 'fax', 'pager'].includes(tokens[i]);
  if (contact) i++;
  const field = tokens[i];
  const known = field !== undefined && ((!contact && FIELDS.has(field)) || CONTACT.has(field));
  if (!known) {
    if (field === undefined) return `autocomplete="${value}": no field name`;
    if (contact && FIELDS.has(field)) return `autocomplete="${value}": "${tokens[i - 1]}" only goes with tel, email or impp`;
    const near = closest(field, [...FIELDS, ...CONTACT]);
    return `autocomplete="${value}": "${field}" is not a field name${near ? ` (did you mean ${near}?)` : ''}`;
  }
  i++;
  if (tokens[i] === 'webauthn') i++;
  if (i < tokens.length) return `autocomplete="${value}": "${tokens[i]}" is out of place`;
  return null;
}

/** The shortest known word that starts with `word` (`nav`:
 * `navigation`), else the known word at most 2 edits from it, if one
 * is. */
export function closest(word: string, known: Iterable<string>): string | null {
  const words = Array.from(known);
  if (word.length >= 3) {
    const longer = words.filter((k) => k.startsWith(word)).sort((a, b) => a.length - b.length)[0];
    if (longer) return longer;
  }
  let best: string | null = null;
  let bestDistance = 3;
  for (const k of words) {
    const d = distance(word, k);
    if (d < bestDistance) {
      best = k;
      bestDistance = d;
    }
  }
  return best;
}

/** Levenshtein distance. */
function distance(a: string, b: string): number {
  let prev = Array.from({ length: b.length + 1 }, (_, j) => j);
  for (let i = 1; i <= a.length; i++) {
    const row = [i];
    for (let j = 1; j <= b.length; j++) {
      row.push(Math.min(prev[j] + 1, row[j - 1] + 1, prev[j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1)));
    }
    prev = row;
  }
  return prev[b.length];
}
