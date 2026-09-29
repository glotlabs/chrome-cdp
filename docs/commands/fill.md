<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp fill`

Replace a field's content with text, in one input event

```text
chrome-cdp fill [OPTIONS] <LOCATOR> <TEXT>
```

Arguments:

- `<LOCATOR>`: The field: a ref (e12) or a locator that matches one
- `<TEXT>`: The text; `-` reads it from stdin; "" clears the field

Options:

- `--force`: Skip the checks (visible, stable, enabled, editable)
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
Waits until the input, textarea or contenteditable element is visible,
enabled and editable, focuses it, selects what it holds and inserts the
text as one trusted edit, so frameworks (React, Vue, Svelte) see it; the
page gets change when the field loses focus. Date, time, colour and range
inputs get the value set (2026-09-27, 13:45, #ff0000, 42). An empty text
clears the field. Needs a running session.

Examples:
  chrome-cdp fill e5 'ada@example.com'
  chrome-cdp fill 'label=Password' - < secret.txt
  chrome-cdp fill '#birthday' 1990-05-17
  chrome-cdp fill e5 ''                      # clear
```

