<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp status`

Show the session's state, browser, tabs and uptime

```text
chrome-cdp status [OPTIONS]
```

Options:

- `--logs`: Also show the last lines of Chrome's log (its stderr)
- `--limit <N>`: How many log lines --logs shows (default `50`)

```text
Examples:
  chrome-cdp status
  chrome-cdp status -q            # just the state: running, crashed, idle or stopped
  chrome-cdp status --logs        # also the end of Chrome's log
```

