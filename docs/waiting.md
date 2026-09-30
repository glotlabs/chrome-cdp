# Waiting

chrome-cdp waits for you, so **never `sleep`**. Navigation waits for the
page to load and settle, every action waits until its element can be
acted on and then for what the action caused, and `wait` covers what is
left: the page changing later on its own. `chrome-cdp help waiting` is the
short version.

## Navigation waits for the page to settle

`open <url>`, `goto`, `back`, `forward` and `reload` wait for the load
event, then for the page to settle: no requests for 500 ms and no DOM
changes for 300 ms, 5 s at most. A single-page app that fetches its data
after `load` is usually rendered by the time the command returns:

```sh
chrome-cdp goto localhost:3000/orders
# 200 http://localhost:3000/orders "Orders" (412ms)
```

`--wait` waits for less: `load`, `domcontentloaded`, `networkidle`,
`commit` (the response arrived) or `none`. Use it for pages that never go
quiet (polling, streaming, video) or when you only need the HTML.

## Actions wait before and after

Before it acts, every action (`click`, `fill`, `select`, `check`, `hover`,
`drag`, …) waits until its element is:

1. **attached**: a locator matches exactly one element;
2. **visible**: it has a size and is not `display: none`,
   `visibility: hidden` or fully clipped;
3. **stable**: it did not move between two animation frames;
4. **enabled**: not `disabled` or `aria-disabled` (and editable, for
   `fill`);
5. **not covered**: the element itself receives a click at its centre.

When a check does not pass within `--timeout`, the error says which and
why: `#off is disabled: it has the disabled attribute`, or what covers it
with a ref and the button that likely dismisses it. `--force` skips the
checks (the element is still scrolled into view).

After its input, the action waits for what it caused, and reports it. A
navigation is followed until the new page has loaded and settled;
otherwise the action waits briefly (at most 1.5 s) for the requests it sent
and for the DOM to go quiet:

```
clicked e6 button "Sign in"
→ navigated to https://acme.test/dashboard "Dashboard" (200)
! console error: TypeError: x is undefined (app.js:12)
! request failed: #42 POST /api/session (500)
```

So after `click` returns, the click's effects are in place, and the report
already says whether it worked. `--no-wait-after` returns as soon as the
input is sent.

## `wait` for the rest

When the page changes later, by itself (a toast after a background save, a
job that finishes, a WebSocket message), wait for the change itself:

| Command | Returns when |
|---|---|
| `wait <loc>` | the element is visible (`--state hidden\|attached\|detached\|enabled` for the others) |
| `wait --text 'Saved'` | the text appears anywhere, in any frame (`--in <loc>` for one part of the page) |
| `wait --text-gone 'Loading'` | the text is gone from every frame |
| `wait --url /dashboard` | the URL matches (a part, a glob, or a `/regex/`) |
| `wait --load` | the page reached its load event |
| `wait --idle` | no requests for 500 ms (not counting WebSockets, beacons and requests open over 10 s) |
| `wait --settle` | network idle and no DOM changes for 300 ms |
| `wait --response '**/api/save' --status 200` | a response since the previous command |
| `wait --download` | a download since the previous command finished |
| `wait --fn 'window.app?.ready'` | a JavaScript expression is truthy |

`wait --response` and `wait --download` also count what happened between
the previous command and this one, so `click e6 && wait --response …`
cannot miss a fast response.

```sh
chrome-cdp click e6 && chrome-cdp wait --text 'Order placed'
chrome-cdp wait .spinner --state hidden
chrome-cdp wait e12 --state detached
chrome-cdp wait --url /dashboard --timeout 20s
```

## Timeouts say what they saw

Waits end at `--timeout`: 10 s for actions and `wait`, 30 s for
navigation (`CHROME_CDP_TIMEOUT` or `timeout` in the config changes the
default). A timeout exits with code 3 and says what the page showed
instead, so the next step is clear without another command:

```
error: waited 500ms for #results to be visible
  observed: no element matches
  page: about:blank ""
  hint: chrome-cdp wait '#results' --timeout 1s   (no element matches the locator yet; check it)
```

It also reports the requests still in flight, the last value of a
`--fn`, and how many console errors are unread (`chrome-cdp errors`
shows them). Raise the timeout only when the page is slow, not when the
locator is wrong.

## Check, don't wait: `expect`

To verify a result rather than wait for it, use `expect`. It retries the
same way, 5 s by default, and fails with exit code 1 saying what it saw
last:

```sh
chrome-cdp expect 'role=alert' text Saved
chrome-cdp expect '.row' count 3
chrome-cdp expect --url /dashboard
chrome-cdp expect --no-errors
```

## Flaky pages

- **Animations.** An element that keeps moving never becomes stable;
  `media --reduced-motion reduce` makes most sites stop animating.
- **Pages that never go idle.** Analytics beacons and long-polling are
  already ignored by `--idle`; for anything else, wait for the element or
  text you need rather than for the network.
- **Time.** `time freeze` fixes `Date` for pages that show the time.
- **E2E tests.** The runner makes pages deterministic for you (frozen
  time, seeded random values, no animations); see
  [e2e/determinism.md](e2e/determinism.md).
