<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp box`

Print an element's box in the viewport: x, y, width and height

```text
chrome-cdp box [OPTIONS] <LOCATOR>
```

Arguments:

- `<LOCATOR>`: The element: a ref (e12) or a locator that matches one

Options:

- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
CSS pixels from the top-left corner of the tab's viewport (iframes'
offsets added), rounded to hundredths: `x=24 y=310 w=120 h=36`, with
`(outside the 1280x720 viewport)` when scrolled out of view. Prints
nothing and a warning when the element is not rendered. -q prints the
four numbers. An element not there yet is waited for (up to --timeout,
10s by default). Needs a running session.

Examples:
  chrome-cdp box e12
  chrome-cdp box '#checkout' -q
```

