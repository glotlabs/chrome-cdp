<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp hover`

Move the mouse over an element (menus, tooltips)

```text
chrome-cdp hover [OPTIONS] <LOCATOR>
```

Arguments:

- `<LOCATOR>`: The element: a ref (e12) or a locator that matches one

Options:

- `--modifiers <KEYS>`: Keys held during it: Alt, Control, Meta, Shift, or Mod (Meta on macOS, Control elsewhere); comma-separated
- `--position <X,Y>`: Where on the element, in CSS pixels from the top-left corner of its box, instead of its centre
- `--force`: Skip the checks (visible, stable, enabled, not covered); still scroll the element into view
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
Waits like click (except that a disabled element is fine), then moves the
mouse to the element's centre, so the page gets mouseover, mouseenter and
mousemove, and :hover styles apply. Needs a running session.

Examples:
  chrome-cdp hover e8
  chrome-cdp hover 'role=menuitem[name="File"]'
```

