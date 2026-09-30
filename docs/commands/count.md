<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp count`

Count the elements a locator matches (0 is not an error)

```text
chrome-cdp count [OPTIONS] <LOCATOR>
```

Arguments:

- `<LOCATOR>`: What to count: any locator

Options:

- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
Prints the count, and how many are visible when that differs:
`4 (3 visible)`. -q prints only the count. It does not wait: 0 is an
answer (`wait LOCATOR` or `expect LOCATOR count` wait). Needs a running
session.

Examples:
  chrome-cdp count '.cart-item'
  chrome-cdp count 'role=row'
  chrome-cdp count 'text=Out of stock' -q
```

