<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp tz`

Set the time zone pages see (an IANA name)

```text
chrome-cdp tz [OPTIONS] <NAME>
```

Arguments:

- `<NAME>`: The value, or `reset` for the browser's own

```text
Frames and workers see it too; a worker that kept another is a warning.

Examples:
  chrome-cdp tz Europe/Berlin
  chrome-cdp tz America/New_York
  chrome-cdp tz reset
```

