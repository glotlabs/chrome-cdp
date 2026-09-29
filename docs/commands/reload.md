<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp reload`

Load the current page again

```text
chrome-cdp reload [OPTIONS]
```

Options:

- `--hard`: Bypass the cache
- `--wait <EVENT>`: What to wait for before returning; --timeout bounds it (one of `settle`, `load`, `domcontentloaded`, `networkidle`, `commit`, `none`; default `settle`)

```text
Examples:
  chrome-cdp reload
  chrome-cdp reload --hard          # bypass the cache
  chrome-cdp reload --wait domcontentloaded
```

