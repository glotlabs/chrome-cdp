<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp locale`

Set the locale: Intl formats, navigator.language, Accept-Language

```text
chrome-cdp locale [OPTIONS] <NAME>
```

Arguments:

- `<NAME>`: The value, or `reset` for the browser's own

```text
Frames and workers see it too; a worker that kept another is a warning.

Examples:
  chrome-cdp locale de-DE
  chrome-cdp locale ja-JP
  chrome-cdp locale reset
```

