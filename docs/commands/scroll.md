<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp scroll`

Scroll the page or an element with the mouse wheel

```text
chrome-cdp scroll [OPTIONS] [LOCATOR]
```

Arguments:

- `<LOCATOR>`: The element to scroll in (or into view); the page if omitted

Options:

- `--by <DX,DY>`: Wheel by this much, in CSS pixels (positive is right and down)
- `--to <WHERE>`: Wheel until this end: top, bottom, left, right, or a scroll offset x,y
- `--force`: Skip the checks on the element (visible, stable, not covered)
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
Real wheel events over the element (the middle of the viewport without
one), so scroll listeners and infinite scroll run. Prints where the
scroll container (the element or its nearest scrollable ancestor, else
the page) is now, and which end it reached. With a locator and neither
--by nor --to, scrolls the element into view. Needs a running session.

Examples:
  chrome-cdp scroll --by 0,600               # the page, one screen down
  chrome-cdp scroll --to bottom              # load more of a feed
  chrome-cdp scroll e30 --by 0,-200          # inside a list
  chrome-cdp scroll e30 --to top
  chrome-cdp scroll e99                      # into view
```

