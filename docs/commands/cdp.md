<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp cdp`

Make one raw CDP call on the session's browser or current page

```text
chrome-cdp cdp [OPTIONS] [Domain.method] [JSON]
```

Arguments:

- `<Domain.method>`: The CDP method, e.g. Browser.getVersion; optional with --listen
- `<JSON>`: Params as a JSON object; `-` reads them from stdin

Options:

- `--target <TARGET>`: Send the call to the browser, or to the active tab (or --tab). Default: page with --tab or --frame, else browser (one of `browser`, `page`)
- `--frame <FRAME>`: Send the call to a frame of the tab: its name attribute, a part of its URL, or its id. Implies --target page. A cross-site frame has its own CDP session; Runtime.evaluate runs in the frame's page world unless the params name a context
- `--listen <EVENT>`: Also print these events of the call's session (Domain.event or Domain.\*; repeat it or separate them with commas)
- `--count <N>`: How many events to wait for (0: all of them until --timeout)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit). With --json, long strings and arrays in `data` are shortened instead (default `20000`)

```text
Starts the session's browser first if it has none. --tab and --frame send
the call to a page. --listen also prints the events of the call's session
that follow it, one per line, until --count of them came (1 by default; 0
keeps them all until --timeout); with no method, it only listens. The page
domains chrome-cdp uses (Page, Runtime, Network, Log, …) are on; send
another's enable call to hear its events.
Methods: https://chromedevtools.github.io/devtools-protocol/

Examples:
  chrome-cdp cdp Browser.getVersion
  chrome-cdp cdp --target page Runtime.evaluate '{"expression":"1+1"}'
  echo '{"expression":"1+1"}' | chrome-cdp cdp --target page Runtime.evaluate -
  chrome-cdp cdp Target.getTargets --session shop
  chrome-cdp cdp --tab t2 Page.reload
  chrome-cdp cdp --frame checkout.example Runtime.evaluate '{"expression":"location.href"}'
  chrome-cdp cdp --target page --listen Page.loadEventFired Page.reload
  chrome-cdp cdp --target page --listen 'Network.*' --count 0 --timeout 5s
```

