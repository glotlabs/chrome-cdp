<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp html`

Print the HTML of the document or of an element, pretty-printed

```text
chrome-cdp html [OPTIONS] [LOCATOR]
```

Arguments:

- `<LOCATOR>`: The element: a ref (e12) or a locator that matches one; the whole document if omitted

Options:

- `--outer`: Include the element itself, not only its content
- `--raw`: The browser's own serialisation, not pretty-printed
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
An element's content (its innerHTML) unless --outer; without a locator,
the whole document. One element per line, two spaces per level, text
collapsed, and long attribute values, comments and script or style bodies
cut; open shadow roots show as #shadow-root. --raw prints the browser's
own serialisation instead. Needs a running session.

Examples:
  chrome-cdp html e12
  chrome-cdp html '#cart' --outer
  chrome-cdp html                           # the whole document
  chrome-cdp html form --raw
```

