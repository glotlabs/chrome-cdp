# Debugging a local dev server

The most common job for a browser tool in a coding agent's hands: the
frontend on `localhost` does something wrong, and the agent must see it,
find out why, fix the code and check the fix. This guide walks through
that loop with a Vite, Next.js or similar dev server on port 3000.

## 1. Open the app

```sh
npm run dev &                         # or however the project starts
chrome-cdp open localhost:3000
```

`localhost` URLs get `http://`, so `localhost:3000/cart` is enough. If the
server is not up yet:

```
error: connection refused on localhost:3000
  hint: is your dev server running? start it, then: chrome-cdp goto http://localhost:3000/
```

Put `base_url = "http://localhost:3000"` in the project's
`.chrome-cdp.toml` and every command takes paths: `goto /cart`.

## 2. Reproduce the bug

Look at the page, then act on what you see:

```sh
chrome-cdp snapshot -i
chrome-cdp fill label=Quantity 3
chrome-cdp click 'role=button[name="Update cart"]'
```

Read the action's report before anything else. It already lists what the
click caused, including the errors:

```
clicked e14 button "Update cart"
! console error: TypeError: Cannot read properties of undefined (reading 'price')
! request failed: #42 POST /api/cart (500)
```

## 3. Find out why

**JavaScript errors.** `errors` lists the uncaught exceptions,
`console.error`s and failed loads the page has not shown yet. On
`localhost`, stack frames point at your source files through the dev
server's source maps:

```
#7 error uncaught TypeError: Cannot read properties of undefined (reading 'price')
    at lineTotal (/src/cart/total.ts:14:22)
    at /src/cart/Cart.tsx:31:9
```

`console` shows every message (`--level warn`, `--grep cart`, `--since
0` for all, `--follow` to stream), and works after the fact: the tab has
logged since it opened.

**Requests.** `network --failed` lists the requests that got no response
or a status of 400 or more; `network show 42` prints one in full:

```sh
chrome-cdp network --failed
chrome-cdp network show 42 --body --request-body
chrome-cdp network --url '**/api/**' --type fetch
```

A CORS failure says so (`CORS: MissingAllowOriginHeader`), which usually
means the API is on another port without the dev server's proxy.

**State.** `eval` runs code in the page like the DevTools console:

```sh
chrome-cdp eval 'window.__store.getState().cart'
chrome-cdp eval --in e14 'el.disabled'
chrome-cdp value label=Quantity
```

**Looks.** A screenshot with refs drawn on it shows layout bugs that a
snapshot cannot:

```sh
chrome-cdp screenshot --annotate          # prints the PNG's path
chrome-cdp screenshot e20                 # one element
chrome-cdp emulate 'iPhone 15' && chrome-cdp screenshot --full
```

**Speed.** `perf` sums up load timing, LCP, CLS, INP and long tasks;
`trace start` / `trace stop -o trace.json` records a trace for DevTools'
Performance panel.

## 4. Try states the backend will not give you

`route` changes what the page receives, without touching the server:

```sh
chrome-cdp route '**/api/cart' --patch-json '{"items": []}'    # an empty cart
chrome-cdp route /api/cart --method POST --status 500 --times 1  # fail once
chrome-cdp route '**/api/cart' --delay 3s                      # slow: see the spinner
chrome-cdp routes                                               # rules and their hits
chrome-cdp unroute --all
```

For a whole fake backend, the project's mock sets work in the session too
(`chrome-cdp mock use checkout`, see [e2e/mocks.md](e2e/mocks.md)).

## 5. Fix, reload, check

After editing the code, the dev server's hot reload usually updates the
page by itself; otherwise `chrome-cdp reload`. A full reload drops the
old refs, so take a new `snapshot -i` (or use locators, which do not go
stale). Then repeat the steps and check the result:

```sh
chrome-cdp reload
chrome-cdp fill label=Quantity 3
chrome-cdp click 'role=button[name="Update cart"]'
chrome-cdp expect 'role=status' text 'Cart updated'
chrome-cdp expect --no-errors
```

`expect --no-errors` fails if the page logged an uncaught exception or a
console error since it loaded (or since a `mark`), so a fix that moves
the crash elsewhere does not pass.

## 6. Keep the check

Once the flow works, save it so it stays fixed:

```sh
chrome-cdp history --as-scenario "updates the quantity" >> e2e/scenarios/cart.cdp
chrome-cdp test e2e/scenarios/cart.cdp
```

`history --as-scenario` turns the commands you just ran into a test, with
refs replaced by stable locators. [testing.md](testing.md) covers scripts
and tests; [e2e/README.md](e2e/README.md) the full E2E setup.

## When a human wants to watch

`open --headed` shows the browser window. `highlight e14` outlines an
element for two seconds, and `inspect` opens DevTools for the tab.
`open --restart --headed localhost:3000` starts the session's browser
again, with a window, and loads the app in it.
