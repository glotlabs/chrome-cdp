<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp unroute`

Remove routing rules

```text
chrome-cdp unroute [OPTIONS] <ID|--all>
```

Arguments:

- `<ID>`: The rules' ids, as `routes` lists them (r1, r2, …)

Options:

- `--all`: Remove every rule

```text
Without routes, requests are no longer intercepted at all.

Examples:
  chrome-cdp unroute r2
  chrome-cdp unroute r1 r3
  chrome-cdp unroute --all
```

