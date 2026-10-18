# Running scenarios against a real backend

The same scenario files run against the mocks and against a real backend
(a local API, a docker-compose stack, staging). Mock mode is the fast
default for every change; a real backend proves the frontend and the API
work together, and `--verify-mocks` shows where the mocks no longer match
the API.

```
chrome-cdp test                          # the mocks (backend mock)
chrome-cdp test --backend local          # the real API of [e2e.backends.local]
chrome-cdp test --backend all            # every backend in turn
chrome-cdp test --backend staging --verify-mocks
```

`--backend` also comes from `CHROME_CDP_BACKEND`, else `default_backend`
in `[e2e]`, else `mock`.

## Backends

`mock` is built in. A real backend is a table in `chrome-cdp.toml`:

```toml
[e2e]
vars = "e2e/vars/mock.toml"            # ${var:…} of every backend

[e2e.backends.local]
mode = "real"
base_url = "http://localhost:8080"
web_server = { command = "docker compose up api web", url = "http://localhost:8080", timeout = "120s" }
setup = ["./scripts/e2e-reset-db.sh"]  # before the first test; teardown = [...] after the last
vars = "e2e/vars/local.toml"           # on top of [e2e]'s vars
mocks = ["third-party/stripe"]         # hybrid mode, see below
jobs = 2
retries = 1                            # the default in real mode
determinism = { freeze_time = false }  # the default in real mode
protected = false
```

A backend's `base_url`, `web_server`, `jobs`, `timeout`, `retries` and
`determinism` replace `[e2e]`'s. `setup` and `teardown` commands run with
`sh -c` in the project root, once the web servers answer; they see
`CHROME_CDP_BACKEND` and `CHROME_CDP_BASE_URL`, and log to
`e2e/.results/backend-setup.log`. A failed setup command stops the run; a
failed teardown command is a warning. Before each command runs, stderr
shows it and the config it came from: `running: ./scripts/e2e-reset-db.sh
(from /home/me/app/chrome-cdp.toml)`, as for a web server's command.

`--backend all` runs `mock` and then every real backend, each with its
own servers and setup. Real backends' artifacts go below
`e2e/.results/<backend>/`; the result lists every test with its backend
(`--json`: `backends` and each test's `backend`).

## What changes in real mode

- **Mocks are off.** The app's requests go to the server. They are still
  in each test's network log, so `expect request` and `requests.txt` (a
  failed test's, or every test's with `keep_logs = true`) work as before.
- **Hybrid mode.** A backend's `mocks` stay on, for example a third-party
  payment, email or maps API, while the app's own API is real. Tests'
  `@mocks` and `default_mocks` are not applied; a request outside those
  sets' scopes goes to the network. For the app's server-side fetches,
  give the backend's web server a mock server with an upstream:
  `web_server = { …, mock_server = { port = 4010, upstream =
  "http://localhost:8080" } }`, and point the app at port 4010. It
  answers with the backend's `mocks` and forwards their `passthrough`
  answers (and requests in their scope no mock takes, with `unmatched =
  "passthrough"`) to the real API, each within `upstream_timeout`
  (default `"30s"`; [beyond-fetch.md](beyond-fetch.md)).
- **Determinism is best-effort.** Time is not frozen (tokens and server
  time checks need the real time) unless the backend's `determinism` says
  so; random values stay seeded and animations off. Failed tests are
  retried once by default, and a test that passes on a retry is reported
  as flaky.
- `--update-snapshots` is refused: baselines come from mock runs.

## Tests for one mode

`@backend mock` marks a test (or a file) as mock-only, e.g. "the API
answers 503 once"; `@backend real` the opposite; `@backend local
staging` names backends. A test that does not apply is **skipped with the
reason**, never counted as passed:

```
- e2e/scenarios/cart.cdp:30 retries when busy (skipped: @backend mock; this run's backend is local)
```

The commands that only work with mocks (`mock state`, `mock seed`,
`expect mock-state`) fail in a test that runs on a real backend, with the
hint to add `@backend mock`.

## Test data

```
@before
  http POST /api/login --json-body "{\"email\":\"${var:admin_email}\",\"password\":\"${secret:ADMIN_PASSWORD}\"}"
  http POST /api/orders --json-body "{\"sku\":\"${var:sku}\"}" --expect-status 201 --save order
  goto /orders/${saved.order.id}

@after
  http DELETE /api/orders/${saved.order.id} --expect-status 2xx,404

@test "shows the order"
  expect "text=Order ${saved.order.id}" visible
  fill label=Note "gift for e2e+${test.id}@example.com"
```

- `${var:name}` comes from the vars files: `[e2e] vars` (the defaults,
  and the mock backend's), then the backend's `vars`. A vars file is flat
  TOML: `name = "value"` (numbers and booleans too). Write
  `expect text=${var:product_name} visible` rather than data only the mocks
  have.
- `${secret:NAME}` is the environment variable `NAME`. Its value is masked
  as `<secret:NAME>` in stdout, stderr, `steps.jsonl`, `report.md`, every
  other artifact and the reporters' files. Password fields show dots in
  screenshots; an input or text area whose whole value is a secret is
  painted over in `failure.png` and the `--trace` screenshots, and
  `report.md` says so. A secret that is only part of a field's value, or
  in an iframe, still shows.
- `${test.id}` is unique per test try (`${run.id}-<n>`, then `-<try>`),
  so parallel tests and repeated runs against a shared server do not
  collide. `${backend}` is the backend's name.

### `http`: arrange state through the API

`http <METHOD> <url> [--json-body '…'] [--body TEXT] [--header 'k: v']
[--expect-status 2xx] [--save NAME]` sends a request from the test's
browser context, outside the page:

- it carries the context's cookies (`HttpOnly` ones too), and the cookies
  the answer sets go into the context, so `http POST /api/login` logs the
  pages in;
- in mock mode the mocks answer it, collections included, so arrange
  steps work the same with every backend;
- it fails unless the status is `--expect-status` (default `2xx`);
- `--save NAME` keeps the JSON answer for `${saved.NAME.field}` in the
  test's later steps (a `@before-all`'s for every test of its file); a
  number reaches into a list: `${saved.order.items.0.sku}`;
- the page never sees it, so `expect request` does not count it.

Arrange in `@before` rather than `@before-all` when the tests also run
against the mocks: every test gets fresh mocks, so what a `@before-all`
created through the mocks is gone in the tests.

## Keeping mocks honest: `--verify-mocks`

```
chrome-cdp test --backend staging --verify-mocks
```

After each test, every real answer to a request in a mock set's scope
(the page's requests and the `http` steps) is compared with the answer
the test's mock sets would have given, fed the same requests in the same
order, so sequences and collections behave as they would have. It
compares the status and the **shape** of JSON bodies: keys, types, the
shape of list items, and null against present; never values.

```
mocks against backend staging: 42 real answers compared with 9 mocks
  drift: base.toml:6 "list products" (products.json), GET https://staging.example.com/api/products
    $[]: the real answer has field "price_cents" (number); the mock has "price" (number)
  no mock: GET https://staging.example.com/api/stats (real answer 200; in the scope of mock set base)
```

Drift or a request without a mock fails the run (exit 1), with the
details in `--json` (`verify`). `--update` rewrites the drifted mocks'
JSON data files from the real answers: fields the API added come in with
the real values, fields it dropped go, and values whose type still fits
stay, in the file's key order, so the change reads as a normal diff.
Inline `json` bodies, `.tmpl.json` templates, collections' seeds and a
different status are left for you.

A nightly CI job running `--backend staging --verify-mocks` is the
recommended setup; the workflow `chrome-cdp init e2e` writes has one,
commented out.

## Recording mocks from a real run: `--record-mocks`

```
chrome-cdp test --backend local --record-mocks
```

Each test that passes has its requests (in the scope of the mock sets it
has in mock mode; without any, its fetch and XHR requests) written as a
mock set of its own, `recorded/<file>/<test>` in the mocks directory, as
`mock record` writes one ([authoring](authoring.md)); an older recording
is replaced. A set per test, because every test starts from fresh mocks:
a list read before and after another test's write cannot be one set's
answer. A test replays its recording with `@mocks recorded/<file>/<test>`;
move what the tests share into the project's own sets.

With `--match-bodies`, requests to one path are told apart by their
bodies, as `mock record --match-bodies` does: a test that logs in as two
users gets a mock per user, not a `sequence`.

## Protected backends

`protected = true` (for anything production-like) makes the run refuse
to start unless every test that would run is marked `@safe` (read-only),
or `--allow-writes` is given. In a `@safe` test on a protected backend,
an `http` POST, PUT, PATCH or DELETE fails in the test's own steps;
arrange and clean-up steps in `@before`, `@after` and the `-all` hooks
may write.
