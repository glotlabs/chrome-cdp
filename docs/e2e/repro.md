# Repro bundles

A bug found in an interactive session is hard to hand over: the steps
live in a shell history, the data in a backend that will have changed by
tomorrow. `repro save` packs what the session did into a folder that
replays it with no server, and `chrome-cdp test` runs that folder.

```
chrome-cdp open http://localhost:5173/shop
chrome-cdp repro start
chrome-cdp click 'role=button[name="Add"]'
chrome-cdp click 'role=button[name="Buy"]'
chrome-cdp expect role=status text 'Order o-17: shipped'      # fails: the bug
chrome-cdp repro save bugs/order-status --name "a paid order shows as shipped"
```

```
/home/me/shop/bugs/order-status: repro "a paid order shows as shipped" with 3 steps, ending with the failed `expect role=status text 'Order o-17: shipped'` (ExpectFailed); 4 mocks from 4 requests
next: chrome-cdp test /home/me/shop/bugs/order-status
```

```
chrome-cdp test bugs/order-status
```

The bundle's test fails the way the session did, at the same step with
the same error. Once the bug is fixed (or the bundle's mock gives the
fixed answer), it passes: then it is a regression test, ready to copy
into the suite.

## Start and save

`repro start` marks the start. It keeps the session's cookies and storage
(what `state save` writes), the time, the page's viewport, locale and
time zone, and where the history is. From then on, each request's
response body is kept as it finishes loading: Chrome drops a page's
bodies once the tab navigates away, so without this the documents and
scripts of earlier pages would be lost. `--scope` keeps only some bodies
(path globs such as `/api/**`, or full-URL globs, as a mock set's
`scope`). The page open at the start is where the bundle's test starts.
`repro start` again starts over.

`repro save DIR` writes the bundle. DIR must be new or empty; `--force`
replaces the bundle's own files in a folder that has others. Without a
`repro start`, it uses the whole history, an empty state and only the
bodies Chrome still has, and warns.

## The bundle

```
bugs/order-status/
  chrome-cdp.toml        # [e2e]: tests = ["repro.cdp"], mocks = "mocks", default_mocks = ["repro"],
                         #   the session's viewport, base_url, locale and time zone, time frozen at the start
  repro.cdp              # @test NAME: the commands since repro start
  mocks/repro.toml       # every request recorded; one no mock answers fails
  mocks/data/…           # response bodies
  state.json             # the cookies and storage at repro start
  evidence/
    snapshot.txt         # the page as the session left it
    screenshot.png
    console.txt, errors.txt, requests.txt
  README.md              # what happened, the last error, left-out bodies, secrets, versions
```

`repro.cdp` is what `history --as-scenario` writes (refs become stable
locators, read-only commands are left out), with these changes:

- `@before` loads `state.json`.
- The test starts with `goto` the page open at `repro start`, unless its
  first step is a `goto`.
- The last command that failed (a page that misbehaved, not a typo: a
  usage error does not count) stays, as the last step, with its error in
  a comment. Commands after it are left out.
- `mock` commands are left out: the bundle's own mocks answer every
  request.

```
@before
  state load state.json

@test "a paid order shows as shipped"
  goto http://localhost:5173/shop
  click 'role=button[name="Add"]'
  click 'role=button[name="Buy"]'
  # failed in the session (ExpectFailed: expect failed: role=status text 'Order o-17: shipped'); passes once the bug is fixed
  expect role=status text 'Order o-17: shipped'
```

`mocks/repro.toml` is `mock record`'s conversion of every request since
the start (documents, scripts, styles, images and API calls), with
`scope = ["/**"]` and `unmatched = "fail"`: a request no mock answers gets
a 501 and fails the test, so the replay never reaches the network. Edit
it like any mock set; `chrome-cdp mock check` lints it.

## Size

Response bodies over 5MB, and images and fonts over 1MB, are left out:
their mock answers with the status and no body, a comment says why, and
the README lists them. `--max-body SIZE` changes the limit (images and
fonts get a fifth of it). Bodies Chrome no longer had (requests before
`repro start`, or without one) are listed the same way.

## Secrets

- A value typed into a password field (`fill`, `type`) is never written:
  the step reads `${secret:PASSWORD}` instead, and the README says the
  replay needs `PASSWORD` in the environment.
- `${secret:…}` placeholders in the history stay placeholders, and the
  README names each.
- `Authorization` and `Cookie` headers are not in the mocks (they match
  by method, path and query), and volatile response headers are left
  out, as `mock record` does. `state.json` holds the session's cookies at
  the start: it is written readable by you only; treat it like any
  `state save` file.

## Replay

`chrome-cdp test DIR`, for a folder with a `chrome-cdp.toml` of its own,
runs that project as if `test` were started in the folder: its tests,
mocks and state, with relative paths (`state load state.json`) read from
there. The results go to `DIR/e2e/.results/`. The project has no web
server: every request, the first document included, comes from the
mocks.

## Limits

- The tabs and requests are the context's of the session (`--context`);
  a WebSocket's frames and a worker's own requests are not replayed.
- Files a step reads (`upload FILE`, `state load FILE` after the start)
  are not copied into the bundle.
- A password typed into an iframe's field is not recognized (the check
  reads the page's focused element).
- Mocks match by path, so two origins' requests to the same path share a
  mock.
