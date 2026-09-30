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

On `localhost`, chrome-cdp reads the dev server's source maps and the
framework's dev build, so the page can tell you which file to open.
Places print as paths relative to the project root (the directory with
`.git` or `chrome-cdp.toml`) when the file is on disk, so an editor takes
them as they are. Ask the page before grepping the code.

**Which file renders it.** `source` names the line that made an element
and the components around it (React 18 and 19, Vue 3, Svelte 5, and
plain JavaScript through Chrome's record of the script that created the
node):

```
$ chrome-cdp source e3
e3 button "Add"
  at src/components/CartItem.tsx:15:7
  in <CartItem>  src/components/CartItem.tsx:15
  in <CartList>  src/components/CartList.tsx:14
  in <App>       src/App.tsx:34
via react 19 (the page's debug info)
```

`source -q e3` prints the place alone. `snapshot -i --source` adds the
innermost component to every ref. `components e3` shows the component's
props and state; `components` alone the whole tree:

```
$ chrome-cdp components e3
<CartItem> src/components/CartItem.tsx:12  (holds e3)
  props: item={id: 2, name: "Tea", price: 5, qty: 1}, onAdd=ƒ, onRemove=ƒ
  state: none
  owners: <CartList> ← <App>
```

**JavaScript errors.** The action's report names the source line of a
console error. `errors` lists the uncaught exceptions, `console.error`s
and failed loads the page has not shown yet, and `--locals` adds the
variables of the top frames where the error was thrown:

```
$ chrome-cdp errors --locals
#4 error uncaught TypeError: Cannot read properties of undefined (reading 'price')
    at lineTotal (src/cart.ts:21:15)
      item = undefined, qty = 1
    at total (src/cart.ts:28:12)
      item = undefined, line = {id: 99, qty: 1}, items = [{id: 1, name: "Mug", …}, …], sum = 41
```

Locals are read when the debugger stops at the throw. A framework that
catches a handler's error first (React 19 does) leaves them out and
says so, with the `debug log` command that gets the values instead.
`console` shows every message (`--level warn`, `--grep cart`, `--since
0` for all, `--follow` to stream), and works after the fact: the tab has
logged since it opened.

**Values while it runs.** A logpoint is `console.log` without editing
the code: it logs its expressions each time a source line runs, and the
page never stops. The next action's report shows the hits:

```
$ chrome-cdp debug log src/cart.ts:28 sum line.qty
logpoint 1 at src/cart.ts:28 (bound in /src/cart.ts)
$ chrome-cdp click e3
clicked e3 button "Add"
! logpoint 1 src/cart.ts:28: sum=0, line.qty=2
! logpoint 1 src/cart.ts:28: sum=24, line.qty=2
! logpoint 1 src/cart.ts:28: sum=34, line.qty=4
$ chrome-cdp debug clear --all
```

A line with no code moves to the next one and says so; a logpoint
survives reloads and binds to modules loaded later. `debug list` shows
them and their hits.

**Requests.** `network --failed` lists the requests that got no response
or a status of 400 or more; `network show 42` prints one in full, with
the stack that sent it, across `await`s and timers:

```
$ chrome-cdp network show 17
#17 GET 200 OK http://localhost:3000/cart.json
…
initiator: script
  fetchCart                               src/api.ts:13:26
  (anonymous)                             src/App.tsx:11:5
```

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

After you save a file, the dev server's hot reload (Vite, webpack,
Next.js) updates the page. `wait --hmr` tells you whether it did:

```
$ chrome-cdp wait --hmr
hot update applied (src/components/CartItem.tsx) (201ms)
```

A compile error fails it (exit 1) with the error's place, so you know the
page still runs the old code:

```
$ chrome-cdp wait --hmr
error: hot update failed: [PARSE_ERROR] Unexpected token (src/components/CartItem.tsx:23)
  client: vite
  at: src/components/CartItem.tsx:23:8
  hint: chrome-cdp wait --hmr   (after you fix it and save, waits for the next update)
```

It answers with the latest update since the previous command began, so
it may run after the save. Any other command also notes an update that
came since the last one (`warning: hot update applied (…)`), and an
action's report lists one that came while it ran. A full reload (`!
full reload`) drops the refs: take a new `snapshot -i`, or use locators,
which do not go stale. Without hot reload, `chrome-cdp reload`.

Then repeat the steps and check the result:

```sh
chrome-cdp fill label=Quantity 3
chrome-cdp click 'role=button[name="Update cart"]'
chrome-cdp expect 'role=status' text 'Cart updated'
chrome-cdp expect --no-errors
```

`expect --no-errors` fails if the page logged an uncaught exception or a
console error since it loaded (or since a `mark`), so a fix that moves
the crash elsewhere does not pass. Remove your logpoints with `debug
clear --all` when done.

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
