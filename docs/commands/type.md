<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp type`

Type text key by key, as a user does (fill is faster)

```text
chrome-cdp type [OPTIONS] <TEXT>
```

Arguments:

- `<TEXT>`: The text; `-` reads it from stdin

Options:

- `--in <LOCATOR>`: Focus this element first: a ref (e12) or a locator
- `--delay <DURATION>`: Wait this long between keys (e.g. 50ms), for pages that react to each one
- `--force`: Skip the checks on the --in element (visible, stable, enabled, editable)
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
Presses a key for each character the US layout has (keydown, keypress,
input, keyup: every key listener runs) and inserts the rest (emoji, CJK)
as an input method does. Types where focus is; --in focuses an element
first (the caret goes after its text) once it takes text. Use fill to
replace a field's content in one go. Needs a running session.

Examples:
  chrome-cdp type 'hello world' --in e5
  chrome-cdp type 'rust' --in 'role=searchbox' --delay 50ms
  chrome-cdp type '👍 ok'
```

