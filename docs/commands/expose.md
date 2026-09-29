<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp expose`

Give pages a function whose calls show up in the console

```text
chrome-cdp expose [OPTIONS] [NAME]...
```

Arguments:

- `<NAME>`: Global function names (letters, digits, _ and $)

```text
Each NAME becomes a global function in every page and frame of the session
(the open ones at once, new ones before their first script). It takes any
arguments and returns a promise; each call is logged in the tab's console
as `binding NAME(args)`, the arguments as JSON, so `console` shows what
the page reported. Without NAME, lists the exposed functions. `unexpose`
takes them away.

Examples:
  chrome-cdp expose onCheckout
  chrome-cdp console --grep 'binding onCheckout'
  chrome-cdp expose
```

