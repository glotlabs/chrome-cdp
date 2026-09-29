<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp markdown`

Print the page's readable content (or an element's) as Markdown

```text
chrome-cdp markdown [OPTIONS] [LOCATOR]
```

Arguments:

- `<LOCATOR>`: The element: a ref (e12) or a locator that matches one; the page's main content if omitted

Options:

- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Without a locator, the main content: a single <main> (or role=main), else
a single <article>, else the element that holds the most paragraph text;
navigation, sidebars and footers inside it are left out. Headings, lists,
links (relative on the page's origin), images, code, quotes and tables are
kept; controls are not. A locator that names a frame reads that frame's
main content. Needs a running session.

Examples:
  chrome-cdp markdown
  chrome-cdp markdown e40
  chrome-cdp markdown 'css=article' --max-chars 0
```

