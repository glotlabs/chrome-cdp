<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp highlight`

Point out an element in the browser window, for a human watching

```text
chrome-cdp highlight [OPTIONS] <LOCATOR>
```

Arguments:

- `<LOCATOR>`: The element: a ref (e12) or a locator that matches one

Options:

- `--duration <DURATION>`: How long the highlight stays (0: until the next highlight) (default `2s`)
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
Scrolls the element into view and paints DevTools' element highlight over
it (content, padding, border and margin boxes, with a tooltip) for
--duration. Meant for a headed browser (open --headed); headless, only
screenshots show it. Needs a running session.

Examples:
  chrome-cdp highlight e12
  chrome-cdp highlight 'role=button[name="Save"]' --duration 5s
  chrome-cdp highlight '#cart' --duration 0      # until the next highlight
```

