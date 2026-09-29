<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp forward`

Go forward one page in the tab's history

```text
chrome-cdp forward [OPTIONS]
```

Options:

- `--wait <EVENT>`: What to wait for before returning; --timeout bounds it (one of `settle`, `load`, `domcontentloaded`, `networkidle`, `commit`, `none`; default `settle`)

```text
At the end of the history it says so and exits 0.

Examples:
  chrome-cdp forward
  chrome-cdp forward --json
```

