<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp click`

Click an element: waits until it can be clicked, then a real mouse click

```text
chrome-cdp click [OPTIONS] <LOCATOR>
```

Arguments:

- `<LOCATOR>`: The element: a ref (e12) or a locator that matches one

Options:

- `--modifiers <KEYS>`: Keys held during it: Alt, Control, Meta, Shift, or Mod (Meta on macOS, Control elsewhere); comma-separated
- `--position <X,Y>`: Where on the element, in CSS pixels from the top-left corner of its box, instead of its centre
- `--force`: Skip the checks (visible, stable, enabled, not covered); still scroll the element into view
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)
- `--button <BUTTON>`: The mouse button (one of `left`, `right`, `middle`; default `left`)
- `--count <N>`: How many clicks in a row (2 is a double click) (default `1`)

```text
Waits (up to --timeout, 10s by default) until the element is visible,
stable, enabled and not covered, scrolls it into view, then moves the
mouse to its centre and presses and releases the button there: trusted
events, as a user makes them. When something covers it, the error names
what, with a ref, and a click that likely dismisses it. Then it waits
for what the click caused (a new page to load, the requests it sent) and
adds a line for each thing that happened: a navigation, a new tab, a
dialog, a download, a console error, a failed request, focus moving to
another form or frame. Every action does. Needs a running session.

Examples:
  chrome-cdp click e6
  chrome-cdp click e6 --snap                  # and what changed on the page
  chrome-cdp click 'role=button[name="Sign in"]'
  chrome-cdp click e6 --button right          # the context menu
  chrome-cdp click e12 --modifiers Shift      # select a range
  chrome-cdp click '#canvas' --position 10,20  # from its top-left corner
  chrome-cdp click e6 --force                 # skip the checks
```

