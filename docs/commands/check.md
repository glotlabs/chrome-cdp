<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp check`

Check a checkbox, radio button or switch (no-op if it is already)

```text
chrome-cdp check [OPTIONS] <LOCATOR>
```

Arguments:

- `<LOCATOR>`: The checkbox, radio button, switch, or its label: a ref (e12) or a locator that matches one

Options:

- `--force`: Skip the checks before the click (visible, stable, enabled, not covered)
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
Clicks it only when it is unchecked, then makes sure it became checked.
Custom checkboxes (role=checkbox, switch) are clicked like native ones; a
native input styled away behind its label is checked by clicking the
label. A label locator checks the control it labels. Needs a running
session.

Examples:
  chrome-cdp check e9
  chrome-cdp check 'label=Remember me'
  chrome-cdp check 'role=radio[name="Express"]'
```

