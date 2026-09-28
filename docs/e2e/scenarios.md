# Scenario files and `chrome-cdp test`

A scenario is a `.cdp` file ([syntax](../scenario-syntax.md)) whose steps
are chrome-cdp command lines, grouped into tests by directives.
`chrome-cdp test` runs them, each test in a fresh browser context with
its mocks. This page is the reference; plan 11 has the design.

```
# e2e/scenarios/checkout.cdp
@mocks base checkout             # layered onto default_mocks
@tags smoke checkout

@before
  state load ../state/user.json
  goto /

@test "lists products"
  expect 'role=listitem' count 3
  expect text=Mechanical keyboard visible

@test "retries when the API is busy"
  @retries 1
  click 'role=button[name="Buy"]'
  expect role=alert text "Please try again"

@test "shows an error page when the API is down"
  @mocks errors/api-down         # extra layer for this test only
  reload
  expect 'role=heading' text "Something went wrong"

@test "empty cart" @skip "flaky upstream component, see #12"
  click text=Cart
```

## Blocks

| Directive | Holds |
|---|---|
| `@test "name" [settings…]` | a test: its steps, and settings for it alone |
| `@before` | steps before each test of the file, in the test's context |
| `@after` | steps after each test, in its context, also when it failed |
| `@before-all` | steps once before the file's first test, in a context of their own |
| `@after-all` | steps once after the file's last test, in a context of their own |

A block's lines are indented 2 spaces. A command outside a block is an
error. A test with no steps is an error unless it is skipped. A failed
`@before-all` fails every test of the file; a failed `@after` or
`@after-all` fails the test it ran after. Contexts are fresh, so a
`@before-all` hands state to the tests through files:
`state save auth.json` there, `state load auth.json` in `@before`.

## Settings

Written at the top of the file they apply to every test; in a test's
body, or after its name on the `@test` line, to that test only.

| Setting | Meaning |
|---|---|
| `@mocks set…` | mock sets layered onto `default_mocks`; the file's, then the test's |
| `@tags tag…` | tags for `--tags`; the file's and the test's add up |
| `@skip [reason]` | skipped, and reported with the reason |
| `@only` | when any selected test has it, only those run |
| `@timeout 60s` | the test's time limit (default: `timeout` in `[e2e]`, 30s) |
| `@viewport 390x844` | the viewport (default: `viewport` in `[e2e]`, 1280x800) |
| `@device "iPhone 15"` | a device of `chrome-cdp emulate list` |
| `@retries N` | runs of a failed test before it counts as failed |
| `@allow-console-errors` | console errors do not fail the test (see [Checks](#checks-the-runner-adds)) |
| `@backend mock\|real\|NAME…` | the backends the test runs on; others skip it, with the reason ([backends](backends.md)). A test's replaces its file's |
| `@safe` | the test only reads, so a `protected` backend may run it |

## Requests the app sent

`expect request <METHOD> <PATH>` checks the requests the test's pages
sent, mocked or not; `expect no-request <METHOD> <PATH>` checks none was:

```
@test "retries when the API is busy"
  mark before-buy
  click 'role=button[name="Buy"]'
  expect request POST /api/orders --json-body '{"qty":1}' --count 2 --since before-buy
  expect no-request DELETE '/api/cart/**'
```

- `PATH` is a path glob (`*` within a segment, `**` across them, `:id`
  for a segment), `**/orders` for any prefix, a full-URL glob, or `*`.
  `METHOD` may be `*`.
- `--query k=v`, `--header 'Name: value'` (values are globs) and
  `--json-body` (a subset of the JSON body, as a mock's `match.json`)
  narrow it down.
- `--count N`, `--min N`, `--max N` say how many; the default is at least
  one. It waits (up to `--timeout`) for enough requests; too many fail at
  once. `--since <mark>` counts only those after a `mark`.
- A failure lists the matching requests and the ones that came closest,
  with the condition each fails:

  ```
  error: expect failed: request POST /api/orders --json-body '{"qty":3}'
    expected: a request POST /api/orders with json {"qty":3}
    actual: 0 requests (after 5s, 199 retries)
    similar: t1 #7 POST http://localhost:5173/api/orders → 201: json $.qty: expects 3, got 2
    hint: chrome-cdp network show 7   (shows the closest request in full, body and headers)
  ```

## Checks the runner adds

- **Unmatched requests fail the test.** After each step, the runner reads
  the test's mock log. A request no mock answered, in the scope of a set
  with `unmatched = "fail"` (the default), fails the test at that step,
  with the mocks that come closest to it:

  ```
  failed at e2e/scenarios/cart.cdp:9:3: click text=Cart
  error: unmatched request: GET http://localhost:5173/api/cart?user=7
    scope: mock set base, whose unmatched = "fail" answered it 501
    closest mock: base.toml:31 "cart": path differs: expects /api/carts, got /api/cart
    hint: add a mock for it, or run chrome-cdp mock explain GET 'http://localhost:5173/api/cart?user=7' --mocks base
  ```

  When the step failed on its own (the app showed an error instead of
  the cart), the unmatched requests are listed under its error.
  WebSockets in a set's scope that no `[[websocket]]` mock takes, and a
  `mock_server`'s requests of the test, count too
  ([beyond fetches](beyond-fetch.md)).
- **Console errors fail the test.** After a test's steps pass, the runner
  checks `expect --no-errors --all-tabs`: no uncaught exception or
  `console.error` in any of its tabs. `@allow-console-errors` on a test
  (or a file) or `allow_console_errors = true` in `[e2e]` turns this off.

## Reuse

`@include ../steps/login.cdp email=admin@example.com` puts the lines of a
step file in place of the `@include` line. Paths are relative to the file
that includes. A step file holds commands and `@include` lines only; its
lines see the parameters as `${email}`, and `${data:…}` paths are
relative to it. `${env:X}` reads an environment variable.

Against more than one backend, `${var:name}`, `${secret:NAME}`,
`${test.id}` and `http … --save NAME` with `${saved.NAME.field}` keep the
same scenario working everywhere; see [backends](backends.md).

## Running

```
chrome-cdp test [PATH…] [--grep RE] [--tags smoke,!slow] [--jobs N]
                [--jobs-isolation context|browser] [--shard 2/4]
                [--retries N | --repeat N] [--bail] [--headed]
                [--update-snapshots] [--list] [--json]
```

- Without paths, the `tests` globs of `[e2e]` find the files (default
  `e2e/scenarios/**/*.cdp`). A path is a file, a folder, or `file.cdp:12`
  for the test at line 12.
- The order is fixed: files by path, tests as written. `--shard 2/4` runs
  the second of four near-equal parts of that list.
- Every file is read and every step checked against the commands before
  anything starts; a problem fails the run with its `file:line:col`
  (exit code 2).
- The `[e2e.web_server]`s start first, unless something answers on their
  URLs already. A web server with `static = "dist"` instead of `command`
  is a built app that chrome-cdp serves itself (client-side routes get
  `index.html`), so the run needs no Node. The run opens a browser session of its own and closes it
  at the end.
- Each test gets a new browser context: its own cookies, storage, cache,
  routes, mock sets (and their state), init scripts and emulation. Mock
  state starts over in every test. `--jobs N` runs N tests at once in one
  Chrome; `--jobs-isolation browser` gives each job its own Chrome.
- Every test sees the same time (frozen at 2026-01-01T10:00:00Z), the
  same random values, `en-US`, `UTC`, a pixel ratio of 1 and no
  animations; `time advance 5m` moves the time on and runs the timers due
  by then. `--repeat 20` runs each test 20 times to find flaky ones. See
  [determinism.md](determinism.md).
- A step runs as `chrome-cdp <line> --session <run's> --context <test's>`.
  Its timeout is cut to what is left of the test's time limit.
- `expect-snapshot` and `expect-screenshot` baselines are in
  `e2e/snapshots` of the project; `--update-snapshots` rewrites them.
- A test that fails, then passes on a retry, is **flaky**: it counts as
  passing and is always reported. `--bail` stops after the first failed
  test; the rest are "not run".
- Exit code 0: every test passed (or was flaky or skipped); 1: a test
  failed; 2: a scenario or argument is wrong; 4: a web server or the
  browser failed.

The output is a line per test as it ends:

```
running 4 tests from 2 files, 2 at once
✓ e2e/scenarios/checkout.cdp:8 lists products (1.1s)
✗ e2e/scenarios/checkout.cdp:12 retries when the API is busy (1.6s)
    failed at e2e/scenarios/checkout.cdp:15:3: expect role=alert text "Please try again"
    error: expect failed: role=alert
      …
    report: /home/me/shop/e2e/.results/checkout.cdp/retries-when-the-api-is-busy/report.md
~ e2e/scenarios/cart.cdp:6 adds an item (2.3s; flaky: passed on try 2)
- e2e/scenarios/checkout.cdp:24 empty cart (skipped: flaky upstream component, see #12)
4 tests: 1 passed, 1 failed, 1 flaky, 1 skipped (4.2s)
failure reports:
  /home/me/shop/e2e/.results/checkout.cdp/retries-when-the-api-is-busy/report.md
  /home/me/shop/e2e/.results/cart.cdp/adds-an-item/report.md
error: 1 of 4 tests failed
  failed: e2e/scenarios/checkout.cdp:12 retries when the API is busy
  hint: chrome-cdp test e2e/scenarios/checkout.cdp:12   (runs the first failed test again)
```

`--json` prints one envelope with every test's status, tries, duration
and failure (the step's `file:line:col`, its text, and its error);
`chrome-cdp schema test` has its schema.

Every test leaves `steps.jsonl`, `requests.txt` and `console.txt` in
`e2e/.results/<file>/<test>/`, and a failed one a `report.md` (its path is
printed under the test's line) with the step, the error, the page around
it, console errors and failed requests. `--debug` keeps the first failed
test's page open, `--watch` reruns tests as files change, and `--reporter
junit,html` writes files for CI and humans: see
[debugging.md](debugging.md).

## Browser contexts outside tests

Contexts are a general feature: `chrome-cdp context new [url]` makes one
(`c1`, `c2`, …), `--context c1` on any command acts in it (its active tab,
its cookies, routes and mocks), `context list` shows them and
`context close c1` closes one. Two contexts are two users logged in side
by side in one browser.
