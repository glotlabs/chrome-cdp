<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp tap`

Tap an element with a touch (needs touch emulation)

```text
chrome-cdp tap [OPTIONS] <LOCATOR>
```

Arguments:

- `<LOCATOR>`: The element: a ref (e12) or a locator that matches one

Options:

- `--modifiers <KEYS>`: Keys held during it: Alt, Control, Meta, Shift, or Mod (Meta on macOS, Control elsewhere); comma-separated
- `--position <X,Y>`: Where on the element, in CSS pixels from the top-left corner of its box, instead of its centre
- `--force`: Skip the checks (visible, stable, enabled, not covered); still scroll the element into view
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
Waits like click, then a touch starts and ends at the element's centre.
The tab must emulate a touch screen (navigator.maxTouchPoints > 0), or
the page would get no touch events; use click otherwise. Needs a running
session.

Examples:
  chrome-cdp tap e6
  chrome-cdp tap 'role=link[name="Menu"]'
```

