<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp dblclick`

Double-click an element (click --count 2)

```text
chrome-cdp dblclick [OPTIONS] <LOCATOR>
```

Arguments:

- `<LOCATOR>`: The element: a ref (e12) or a locator that matches one

Options:

- `--modifiers <KEYS>`: Keys held during it: Alt, Control, Meta, Shift, or Mod (Meta on macOS, Control elsewhere); comma-separated
- `--position <X,Y>`: Where on the element, in CSS pixels from the top-left corner of its box, instead of its centre
- `--force`: Skip the checks (visible, stable, enabled, not covered); still scroll the element into view
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)
- `--button <BUTTON>`: The mouse button (one of `left`, `right`, `middle`; default `left`)

```text
Two clicks in a row, as a user makes them: the page gets mousedown,
mouseup and click twice, then dblclick. Waits like click. Needs a running
session.

Examples:
  chrome-cdp dblclick e6
  chrome-cdp dblclick 'text=Rename me'
```

