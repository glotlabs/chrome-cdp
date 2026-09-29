<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp context`

Make, list or close browser contexts: isolated cookies, storage, routes and mocks in one browser

```text
chrome-cdp context [OPTIONS] <COMMAND>
```

```text
A context is like an incognito window: its own cookies, storage and
cache, plus its own routes, mock sets, init scripts, emulation and
permissions, none of which it shares with the session's default context
or other contexts.
Contexts have ids (c1, c2, …) in the order they were made, never reused.
Any command takes --context <id> (or CHROME_CDP_CONTEXT) to act in one; a
page command then acts on the context's active tab. `chrome-cdp test`
runs each test in a fresh context.

Examples:
  chrome-cdp context new /login                 # c1, with a tab at base_url/login
  chrome-cdp --context c1 fill label=Email a@example.com
  chrome-cdp context new -q                     # prints just the id
  chrome-cdp context list
  chrome-cdp context close c1
  chrome-cdp context close --all
```

## `chrome-cdp context new`

Make a browser context with one tab, optionally loading a URL

```text
chrome-cdp context new [OPTIONS] [URL]
```

Arguments:

- `<URL>`: A URL to load; without a scheme, https:// (or http:// for localhost) is added

```text
Starts the session's browser first if it has none. The new context starts
with nothing: no cookies, storage, routes, mock sets, init scripts or
emulation.

Examples:
  chrome-cdp context new
  chrome-cdp context new https://example.com
  chrome-cdp context new /login -q      # prints just the id (with base_url)
```

## `chrome-cdp context list`

List the contexts, their tabs (`*` marks the active one) and settings

```text
chrome-cdp context list [OPTIONS]
```

Options:

- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Examples:
  chrome-cdp context list
  chrome-cdp context list -q     # one id per line
```

## `chrome-cdp context close`

Close contexts with their tabs, and drop their settings

```text
chrome-cdp context close [OPTIONS] [ID]...
```

Arguments:

- `<ID>`: The contexts to close, e.g. c1

Options:

- `--all`: Close every context of the session

```text
Without an id, closes --context's.

Examples:
  chrome-cdp context close c1
  chrome-cdp --context c2 context close
  chrome-cdp context close --all
```

