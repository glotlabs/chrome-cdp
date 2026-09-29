<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp focus`

Give an element focus (without clicking it)

```text
chrome-cdp focus [OPTIONS] <LOCATOR>
```

Arguments:

- `<LOCATOR>`: The element: a ref (e12) or a locator that matches one

Options:

- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
Waits until the element is visible, then focuses it, so focus styles,
focus listeners and later keys apply to it. Fails if it cannot take
focus. Needs a running session.

Examples:
  chrome-cdp focus e5
  chrome-cdp focus 'label=Email'
```

