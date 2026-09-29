<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp text`

Print the visible text of the page, or of every element a locator matches

```text
chrome-cdp text [OPTIONS] [LOCATOR]
```

Arguments:

- `<LOCATOR>`: The element(s): a ref (e12) or any locator; the page if omitted

Options:

- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
The rendered text (innerText): hidden content left out, runs of spaces
collapsed, lines trimmed, at most one empty line in a row. With a locator
that matches several elements, one text per match, in page order. A
locator that names a frame (frame=payment, or an iframe's ref) reads that
frame's document. Needs a running session.

Examples:
  chrome-cdp text
  chrome-cdp text e12
  chrome-cdp text '#results'
  chrome-cdp text 'role=listitem'           # one line per item
  chrome-cdp text frame=payment
```

