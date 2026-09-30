// What the read commands take from an element (plan 04, milestone 4.5):
// its visible text, its HTML pretty-printed, a field's value, and the
// page's readable content as Markdown.

import { SKIP, childrenOf, descendants, norm, parentOf, tag, visible } from './dom';

/** Tidies rendered text: no-break spaces and runs of spaces become one
 * space, lines are trimmed, and more than one empty line in a row
 * becomes one. */
export function tidy(text: string): string {
  return text
    .replace(/ /g, ' ')
    .split(/\r?\n/)
    .map((line) => line.replace(/[ \t\f\v\r]+/g, ' ').trim())
    .join('\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}

/** Where a child frame's content goes in what `text` and `markdown`
 * return: `\u0000` and the frame's index in `owners`. The engine puts the
 * frame's own text there. */
export const frameMark = (i: number): string => `\u0000${i}\u0000`;

/** The index in `owners` of `el` if it is one of them and shown. */
function ownerIndex(el: Element, owners: Element[]): number {
  if (!owners.length) return -1;
  const i = owners.indexOf(el);
  return i >= 0 && visible(el) ? i : -1;
}

/** `text`: the element's (or the document body's) rendered text
 * (`innerText`), tidied; empty when it is not rendered. `innerText` does
 * not reach into shadow roots, so the elements around open shadow roots
 * are walked here, and the rest read with `innerText`. Each of `owners`
 * (the `<iframe>`s of child frames) that shows gets a line with its
 * [`frameMark`]. */
export function text(node: Element | Document, owners: Element[] = []): string {
  const el = node instanceof Document ? node.body || node.documentElement : node;
  if (!el) return '';
  if (!(el instanceof HTMLElement)) return tidy(el.textContent || '');
  if (!visible(el)) return '';
  const around = new Set<Element>();
  const mark = (d: Element) => {
    for (let a: Element | null = d; a && !around.has(a); a = a === el ? null : parentOf(a)) around.add(a);
  };
  for (const d of descendants(el)) {
    if (d.shadowRoot || ownerIndex(d, owners) >= 0) mark(d);
  }
  if (!around.size) return tidy(el.innerText);
  let out = '';
  const walk = (parent: Element) => {
    for (const child of childrenOf(parent)) {
      if (child.nodeType === Node.TEXT_NODE) {
        out += (child as Text).data.replace(/\s+/g, ' ');
        continue;
      }
      if (!(child instanceof HTMLElement) || SKIP.has(tag(child))) continue;
      const frame = ownerIndex(child, owners);
      if (frame >= 0) {
        out += `\n${frameMark(frame)}\n`;
        continue;
      }
      const style = getComputedStyle(child);
      if (style.display === 'none') continue;
      const block = style.display !== 'contents' && !style.display.startsWith('inline');
      if (block) out += '\n';
      if (tag(child) === 'br') out += '\n';
      else if (around.has(child)) walk(child);
      else out += child.innerText;
      if (block) out += '\n';
    }
  };
  walk(el);
  return tidy(out);
}

const VOID = new Set(['area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta',
  'param', 'source', 'track', 'wbr']);

/** How long an attribute value, comment, or script or style body may be
 * before `html` cuts it. */
const CUT = 200;

/** How long a line may get when an element and its text share it. */
const LINE = 120;

const cut = (s: string, n: number): string => (s.length > n ? `${s.slice(0, n)}… (${s.length} chars)` : s);

const escapeText = (s: string): string => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

const escapeAttr = (s: string): string => s.replace(/&/g, '&amp;').replace(/"/g, '&quot;');

function openTag(el: Element): string {
  let out = `<${tag(el)}`;
  for (const a of Array.from(el.attributes)) {
    out += a.value === '' ? ` ${a.name}` : ` ${a.name}="${escapeAttr(cut(a.value, CUT))}"`;
  }
  return out + '>';
}

/** `html`: the element (`outer`) or its content, one element per line,
 * two spaces per level; whitespace between tags dropped and text
 * collapsed (except in `<pre>` and `<textarea>`), long attribute values,
 * comments and script or style bodies cut. An open shadow root shows as
 * `#shadow-root (open)` before the children. */
export function html(node: Element | Document, outer: boolean): string {
  const out: string[] = [];
  if (node instanceof Document) {
    if (node.doctype) out.push(`<!doctype ${node.doctype.name}>`);
    if (node.documentElement) pretty(node.documentElement, 0, out);
  } else if (outer) {
    pretty(node, 0, out);
  } else {
    for (const child of contentOf(node)) pretty(child, 0, out);
  }
  return out.join('\n');
}

/** An element's shown children in the `html` form: its open shadow root
 * as one node, then its own children. */
function contentOf(el: Element): Node[] {
  const kids: Node[] = [];
  if (el.shadowRoot) kids.push(el.shadowRoot);
  const own = tag(el) === 'template' ? (el as HTMLTemplateElement).content.childNodes : el.childNodes;
  kids.push(...Array.from(own));
  return kids.filter((n) => !(n.nodeType === Node.TEXT_NODE && !(n as Text).data.trim()));
}

function pretty(node: Node, depth: number, out: string[]): void {
  const indent = '  '.repeat(depth);
  if (node instanceof ShadowRoot) {
    out.push(`${indent}#shadow-root (${node.mode})`);
    for (const child of Array.from(node.childNodes)) {
      if (child.nodeType === Node.TEXT_NODE && !(child as Text).data.trim()) continue;
      pretty(child, depth + 1, out);
    }
    return;
  }
  if (node.nodeType === Node.TEXT_NODE) {
    const collapsed = norm((node as Text).data);
    if (collapsed) out.push(indent + escapeText(collapsed));
    return;
  }
  if (node.nodeType === Node.COMMENT_NODE) {
    out.push(`${indent}<!-- ${cut(norm((node as Comment).data), CUT)} -->`);
    return;
  }
  if (!(node instanceof Element)) return;
  const name = tag(node);
  const open = openTag(node);
  if (VOID.has(name)) {
    out.push(indent + open);
    return;
  }
  const close = `</${name}>`;
  if (name === 'script' || name === 'style') {
    const body = (node.textContent || '').trim();
    out.push(indent + open + (body ? cut(body, CUT) : '') + close);
    return;
  }
  if (name === 'pre' || name === 'textarea') {
    out.push(indent + open + escapeText(node.textContent || '') + close);
    return;
  }
  const kids = contentOf(node);
  const line = compact(node, LINE - indent.length);
  if (line !== null) {
    out.push(indent + line);
    return;
  }
  out.push(indent + open);
  for (const child of kids) pretty(child, depth + 1, out);
  out.push(indent + close);
}

/** `el` on one line, if it fits in `room` characters and holds only
 * text and elements that fit too; else `null`. */
function compact(el: Element, room: number): string | null {
  const name = tag(el);
  if (name === 'script' || name === 'style' || name === 'pre' || name === 'textarea' || el.shadowRoot) return null;
  let line = openTag(el);
  if (VOID.has(name)) return line.length <= room ? line : null;
  const own = name === 'template' ? (el as HTMLTemplateElement).content.childNodes : el.childNodes;
  const kids = Array.from(own);
  for (let i = 0; i < kids.length && line.length <= room; i++) {
    const child = kids[i];
    if (child.nodeType === Node.TEXT_NODE) {
      let t = (child as Text).data.replace(/\s+/g, ' ');
      if (i === 0) t = t.trimStart();
      if (i === kids.length - 1) t = t.trimEnd();
      line += escapeText(t);
    } else if (child instanceof Element) {
      const inner = compact(child, room - line.length);
      if (inner === null) return null;
      line += inner;
    } else {
      return null;
    }
  }
  line += `</${name}>`;
  return line.length <= room ? line : null;
}

/** What `value` reads: a field's value, or `{error}` if the element has
 * none. */
export interface Value {
  value?: string | string[];
  kind?: string;
  error?: string;
}

/** `value`: an input's, textarea's or select's current value (a multiple
 * select's as a list), a file input's file names, an editable element's
 * text, or the value of a slider, spin button, progress bar or meter. */
export function value(el: Element): Value {
  if (el instanceof HTMLInputElement) {
    if (el.type === 'file') {
      return { value: Array.from(el.files || []).map((f) => f.name), kind: 'file' };
    }
    return { value: el.value, kind: el.type === 'password' ? 'password' : 'input' };
  }
  if (el instanceof HTMLTextAreaElement) return { value: el.value, kind: 'textarea' };
  if (el instanceof HTMLSelectElement) {
    const chosen = Array.from(el.selectedOptions).map((o) => o.value);
    return { value: el.multiple ? chosen : chosen[0] ?? '', kind: 'select' };
  }
  if (el instanceof HTMLOutputElement) return { value: el.value, kind: 'output' };
  if (el instanceof HTMLProgressElement || el instanceof HTMLMeterElement) {
    return { value: String(el.value), kind: tag(el) };
  }
  if (el instanceof HTMLElement && el.isContentEditable) {
    return { value: tidy(el.innerText), kind: 'contenteditable' };
  }
  const now = el.getAttribute('aria-valuetext') ?? el.getAttribute('aria-valuenow');
  if (now !== null) return { value: now, kind: 'aria' };
  return { error: `it is a <${tag(el)}>, not a field` };
}

// Markdown: readability-style extraction, then conversion.

/** Blocks the conversion never shows. */
const DROP = new Set(['script', 'style', 'noscript', 'template', 'head', 'svg', 'canvas', 'iframe',
  'frame', 'object', 'embed', 'video', 'audio', 'map', 'input', 'select', 'textarea', 'button',
  'dialog', 'datalist', 'option', 'label']);

/** The first line of a Markdown list. */
const LIST_START = /^(?:- |\d+\. )/;

/** Page furniture left out of the readable content (unless it is what
 * was asked for). */
const FURNITURE = new Set(['nav', 'aside', 'footer']);
const FURNITURE_ROLES = new Set(['navigation', 'complementary', 'contentinfo', 'search']);

/** Where `markdown` found the content, and the content. */
export interface Readable {
  markdown: string;
  /** `main`, `article`, `div#content`, `body`, or the element asked for. */
  source: string;
}

/** `markdown`: `el`'s content as Markdown; for the document, its main
 * content (a single `<main>` or `[role=main]`, else a single `<article>`,
 * else the element that holds the most paragraph text), without page
 * furniture. Each of `owners` (the `<iframe>`s of child frames) that
 * shows is a block with its [`frameMark`]. */
export function markdown(el: Element | Document, owners: Element[] = []): Readable {
  const readable = el instanceof Document;
  const root = el instanceof Document ? mainContent() : el;
  const md = new Markdown(readable, owners);
  const blocks: string[] = [];
  md.blocks(root, blocks);
  return { markdown: blocks.join('\n\n').replace(/\n{3,}/g, '\n\n').trim(), source: describeEl(root) };
}

function describeEl(el: Element): string {
  let out = tag(el);
  if (el.id) out += `#${el.id}`;
  else if (typeof el.className === 'string' && el.className.trim()) out += `.${el.className.trim().split(/\s+/)[0]}`;
  return out;
}

function mainContent(): Element {
  const body = document.body || document.documentElement;
  const one = (selector: string): Element | null => {
    const found = Array.from(document.querySelectorAll(selector)).filter(visible);
    return found.length === 1 ? found[0] : null;
  };
  const main = one('main, [role=main]') || one('article');
  if (main) return main;
  // Readability's idea, simply: paragraphs score their parent fully and
  // their grandparent half; the best-scoring element holds the content.
  const scores = new Map<Element, number>();
  for (const p of Array.from(body.querySelectorAll('p, pre, blockquote, td, li'))) {
    const t = norm(p.textContent);
    if (t.length < 25 || !visible(p)) continue;
    const score = 1 + t.split(',').length + Math.min(Math.floor(t.length / 100), 3);
    const parent = p.parentElement;
    if (!parent) continue;
    scores.set(parent, (scores.get(parent) || 0) + score);
    const grand = parent.parentElement;
    if (grand) scores.set(grand, (scores.get(grand) || 0) + score / 2);
  }
  let best: Element | null = null;
  let top = 0;
  for (const [candidate, score] of scores) {
    if (score > top) {
      best = candidate;
      top = score;
    }
  }
  return best && top >= 5 ? best : body;
}

class Markdown {
  private base = location.href;

  /** `readable`: leave page furniture (navigation, sidebars, footers)
   * out. `owners`: the `<iframe>`s whose frames' content goes in. */
  constructor(
    private readable: boolean,
    private owners: Element[],
  ) {}

  private display(el: Element): string {
    return (el.ownerDocument.defaultView as Window).getComputedStyle(el).display;
  }

  private shown(el: Element): boolean {
    if (DROP.has(tag(el))) return false;
    if (el.getAttribute('aria-hidden') === 'true') return false;
    return this.display(el) !== 'none';
  }

  private furniture(el: Element): boolean {
    if (!this.readable) return false;
    const role = el.getAttribute('role');
    return FURNITURE.has(tag(el)) || (role !== null && FURNITURE_ROLES.has(role));
  }

  private isBlock(el: Element): boolean {
    const display = this.display(el);
    return display !== 'contents' && !display.startsWith('inline') && display !== 'none';
  }

  /** Appends `el`'s content as Markdown blocks to `out`. */
  blocks(el: Element, out: string[]): void {
    let run = '';
    const flush = () => {
      const paragraph = tidyInline(run);
      if (paragraph) out.push(paragraph);
      run = '';
    };
    for (const child of childrenOf(el)) {
      if (child.nodeType === Node.TEXT_NODE) {
        run += collapse((child as Text).data);
        continue;
      }
      if (!(child instanceof Element)) continue;
      const frame = ownerIndex(child, this.owners);
      if (frame >= 0) {
        flush();
        out.push(frameMark(frame));
        continue;
      }
      if (!this.shown(child) || this.furniture(child)) continue;
      if (this.display(child) === 'contents') {
        flush();
        this.blocks(child, out);
        continue;
      }
      if (!this.isBlock(child) && tag(child) !== 'table') {
        run += this.inline(child);
        continue;
      }
      flush();
      this.block(child, out);
    }
    flush();
  }

  private block(el: Element, out: string[]): void {
    const name = tag(el);
    const heading = /^h([1-6])$/.exec(name);
    if (heading) {
      const t = tidyInline(this.inlineChildren(el)).replace(/\n+/g, ' ');
      if (t) out.push(`${'#'.repeat(Number(heading[1]))} ${t}`);
      return;
    }
    switch (name) {
      case 'hr':
        out.push('---');
        return;
      case 'br':
        return;
      case 'img': {
        const img = this.inline(el);
        if (img) out.push(img);
        return;
      }
      case 'pre': {
        const code = el.querySelector('code');
        const lang = /(?:^|\s)(?:language|lang)-(\S+)/.exec((code || el).className || '');
        const body = ((el as HTMLElement).innerText || el.textContent || '').replace(/\n+$/, '');
        out.push('```' + (lang ? lang[1] : '') + '\n' + body + '\n```');
        return;
      }
      case 'ul':
      case 'ol':
        this.list(el, name === 'ol', out);
        return;
      case 'blockquote': {
        const inner: string[] = [];
        this.blocks(el, inner);
        const quoted = inner.join('\n\n').split('\n').map((l) => (l ? `> ${l}` : '>')).join('\n');
        if (inner.length) out.push(quoted);
        return;
      }
      case 'table':
        this.table(el as HTMLTableElement, out);
        return;
      case 'dt': {
        const t = tidyInline(this.inlineChildren(el));
        if (t) out.push(`**${t}**`);
        return;
      }
      case 'dd': {
        const inner: string[] = [];
        this.blocks(el, inner);
        if (inner.length) out.push(indent(inner.join('\n\n'), ': ', '  '));
        return;
      }
      default:
        this.blocks(el, out);
    }
  }

  private list(el: Element, ordered: boolean, out: string[]): void {
    const items: string[] = [];
    let n = ordered ? Number(el.getAttribute('start') || 1) : 0;
    for (const child of childrenOf(el)) {
      if (!(child instanceof Element) || !this.shown(child)) continue;
      const inner: string[] = [];
      if (tag(child) === 'li') this.blocks(child, inner);
      else this.block(child, inner);
      // A nested list stays next to the line before it.
      const body = inner.reduce((all, b) => (!all ? b : all + (LIST_START.test(b) ? '\n' : '\n\n') + b), '');
      if (!body) continue;
      const marker = ordered ? `${n++}. ` : '- ';
      items.push(indent(body, marker, ' '.repeat(marker.length)));
    }
    // Items that are one paragraph each stay together (a tight list).
    if (items.length) out.push(items.join(items.some((i) => i.includes('\n\n')) ? '\n\n' : '\n'));
  }

  private table(table: HTMLTableElement, out: string[]): void {
    const rows = Array.from(table.rows).filter((r) => this.shown(r));
    if (!rows.length) return;
    const cells = rows.map((r) =>
      Array.from(r.cells).map((c) => tidyInline(this.inlineChildren(c)).replace(/\n+/g, ' ').replace(/\|/g, '\\|')));
    const width = Math.max(...cells.map((r) => r.length));
    if (width === 0) return;
    const line = (r: string[]) => `| ${Array.from({ length: width }, (_, i) => r[i] ?? '').join(' | ')} |`;
    const header = cells[0];
    const lines = [line(header), `|${' --- |'.repeat(width)}`, ...cells.slice(1).map(line)];
    const caption = table.caption ? tidyInline(this.inlineChildren(table.caption)) : '';
    if (caption) out.push(`**${caption}**`);
    out.push(lines.join('\n'));
  }

  private inlineChildren(el: Element): string {
    let out = '';
    for (const child of childrenOf(el)) out += this.inline(child);
    return out;
  }

  /** `node` as inline Markdown; a block inside inline content becomes
   * a line break. */
  private inline(node: Node): string {
    if (node.nodeType === Node.TEXT_NODE) return collapse((node as Text).data);
    if (!(node instanceof Element)) return '';
    const frame = ownerIndex(node, this.owners);
    if (frame >= 0) return `\n${frameMark(frame)}\n`;
    if (!this.shown(node) || this.furniture(node)) return '';
    const name = tag(node);
    if (name === 'br') return '\n';
    if (name === 'img') {
      const alt = norm(node.getAttribute('alt'));
      const src = node.getAttribute('src') || '';
      if (!alt && (!src || src.startsWith('data:'))) return '';
      return `![${alt}](${src.startsWith('data:') ? '' : this.href(src)})`;
    }
    const inner = this.inlineChildren(node);
    const wrapped = this.isBlock(node) ? `\n${inner}\n` : inner;
    const t = norm(inner);
    if (!t) return wrapped.includes('![') ? wrapped : ' ';
    switch (name) {
      case 'a': {
        const href = node.getAttribute('href');
        if (!href || /^javascript:/i.test(href)) return wrapped;
        return edge(inner, `[${tidyInline(inner).replace(/\n+/g, ' ')}](${this.href(href)})`);
      }
      case 'strong':
      case 'b':
        return edge(inner, `**${t}**`);
      case 'em':
      case 'i':
        return edge(inner, `_${t}_`);
      case 'code':
      case 'kbd':
      case 'samp':
        return edge(inner, t.includes('`') ? `\`\` ${t} \`\`` : `\`${t}\``);
      case 'del':
      case 's':
        return edge(inner, `~~${t}~~`);
      default:
        return wrapped;
    }
  }

  /** A link as the snapshot shows it: relative (path, query, fragment)
   * when on the page's origin. */
  private href(raw: string): string {
    let url: URL;
    try {
      url = new URL(raw, this.base);
    } catch {
      return raw;
    }
    const here = new URL(this.base);
    if (url.origin === here.origin && /^(https?|file):$/.test(url.protocol)) {
      return url.pathname + url.search + url.hash;
    }
    return url.href;
  }
}

/** Text as rendered outside `<pre>`: whitespace runs are one space. */
const collapse = (s: string): string => s.replace(/[ \t\n\r\f]+/g, ' ');

/** Keeps the spaces around an inline element's text outside its marks:
 * `a <b> bold </b> word` → `a **bold** word`. */
function edge(raw: string, marked: string): string {
  return (/^\s/.test(raw) ? ' ' : '') + marked + (/\s$/.test(raw) ? ' ' : '');
}

/** Collapses a paragraph's whitespace, keeping its line breaks. */
function tidyInline(s: string): string {
  return s
    .replace(/ /g, ' ')
    .split('\n')
    .map((l) => l.replace(/[ \t\f\v\r]+/g, ' ').trim())
    .filter((l, i, all) => l || (i > 0 && all[i - 1]))
    .join('\n')
    .trim();
}

/** Puts `first` before the first line of `body` and `rest` before the
 * others (a list item's marker and its continuation). */
function indent(body: string, first: string, rest: string): string {
  return body
    .split('\n')
    .map((l, i) => (i === 0 ? first + l : l ? rest + l : l))
    .join('\n');
}
