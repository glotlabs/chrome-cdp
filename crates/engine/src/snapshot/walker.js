// chrome-cdp's DOM walker (plan 04, milestone 4.3, engine B), called with
// Runtime.callFunctionOn in a frame's isolated world. It walks the frame's
// document (open shadow roots and slotted content included), works out
// each visible node's ARIA role, accessible name (a compact take on
// accname 1.2), value and states, and returns
//   {json: '{"title","url","nodes":[…]}', els: [Element, …]}
// `nodes` is flat, in document order; each has `p` (its parent's index,
// -1 at the top). Nodes the engine must address (controls, clickable and
// scrollable elements, iframes, and `scope`) point into `els` with `e`, so
// the engine can get their backendNodeIds with DOM.describeNode.
function (scope) {
  const view = document.defaultView;
  const nodes = [];
  const els = [];
  const active = (() => {
    let a = document.activeElement;
    while (a && a.shadowRoot && a.shadowRoot.activeElement) a = a.shadowRoot.activeElement;
    return a;
  })();

  const SKIP = new Set(['script', 'style', 'template', 'noscript', 'head', 'meta', 'link', 'title']);
  const INTERACTIVE = new Set(['button', 'link', 'textbox', 'searchbox', 'checkbox', 'radio',
    'switch', 'combobox', 'listbox', 'option', 'menuitem', 'menuitemcheckbox', 'menuitemradio',
    'tab', 'slider', 'spinbutton', 'treeitem', 'iframe']);
  const FROM_CONTENT = new Set(['button', 'link', 'heading', 'tab', 'option', 'menuitem',
    'menuitemcheckbox', 'menuitemradio', 'treeitem', 'checkbox', 'radio', 'switch', 'cell',
    'gridcell', 'columnheader', 'rowheader', 'tooltip', 'row']);
  const KNOWN = new Set([...INTERACTIVE, ...FROM_CONTENT, 'alert', 'alertdialog', 'application',
    'article', 'banner', 'blockquote', 'caption', 'code', 'complementary', 'contentinfo',
    'definition', 'deletion', 'dialog', 'directory', 'document', 'emphasis', 'feed', 'figure',
    'form', 'generic', 'grid', 'group', 'img', 'image', 'insertion', 'list', 'listitem', 'log',
    'main', 'marquee', 'math', 'menu', 'menubar', 'meter', 'navigation', 'none', 'note',
    'paragraph', 'presentation', 'progressbar', 'radiogroup', 'region', 'rowgroup', 'scrollbar',
    'search', 'separator', 'status', 'strong', 'subscript', 'superscript', 'table', 'tablist',
    'tabpanel', 'term', 'time', 'timer', 'toolbar', 'tree', 'treegrid']);
  const INPUT = {
    button: 'button', submit: 'button', reset: 'button', image: 'button', file: 'button',
    checkbox: 'checkbox', radio: 'radio', range: 'slider', number: 'spinbutton',
    search: 'searchbox', color: 'button',
  };

  const norm = (s) => String(s == null ? '' : s).replace(/\s+/g, ' ').trim();
  const attr = (el, name) => el.getAttribute(name);
  const ariaTrue = (el, name) => attr(el, name) === 'true';
  const tag = (el) => el.localName;
  const scoped = (el) => el.closest('article, aside, main, nav, section');

  function implicitRole(el) {
    const t = tag(el);
    switch (t) {
      case 'a': case 'area': return el.hasAttribute('href') ? 'link' : 'generic';
      case 'article': return 'article';
      case 'aside': return 'complementary';
      case 'button': return 'button';
      case 'datalist': return 'listbox';
      case 'dd': return 'definition';
      case 'dl': return 'descriptionlist';
      case 'details': return 'group';
      case 'dfn': case 'dt': return 'term';
      case 'dialog': return 'dialog';
      case 'fieldset': case 'optgroup': return 'group';
      case 'figure': return 'figure';
      case 'figcaption': return 'figcaption';
      case 'footer': return scoped(el.parentElement || el) ? 'generic' : 'contentinfo';
      case 'header': return scoped(el.parentElement || el) ? 'generic' : 'banner';
      case 'form': return 'form';
      case 'h1': case 'h2': case 'h3': case 'h4': case 'h5': case 'h6': return 'heading';
      case 'hr': return 'separator';
      case 'iframe': case 'frame': return 'iframe';
      case 'img': return attr(el, 'alt') === '' && !el.hasAttribute('title') ? 'presentation' : 'img';
      case 'input': {
        const type = (attr(el, 'type') || 'text').toLowerCase();
        if (type === 'hidden') return null;
        if (el.hasAttribute('list') && !INPUT[type]) return 'combobox';
        return INPUT[type] || 'textbox';
      }
      case 'label': return 'label';
      case 'legend': return 'legend';
      case 'li': return 'listitem';
      case 'main': return 'main';
      case 'menu': case 'ol': case 'ul': return 'list';
      case 'meter': return 'meter';
      case 'nav': return 'navigation';
      case 'option': return 'option';
      case 'output': return 'status';
      case 'p': return 'paragraph';
      case 'progress': return 'progressbar';
      case 'search': return 'search';
      case 'section': return el.hasAttribute('aria-label') || el.hasAttribute('aria-labelledby') ? 'region' : 'generic';
      case 'select': return el.multiple || el.size > 1 ? 'listbox' : 'combobox';
      case 'summary': return 'button';
      case 'table': return 'table';
      case 'tbody': case 'thead': case 'tfoot': return 'rowgroup';
      case 'td': return el.closest('table[role=grid], [role=grid]') ? 'gridcell' : 'cell';
      case 'th': return attr(el, 'scope') === 'row' ? 'rowheader' : 'columnheader';
      case 'textarea': return 'textbox';
      case 'tr': return 'row';
      case 'caption': return 'caption';
      case 'blockquote': return 'blockquote';
      case 'code': return 'code';
      case 'em': return 'emphasis';
      case 'strong': return 'strong';
      case 'time': return 'time';
      case 'svg': return 'img';
      case 'math': return 'math';
      case 'html': case 'body': return 'none';
      default:
        if (el.isContentEditable && !(el.parentElement && el.parentElement.isContentEditable)) return 'textbox';
        return 'generic';
    }
  }

  function roleOf(el) {
    const explicit = (attr(el, 'role') || '').trim().toLowerCase().split(/\s+/).find((r) => KNOWN.has(r));
    if (explicit === 'presentation' || explicit === 'none') {
      // Focusable elements keep their role (ARIA's conflict resolution).
      return el.tabIndex >= 0 && el.hasAttribute('tabindex') ? implicitRole(el) : 'none';
    }
    if (explicit === 'image') return 'img';
    return explicit || implicitRole(el);
  }

  function hidden(el, style) {
    return style.display === 'none' || ariaTrue(el, 'aria-hidden') || el.hasAttribute('inert');
  }

  // The text an element contributes to a name (accname's step 2F/2G).
  // A tree item's name leaves out its nested group (as Chrome does).
  function textOf(node, skip, depth, treeitem) {
    if (depth > 30) return '';
    if (node.nodeType === Node.TEXT_NODE) return node.data;
    if (node.nodeType !== Node.ELEMENT_NODE && node.nodeType !== Node.DOCUMENT_FRAGMENT_NODE) return '';
    if (node.nodeType === Node.ELEMENT_NODE) {
      const el = node;
      if (el === skip || SKIP.has(tag(el))) return '';
      if (treeitem && (attr(el, 'role') === 'group' || attr(el, 'role') === 'treeitem')) return '';
      const style = view.getComputedStyle(el);
      if (hidden(el, style) || style.visibility === 'hidden') return '';
      const label = attr(el, 'aria-label');
      if (label && label.trim()) return ' ' + label + ' ';
      const t = tag(el);
      if (t === 'img' || (t === 'input' && attr(el, 'type') === 'image')) return ' ' + (attr(el, 'alt') || '') + ' ';
      if (t === 'input' || t === 'textarea') {
        const role = roleOf(el);
        if (role === 'textbox' || role === 'searchbox' || role === 'spinbutton' || role === 'slider') return ' ' + el.value + ' ';
        if (role === 'button') return ' ' + (el.value || '') + ' ';
        return '';
      }
      if (t === 'select') {
        const opt = el.selectedOptions && el.selectedOptions[0];
        return opt ? ' ' + opt.text + ' ' : '';
      }
      if (t === 'br') return ' ';
      if (t === 'svg') return ' ' + svgTitle(el) + ' ';
      const block = style.display !== 'inline' && style.display !== 'contents';
      let out = '';
      for (const child of childrenOf(el)) out += textOf(child, skip, depth + 1, treeitem);
      return block ? ' ' + out + ' ' : out;
    }
    let out = '';
    for (const child of node.childNodes) out += textOf(child, skip, depth + 1, treeitem);
    return out;
  }

  function svgTitle(el) {
    const title = Array.from(el.children).find((c) => tag(c) === 'title');
    return title ? title.textContent : '';
  }

  function byIds(el, ids) {
    const root = el.getRootNode();
    return ids.split(/\s+/).map((id) => (root.getElementById ? root.getElementById(id) : document.getElementById(id)))
      .filter(Boolean);
  }

  function nameOf(el, role) {
    const labelledby = attr(el, 'aria-labelledby');
    if (labelledby) {
      const text = norm(byIds(el, labelledby).map((l) => textOf(l, null, 0)).join(' '));
      if (text) return text;
    }
    const label = norm(attr(el, 'aria-label'));
    if (label) return label;
    const t = tag(el);
    if (t === 'input' || t === 'textarea' || t === 'select' || t === 'meter' || t === 'progress' || t === 'output') {
      const type = (attr(el, 'type') || '').toLowerCase();
      if (t === 'input' && ['button', 'submit', 'reset'].includes(type)) {
        return norm(el.value) || (type === 'submit' ? 'Submit' : type === 'reset' ? 'Reset' : '');
      }
      if (t === 'input' && type === 'image') return norm(attr(el, 'alt')) || 'Submit';
      const labels = el.labels ? Array.from(el.labels) : [];
      const text = norm(labels.map((l) => textOf(l, el, 0)).join(' '));
      if (text) return text;
      return norm(attr(el, 'title')) || norm(attr(el, 'placeholder'));
    }
    if (t === 'img' || t === 'area') return norm(attr(el, 'alt')) || norm(attr(el, 'title'));
    if (t === 'svg') return norm(svgTitle(el)) || norm(attr(el, 'title'));
    if (t === 'fieldset') {
      const legend = Array.from(el.children).find((c) => tag(c) === 'legend');
      if (legend) return norm(textOf(legend, null, 0));
    }
    if (t === 'table') {
      const caption = el.caption;
      if (caption) return norm(textOf(caption, null, 0));
    }
    if (t === 'iframe') return norm(attr(el, 'title'));
    if (FROM_CONTENT.has(role) && role !== 'row') {
      let text = '';
      for (const child of childrenOf(el)) text += textOf(child, null, 1, role === 'treeitem');
      text = norm(text);
      if (text) return text;
    }
    return norm(attr(el, 'title'));
  }

  function childrenOf(el) {
    if (el.shadowRoot) return Array.from(el.shadowRoot.childNodes);
    if (tag(el) === 'slot') {
      const assigned = el.assignedNodes({ flatten: true });
      return assigned.length ? assigned : Array.from(el.childNodes);
    }
    if (tag(el) === 'details' && !el.open) {
      return Array.from(el.children).filter((c) => tag(c) === 'summary').slice(0, 1);
    }
    return Array.from(el.childNodes);
  }

  function valueOf(el, role) {
    const now = attr(el, 'aria-valuetext') || attr(el, 'aria-valuenow');
    const t = tag(el);
    if (t === 'input' || t === 'textarea') return el.value;
    if (t === 'select') {
      const opt = el.selectedOptions && el.selectedOptions[0];
      return opt ? opt.text : '';
    }
    if (role === 'textbox' && el.isContentEditable) return el.innerText;
    return now;
  }

  function statesOf(el, role) {
    const s = {};
    const t = tag(el);
    if (t === 'input' && (el.type === 'checkbox' || el.type === 'radio')) {
      s.c = el.indeterminate ? 'mixed' : el.checked ? 'true' : 'false';
    } else if (['checkbox', 'radio', 'switch', 'menuitemcheckbox', 'menuitemradio'].includes(role)) {
      const c = attr(el, 'aria-checked');
      s.c = c === 'true' || c === 'mixed' ? c : 'false';
    }
    if (el.hasAttribute('aria-expanded')) s.x = ariaTrue(el, 'aria-expanded');
    else if (t === 'summary' && el.parentElement && tag(el.parentElement) === 'details') s.x = el.parentElement.open;
    else if (t === 'select' && role === 'combobox') s.x = false;
    if (t === 'option' ? el.selected : ariaTrue(el, 'aria-selected')) s.s = 1;
    if ((el.matches && el.matches(':disabled')) || el.closest('[aria-disabled="true"]')) s.d = 1;
    if (el.readOnly || ariaTrue(el, 'aria-readonly')) s.r = 1;
    if (el.required || ariaTrue(el, 'aria-required')) s.q = 1;
    const invalid = attr(el, 'aria-invalid');
    if (invalid && invalid !== 'false') s.i = 1;
    if (el === active && el !== document.body) s.f = 1;
    if (ariaTrue(el, 'aria-pressed')) s.p = 1;
    return s;
  }

  function level(el, role) {
    if (role !== 'heading') return undefined;
    const aria = parseInt(attr(el, 'aria-level'), 10);
    if (aria > 0) return aria;
    const m = /^h([1-6])$/.exec(tag(el));
    return m ? Number(m[1]) : 2;
  }

  function scrolls(el, style) {
    if (tag(el) === 'html' || tag(el) === 'body') return false;
    const over = (v) => v === 'auto' || v === 'scroll' || v === 'overlay';
    return (over(style.overflowY) && el.scrollHeight - el.clientHeight > 1)
      || (over(style.overflowX) && el.scrollWidth - el.clientWidth > 1);
  }

  // `invisible`: text right here is not shown (its element is visibility:
  // hidden). `mute`: text anywhere below is not shown: it is in a label
  // that names its control, so the control's name says it.
  function walk(node, parent, parentStyle, invisible, mute) {
    if (nodes.length > 200000) return;
    if (node.nodeType === Node.TEXT_NODE) {
      if (!invisible && !mute && node.data) nodes.push({ p: parent, t: 1, n: node.data });
      return;
    }
    if (node.nodeType !== Node.ELEMENT_NODE) return;
    const el = node;
    if (SKIP.has(tag(el))) return;
    if (tag(el) === 'br') {
      if (!invisible && !mute) nodes.push({ p: parent, t: 1, n: '\n' });
      return;
    }
    const style = view.getComputedStyle(el);
    if (hidden(el, style)) return;
    const role = roleOf(el);
    if (role === null) return;
    const vis = style.visibility !== 'hidden' && style.visibility !== 'collapse';
    const index = nodes.length;
    // An element that is visibility: hidden is not shown, but a child
    // that is visible again is.
    if (!vis) {
      for (const child of childrenOf(el)) walk(child, parent, style, true, mute);
      return;
    }
    const n = { p: parent, r: role };
    const name = role === 'none' || role === 'generic' || role === 'paragraph' ? norm(attr(el, 'aria-label')) : nameOf(el, role);
    if (name) n.n = name;
    const value = valueOf(el, role);
    if (value != null && value !== '') n.v = String(value);
    const lv = level(el, role);
    if (lv) n.l = lv;
    if (role === 'link' && el.href) n.h = String(el.href.baseVal !== undefined ? el.href.baseVal : el.href);
    if (tag(el) === 'input' && el.type === 'password') n.w = 1;
    const s = statesOf(el, role);
    if (Object.keys(s).length) n.s = s;
    if (style.display === 'inline' || style.display === 'contents') n.i = 1;
    const tabindex = parseInt(attr(el, 'tabindex'), 10);
    const pointerRoot = style.cursor === 'pointer' && !(parentStyle && parentStyle.cursor === 'pointer');
    if (tag(el) !== 'html' && tag(el) !== 'body'
      && (el.hasAttribute('onclick') || tabindex >= 0 || pointerRoot)) n.c = 1;
    if (scrolls(el, style)) n.x = 1;
    // Long lists get a key too: their collapsed tail names them.
    const long = el.childElementCount > 30
      || Array.from(el.children).some((c) => c.childElementCount > 30);
    if (INTERACTIVE.has(role) || n.c || n.x || el === scope || role === 'textbox' || long) {
      n.e = els.length;
      els.push(el);
    }
    nodes.push(n);
    if (role === 'iframe' || tag(el) === 'svg' || role === 'img') return;
    const muted = mute || (tag(el) === 'label' && !!el.control);
    for (const child of childrenOf(el)) walk(child, index, style, false, muted);
  }

  for (const child of document.documentElement ? [document.documentElement] : []) walk(child, -1, null, false, false);
  return {
    json: JSON.stringify({ title: document.title, url: location.href, nodes }),
    els,
  };
}
