<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp ua`

Set the user agent pages and servers see

```text
chrome-cdp ua [OPTIONS] <USER-AGENT>
```

Arguments:

- `<USER-AGENT>`: The user agent (`-` reads stdin), or `reset`

```text
`emulate <device>` sets a device's user agent with its viewport.

Examples:
  chrome-cdp ua 'Mozilla/5.0 (compatible; MyBot/1.0)'
  chrome-cdp ua reset
```

