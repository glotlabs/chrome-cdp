<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp blur`

Take focus away from an element (blur and change fire)

```text
chrome-cdp blur [OPTIONS] <LOCATOR>
```

Arguments:

- `<LOCATOR>`: The element: a ref (e12) or a locator that matches one

Options:

- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
If the element (or something in it) has focus, it loses it: the page gets
blur, and change for a field whose value was edited. Needs a running
session.

Examples:
  chrome-cdp blur e5
```

