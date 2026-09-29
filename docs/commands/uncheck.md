<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp uncheck`

Uncheck a checkbox or switch (no-op if it is not checked)

```text
chrome-cdp uncheck [OPTIONS] <LOCATOR>
```

Arguments:

- `<LOCATOR>`: The checkbox, radio button, switch, or its label: a ref (e12) or a locator that matches one

Options:

- `--force`: Skip the checks before the click (visible, stable, enabled, not covered)
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
Like check, the other way. A radio button cannot be unchecked: check
another one of its group. Needs a running session.

Examples:
  chrome-cdp uncheck e9
  chrome-cdp uncheck 'label=Send me offers'
```

