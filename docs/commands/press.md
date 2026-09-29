<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp press`

Press keys or shortcuts: Enter, Control+A, "Mod+A Backspace"

```text
chrome-cdp press [OPTIONS] <KEYS>
```

Arguments:

- `<KEYS>`: Keys or chords, space-separated: Enter, Control+A, "Tab Tab Enter"

Options:

- `--in <LOCATOR>`: Focus this element first: a ref (e12) or a locator
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
Keys go where focus is, as a user's do; --in focuses an element first.
Name keys like Enter, Tab, Escape, ArrowDown, PageUp, F1, Numpad0, a or A,
and hold them together with + (Control+Shift+Z). Mod is Meta when the
browser runs on macOS and Control elsewhere, so Mod+A selects all on both.
Several chords, separated by spaces, are pressed in turn. Needs a running
session.

Examples:
  chrome-cdp press Enter
  chrome-cdp press Tab --in e5
  chrome-cdp press 'Mod+A Backspace' --in 'label=Search'
  chrome-cdp press Shift+Tab
```

