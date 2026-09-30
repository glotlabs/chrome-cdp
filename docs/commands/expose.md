<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp expose`

Give pages a function whose calls show up in the console

```text
chrome-cdp expose [OPTIONS] [NAME]...
```

Arguments:

- `<NAME>`: Global function names (letters, digits, _ and $)

Options:

- `--returns <JSON>`: Each call's promise resolves to this JSON value
- `--returns-seq <JSON-ARRAY>`: The calls get these values in call order, the last one from then on (a JSON array)

```text
Each NAME becomes a global function in every page and frame of the session
(the open ones at once, new ones before their first script). It takes any
arguments and returns a promise; each call is logged in the tab's console
as `binding NAME(args)`, the arguments as JSON, so `console` shows what
the page reported. Without NAME, lists the exposed functions. `unexpose`
takes them away.

The promise resolves to undefined, or with --returns to that JSON value.
--returns-seq gives the calls its values in call order (counted across
the session's tabs), the last one for every later call. The answer is
data, not code: for logic in the page, use eval or init-script. Exposing
a NAME again with other answers starts its sequence over.

Examples:
  chrome-cdp expose onCheckout
  chrome-cdp console --grep 'binding onCheckout'
  chrome-cdp expose getConfig --returns '{"flags":["beta"]}'
  chrome-cdp expose nextToken --returns-seq '["a","b",null]'
  chrome-cdp wait --binding onCheckout
  chrome-cdp expose
```

