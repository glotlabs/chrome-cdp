<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp attr`

Print an attribute of an element

```text
chrome-cdp attr [OPTIONS] <LOCATOR> <NAME>
```

Arguments:

- `<LOCATOR>`: The element: a ref (e12) or a locator that matches one
- `<NAME>`: The attribute, e.g. href

Options:

- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Prints nothing (and a warning naming the attributes it has) when the
element has no such attribute; --json has "value": null. For a
field's current value use `value`. An element not there yet is waited
for (up to --timeout, 10s by default). Needs a running session.

Examples:
  chrome-cdp attr e12 href
  chrome-cdp attr '#avatar' src
  chrome-cdp attr 'role=button[name="Save"]' aria-disabled
```

