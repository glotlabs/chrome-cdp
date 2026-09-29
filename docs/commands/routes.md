<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp routes`

List the routing rules and their hits, or the requests they saw

```text
chrome-cdp routes [OPTIONS]
```

Options:

- `--log`: List the requests routing saw instead of the rules
- `--limit <N>`: With --log: show at most this many requests, the newest (0 = all) (default `20`)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Each line reads: id, what it matches, what it does, and how many requests
it handled. --log lists the requests routing saw instead (while routes
exist, every request is seen), newest last, with the rule that handled
each.

Examples:
  chrome-cdp routes
  chrome-cdp routes --log
  chrome-cdp routes --log --limit 100
```

