<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp tabs`

List the session's tabs; `*` marks the active one

```text
chrome-cdp tabs [OPTIONS]
```

Options:

- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Tabs have short ids (t1, t2, …) in the order they opened, never reused.
Commands act on the active tab unless --tab names another. A popup opens a
new tab but does not become active (unless `open --follow-popups`).

Examples:
  chrome-cdp tabs
  chrome-cdp tabs -q              # one id per line
```

