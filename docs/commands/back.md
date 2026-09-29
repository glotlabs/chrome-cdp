<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp back`

Go back one page in the tab's history

```text
chrome-cdp back [OPTIONS]
```

Options:

- `--wait <EVENT>`: What to wait for before returning; --timeout bounds it (one of `settle`, `load`, `domcontentloaded`, `networkidle`, `commit`, `none`; default `settle`)

```text
At the start of the history it says so and exits 0. Going back to a
pushState entry of a single-page app is a same-document navigation.

Examples:
  chrome-cdp back
  chrome-cdp back --tab t2 --wait commit
```

