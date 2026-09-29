<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp storage`

Read or change a page's localStorage or sessionStorage

```text
chrome-cdp storage [OPTIONS] <AREA> [ACTION] [KEY] [VALUE]
```

Arguments:

- `<AREA>`: local (localStorage) or session (sessionStorage) (one of `local`, `session`)
- `<ACTION>`: list, get, set, remove or clear (one of `list`, `get`, `set`, `remove`, `clear`; default `list`)
- `<KEY>`: The key (get, set, remove)
- `<VALUE>`: The value (set); `-` reads stdin

Options:

- `--origin <ORIGIN>`: Use the tab's first frame of this origin (e.g. an iframe's)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
AREA is local (localStorage) or session (sessionStorage); ACTION is list
(the default), get KEY, set KEY VALUE (VALUE `-` reads stdin), remove KEY
or clear. It works in the tab's main frame, or with --origin in its first
frame of that origin (an iframe's). sessionStorage belongs to the tab.

Examples:
  chrome-cdp storage local
  chrome-cdp storage local get token
  chrome-cdp storage local set feature-x on
  chrome-cdp storage session clear
  chrome-cdp storage local --origin https://auth.example.com
```

