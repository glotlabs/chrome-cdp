<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp tab`

Switch to a tab, or open or close one

```text
chrome-cdp tab [OPTIONS] [ID]
chrome-cdp tab <COMMAND>
```

Arguments:

- `<ID>`: The tab to make active, e.g. t2

```text
Examples:
  chrome-cdp tab t2                        # make t2 the active tab
  chrome-cdp tab new https://example.com   # open a tab and switch to it
  chrome-cdp tab close                     # close the active tab
  chrome-cdp tab close t3
```

## `chrome-cdp tab new`

Open a tab, make it active, and optionally load a URL in it

```text
chrome-cdp tab new [OPTIONS] [URL]
```

Arguments:

- `<URL>`: A URL to load; without a scheme, https:// (or http:// for localhost) is added

```text
Starts the session's browser first if it has none.

Examples:
  chrome-cdp tab new
  chrome-cdp tab new https://example.com
  chrome-cdp tab new localhost:3000/login -q     # prints just the new id
```

## `chrome-cdp tab close`

Close a tab (default: the active one)

```text
chrome-cdp tab close [OPTIONS] [ID]
```

Arguments:

- `<ID>`: The tab to close, e.g. t3. Default: --tab, else the active tab

Options:

- `--close-browser`: If it is the last tab, close the session and its browser too

```text
Closing the last tab leaves a blank one, so the session keeps its browser;
--close-browser closes the session instead.

Examples:
  chrome-cdp tab close
  chrome-cdp tab close t3
  chrome-cdp tab close --close-browser     # on the last tab: close the session
```

