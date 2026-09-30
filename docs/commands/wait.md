<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp wait`

Wait for the page: an element, text, the URL, network idle, a request, …

```text
chrome-cdp wait [OPTIONS] <LOCATOR|MS|--load|--domcontentloaded|--idle|--settle|--text <TEXT>|--text-gone <TEXT>|--url <PATTERN>|--fn <JS>|--request <PATTERN>|--response <PATTERN>|--download|--console <REGEX>|--binding <NAME>>
```

Arguments:

- `<LOCATOR|MS>`: An element to wait for (any locator: e12, #id, text=…, role=…), or how long to sleep: 500 (milliseconds) or 2s

Options:

- `--state <STATE>`: The state the element must reach (one of `visible`, `hidden`, `attached`, `detached`, `enabled`)
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)
- `--load`: The page's load event
- `--domcontentloaded`: DOMContentLoaded: the HTML is parsed
- `--idle`: Network idle: no requests for 500ms (not counting WebSockets, beacons and requests open over 10s)
- `--settle`: Network idle and no DOM changes for 300ms (3s at most for the DOM)
- `--text <TEXT>`: Visible text to wait for; `-` reads it from stdin
- `--text-gone <TEXT>`: Visible text to wait to go away; `-` reads it from stdin
- `--in <LOCATOR>`: Look for --text or --text-gone only in the first element this locator matches (or in the frame=… it names)
- `--url <PATTERN>`: The page URL: a part of it, a glob (\* and \*\*) or /regex/
- `--fn <JS>`: A JavaScript expression, polled in the page until it is truthy (promises are awaited); `-` reads it from stdin
- `--request <PATTERN>`: A request whose URL matches (a part, a glob or /regex/), sent since the previous command began
- `--response <PATTERN>`: A response whose URL matches, received since the previous command began
- `--status <CODE>`: The status the --response must have
- `--download`: A download, started since the previous command began, to finish
- `--console <REGEX>`: A console entry whose text or URL matches this regular expression, logged since the previous command began
- `--binding <NAME>`: A call of the function `expose NAME` put in the page, since the previous command began
- `--max-chars <N>`: Bound the output (--fn's value) to this many characters (0 = no limit) (default `20000`)

```text
Give exactly one condition. It is checked against the active tab (or
--tab) until it holds or --timeout (default 10s) passes; a timeout
(exit 3) says what the page showed instead. Needs a running session.

  <LOCATOR>           an element: a ref (e12), #id, css=…, text=…,
                      role=button[name="…"], label=…, frame=… >> …;
                      --state visible (default), hidden, attached,
                      detached or enabled
  --text, --text-gone visible text anywhere, iframes included (or only
                      --in <LOCATOR>); spaces are collapsed, case counts
  --url               the page URL: a part of it, a glob (* and **) or
                      /regex/
  --load, --domcontentloaded, --idle (no requests for 500ms, not counting
                      WebSockets, beacons and requests open over 10s),
  --settle            (network idle and no DOM changes for 300ms)
  --fn                a JavaScript expression that becomes truthy
  --request, --response [--status]  since the previous command began
  --download          a download started since the previous command began
                      has finished (prints its path with -q)
  --console <REGEX>   a console entry (any level or source) whose text or
                      URL matches, logged since the previous command
                      began; prints it as `console` does
  --binding <NAME>    a call of a function `expose NAME` put in the page,
                      since the previous command began
  <MS>                a fixed sleep, e.g. 500 or 2s (prefer a condition)

Examples:
  chrome-cdp wait '#results'
  chrome-cdp wait 'role=button[name="Save"]'
  chrome-cdp wait e12 --state detached
  chrome-cdp wait .spinner --state hidden
  chrome-cdp wait --text 'Welcome back'
  chrome-cdp wait --text-gone Loading --in '#main'
  chrome-cdp wait --url /dashboard
  chrome-cdp wait --idle
  chrome-cdp wait --fn 'window.app && window.app.ready'
  chrome-cdp wait --response '**/api/save' --status 200
  chrome-cdp wait --download
  chrome-cdp wait --console 'checkout (done|failed)'
  chrome-cdp wait --binding onCheckout      # after expose onCheckout
```

