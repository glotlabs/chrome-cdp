# Plan 11 — End-to-end testing with a mocked backend

Status: IN PROGRESS (11.1-11.7 done)
Depends on: 07 §7.1 (routing engine), 08 §8.2 (`.cdp` script syntax), 09 §9.1–9.2 (`expect`)
Supersedes: 09 §9.3 (test runner). That milestone is now specified here.

## Goal

**Best-in-class E2E testing of web frontends against a mocked backend.** A
project using chrome-cdp commits two kinds of plain files:

- **scenarios**: what a user does and what should happen;
- **mocks**: how the backend behaves, plus the data it returns.

`chrome-cdp test` runs them: it starts the app if needed, runs every scenario
in an isolated browser context with its mocks, and reports failures in a form
an agent can fix without re-running anything.

The target user is an agent (or a human) that just changed frontend code and
wants a fast, deterministic answer to "does it still work?". The run needs no
real backend, no database, no network, and no flakiness.

**The same scenarios also run against a real backend** (a local API, a
docker-compose stack, or staging) with `chrome-cdp test --backend <name>`
(§11.11). Mock mode is the fast default for every change. Real mode proves the
frontend and backend actually work together, and `--verify-mocks` uses real
runs to show where the mocks no longer match the real API.

### Why this beats Playwright

| | Playwright | chrome-cdp |
|---|---|---|
| Scenarios | TypeScript test code | `.cdp` files in the **same syntax the agent already uses interactively**. An exploratory session can be saved as a test (§11.8). |
| Mocks | `page.route` callbacks written in code, per test | Declarative mock files + data files, reusable, layered by name, and loadable into an interactive session |
| Stateful backend | Build it yourself | Built-in fake REST collections and response sequences (§11.3) |
| Missing mocks | Request goes to the network or hangs | Unmatched requests in scope fail the test, with a closest-match diagnosis (§11.5) |
| Mock drift | Not addressed | `mock check` validates mocks against an OpenAPI spec (§11.8) |
| Creating mocks | HAR replay (opaque blob) | `mock record` writes readable, editable mock files and data files |
| Server-side fetches (SSR) | Not interceptable | `mock serve` runs the same mock files as a real HTTP server (§11.9) |
| Mocked vs real backend | Separate test setups, or conditionals in code | Same scenario files; `--backend mock\|<real>`; hybrid mode mocks only chosen services; `--verify-mocks` compares mocks with real responses (§11.11) |
| Failures | Trace viewer (for a human with a GUI) | `report.md` per failed test: failing step, snapshot, screenshot path, console errors, request journal. Written for an agent to read. |

## Project layout (convention; every path is configurable)

```
chrome-cdp.toml                  # project config (the [e2e] section)
e2e/
  scenarios/
    checkout.cdp
    products/listing.cdp
  steps/                         # reusable step files (@include)
    login.cdp
  mocks/
    base.toml                    # mock set "base"
    checkout.toml                # mock set "checkout"
    errors/api-down.toml         # mock set "errors/api-down"
    data/
      products.json
      products/1.json
      todos.json
  state/
    user.json                    # storageState (plan 07 §7.4)
  snapshots/                     # expect-snapshot baselines (plan 09 §9.2)
  .results/                      # artifacts (git-ignored; `init e2e` adds it to .gitignore)
```

## Milestone 11.1 — Project config, `init`, web server

`chrome-cdp.toml` is found by walking up from the current directory (same
lookup as plan 08 §4):

```toml
[e2e]
base_url = "http://localhost:5173"
tests = ["e2e/scenarios/**/*.cdp"]
mocks = "e2e/mocks"
default_mocks = ["base"]         # applied to every test unless it says otherwise
jobs = 4                         # "auto" = number of CPUs / 2
retries = 0
timeout = "30s"                  # per test
artifacts = "e2e/.results"
viewport = "1280x800"

[e2e.web_server]
command = "npm run dev -- --port 5173"
url = "http://localhost:5173"    # ready when this returns < 500
reuse_existing = true            # if something already answers on url, use it and don't start or stop anything
timeout = "60s"
env = { VITE_API_URL = "/api" }

[e2e.determinism]                # see §11.6
freeze_time = "2026-01-01T10:00:00Z"
random_seed = 42
disable_animations = true

# Backends (§11.11). "mock" is built in and the default; every other entry is a real backend.
[e2e.backends.local]
mode = "real"
web_server = { command = "docker compose up api web", url = "http://localhost:8080", timeout = "120s" }
base_url = "http://localhost:8080"
setup = ["./scripts/e2e-reset-db.sh"]         # run before the suite; teardown = [...] after
vars = "e2e/vars/local.toml"

[e2e.backends.staging]
mode = "real"
base_url = "https://staging.example.com"
vars = "e2e/vars/staging.toml"
jobs = 2
determinism = { freeze_time = false }          # real servers issue real-time tokens
```

- `web_server` starts the command in its own process group and streams its
  output to `.results/web-server.log`. It waits for `url`, then tears the whole
  group down at the end, including on Ctrl-C or failure. If the server exits
  early, fail immediately with the last 30 log lines. More than one
  `[[e2e.web_server]]` block is allowed (e.g. frontend + a static asset
  server).
- `chrome-cdp init e2e` scaffolds the layout above:
  - it detects the framework from `package.json` (Vite, Next, CRA, Astro,
    SvelteKit) and fills in `web_server` and `base_url`;
  - it writes one example scenario, one mock set, and a GitHub Actions
    workflow (`.github/workflows/e2e.yml`);
  - it never overwrites existing files.

## Milestone 11.2 — Mock files & engine

Mock files are TOML; one file is one named **mock set**. The format is also
exported as JSON Schema (`chrome-cdp schema mocks`) so editors and agents can
validate it.

```toml
# e2e/mocks/base.toml
[settings]
scope = ["/api/**"]              # requests the mocks own (globs on path, or full-URL globs)
unmatched = "fail"               # fail | passthrough | abort — for in-scope requests with no mock
cors = "auto"                    # add CORS headers + answer preflights for cross-origin API hosts
latency = "0ms"                  # default added delay; "50ms..200ms" for a random range (seeded)

[[mock]]
name = "list products"
match = { method = "GET", path = "/api/products" }
respond = { json_file = "data/products.json" }

[[mock]]
name = "product by id"
match = { method = "GET", path = "/api/products/:id" }
respond = { json_file = "data/products/{{params.id}}.json" }   # a missing file becomes 404 automatically

[[mock]]
name = "search"
match = { method = "GET", path = "/api/search", query = { q = "*" } }
respond = { json = { results = [], query = "{{query.q}}" } }

[[mock]]
name = "login"
match = { method = "POST", path = "/api/login", json = { email = "*@example.com" } }
respond = { status = 200, headers = { "set-cookie" = "sid=test; Path=/" }, json = { ok = true } }
```

Matching:

- `method`; `path` with `:param` and `*`/`**` globs, or `url` for full URLs,
  or `regex`; `query` (subset; values can be globs); `headers` (subset).
- The body can be matched as:
  - `json` (a deep **subset** match; `"*"` wildcards; arrays must match
    exactly unless `json_mode = "contains"`);
  - `form`;
  - `body` (text glob or regex);
  - `graphql = { operation = "GetCart", variables = { … } }` (parses the
    GraphQL POST body).
- Precedence: inline `route` commands in the test (plan 07) come first. Then
  mock sets, in reverse layering order (later sets override earlier ones).
  Within a file, the first match wins. `chrome-cdp mock explain <METHOD> <url>`
  prints which mock would answer and why the others didn't match.

Responses:

- `status`, `headers`, and exactly one of `json`, `json_file`, `body`,
  `body_file`, `file` (content type from the extension). Also `delay`,
  `abort = "connectionreset"` (and other network errors), and
  `passthrough = true`, which sends the request to the real server but can
  still `patch` the response (deep-merge into the JSON).
- Templating in string values and file paths (`minijinja`):
  - `{{params.x}}`, `{{query.x}}`, `{{headers.x}}`, `{{json.path.to.field}}`
    (request body);
  - `{{uuid}}`, `{{now}}` (frozen time aware), `{{seq}}` (per-mock counter),
    `{{faker.name}}` (seeded; small built-in set).

  A template error is a mock error, reported as such, and never a silent empty
  string.
- Data files are served as-is. Only files ending in `.tmpl.json` go through
  templating, so large fixtures aren't slowed down.

Engine:

- Built on the plan 07 `Fetch` routing engine, running in the daemon.
  Requests never reach a network socket, so no ports are needed, it works for
  cross-origin API hosts, and it works offline.
- It applies per browser context, so parallel tests have independent mocks
  and state. Service worker bypass is automatic (plan 07).
- Mock files are loaded and validated once per run, with errors reported with
  file:line. Files are re-read on change in `--watch` and interactive modes.

Interactive use (the same engine, outside `test`):

- `mock use <set...>` applies mock sets to the current session. An agent can
  then develop or debug the UI against mocks by hand.
- Also: `mock list`, `mock off`, `mock log [--unmatched]` (a request journal,
  showing which mock answered each request), and `mock reset` (reset state,
  see §11.3).

## Milestone 11.3 — Stateful mocks

A realistic backend has state. Two built-in mechanisms, both reset for each
test:

**Sequences:** a mock answers differently on each call.

```toml
[[mock]]
name = "orders: busy once, then ok"
match = { method = "POST", path = "/api/orders" }
sequence = [
  { status = 503, json = { error = "busy" } },
  { status = 201, json = { id = "{{uuid}}", qty = "{{json.qty}}" } },
]
after_sequence = "repeat-last"   # repeat-last | cycle | unmatched
```

**Collections:** a fake REST resource backed by an in-memory store, seeded
from a data file.

```toml
[[collection]]
name = "todos"
path = "/api/todos"              # GET list, GET /:id, POST, PUT/PATCH /:id, DELETE /:id
id = "id"                        # id field; POST generates one (uuid, or increment if the seed ids are numbers)
seed = "data/todos.json"
list = { filter_query = true, page_param = "page", page_size_param = "limit", wrap = "items" }  # optional
```

- A POST appears in the next GET, so create-then-list flows work without
  writing any code. Validation is minimal by design; for anything special, add
  a `[[mock]]` in front of the collection.
- Scenarios can inspect and change state:
  - `mock state todos` prints the current contents;
  - `expect mock-state todos count 4`;
  - `mock seed todos data/other.json`.
- `[state]` holds simple named values, which a mock can change (`set_state =
  { logged_in = true }`) and require (`when = { logged_in = true }`). This
  covers login/logout flows without code.

## Milestone 11.4 — Scenario files & the runner

Scenarios are plan 08 `.cdp` scripts plus test directives:

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
  click 'role=button[name="Buy"]'
  expect role=alert text "Please try again"
  click text=Retry
  expect --url **/orders/*
  expect request POST /api/orders --count 2

@test "shows an error page when the API is down"
  @mocks errors/api-down         # extra layer for this test only
  reload
  expect 'role=heading' text "Something went wrong"

@test "empty cart" @skip "flaky upstream component, see #12"
  ...
```

- Directives: `@test`, `@before`, `@after`, `@before-all`, `@after-all`,
  `@mocks`, `@tags`, `@skip [reason]`, `@only`, `@timeout 60s`,
  `@viewport 390x844` / `@device "iPhone 15"`, `@retries N`.
- Reuse:
  - `@include ../steps/login.cdp email=admin@example.com` inlines a step file,
    with parameters available as `${email}`. Paths are relative to the
    including file.
  - `${data:state/user.json#/email}` reads a value from a data file.
  - `${env:X}` reads an environment variable.
- Isolation: each test gets a **fresh browser context** in a shared Chrome
  (about 50 ms; the browser is not relaunched). Mock state, cookies, storage
  and routes are all per context, so tests can't leak into each other.
  Browser contexts are also a general interactive feature:
  `context new|list|close` and `--context <id>` on any command, which allows
  multi-user flows such as two logged-in users in one browser.
- `chrome-cdp test [paths…] [--backend mock|<name>] [--grep re] [--tags smoke,!slow] [--jobs N]
  [--shard 2/4] [--retries N] [--headed] [--watch] [--debug] [--update-snapshots]
  [--reporter text,json,junit,html] [--bail] [--list]`.
  - Paths may be files, folders or `file.cdp:12` (the test at that line).
  - `--list` prints the tests without running them.
  - Exit code 0 means everything passed, and 1 means a failure (plan 08 §3).
- Test order is deterministic. Parallelism distributes tests across N browser
  contexts: one daemon, one Chrome, `jobs` concurrent contexts. With
  `--jobs-isolation browser`, it uses N Chromes instead.
- Retries: a test that fails and then passes is reported as **flaky** and
  still shown in the summary. Flakiness is never hidden.

## Milestone 11.5 — Request assertions & unmatched-request diagnostics

- `expect request <METHOD> <path-glob> [--json '<subset>'] [--query k=v]
  [--header k:v] [--count N | --min N | --max N] [--since mark]`. It checks
  requests the app **sent** (from the per-test journal). On failure, it
  lists the requests that were actually sent to similar paths.
- `expect no-request <METHOD> <path-glob>`.
- **Unmatched requests fail the test** when `unmatched = "fail"` (the
  default for in-scope paths). The report shows the closest mocks by method,
  path and body similarity, and which condition didn't match:

  ```
  unmatched request: GET /api/cart?user=7
    closest mock: "cart" (base.toml:31) expects path /api/carts — path differs
    hint: add a mock, or run `chrome-cdp mock explain GET /api/cart?user=7`
  ```
- `expect no-errors` (plan 09) runs **automatically** at the end of every test
  unless `allow_console_errors = true` is set in config or on the test. Real
  apps under mocks often throw on unexpected data shapes, and this catches it.

## Milestone 11.6 — Determinism

Flaky E2E suites are the main reason teams give up on them. Every test gets
these by default (configurable in `[e2e.determinism]`):

- **Frozen time:** `Date`, `performance.now` offset, and timers
  (plan 07 §7.2 `time freeze`). `time advance 5m` is available in
  scenarios for testing expiry, polling and debounce.
- **Seeded `Math.random`** and `crypto.randomUUID`, through an init script.
- **Animations off:** injected CSS (`*, *::before, *::after { transition:
  none !important; animation: none !important; caret-color: transparent }`) plus
  `prefers-reduced-motion: reduce`.
- **Fixed** viewport, locale (`en-US`), timezone (`UTC`), and DPR 1.
- **Consistent fonts** for screenshot assertions: a documented Docker image,
  and a warning when baselines were made on a different OS.
- Mock latency is either zero or seeded random, so reruns match.
- `chrome-cdp test --repeat 20 <file>` is a built-in flakiness check before
  committing.

## Milestone 11.7 — Reports & debugging

- For each test, write to `.results/<file>/<test-slug>/`:
  - `steps.jsonl` (each step: command, duration, result);
  - `requests.txt` (the journal: every request and the mock that answered it);
  - `console.txt`.

  For a failed test, also write:
  - `failure.png`;
  - `snapshot.txt` (`snapshot -i` at the moment of failure);
  - `report.md`.
- `report.md` is **the agent-facing failure summary**. It is small and
  self-contained: the failing step and its line number, the error with its
  hint, the last 3 steps before it, the snapshot excerpt around the target,
  console errors, unmatched and failed requests, and the paths of the other
  artifacts. At the end of a run, the text reporter prints the list of
  `report.md` paths.
- Reporters:
  - `text` (the default; one line per test, and failure summaries inline up to
    a size limit);
  - `json`;
  - `junit`;
  - `html`: one self-contained file with a timeline of steps, a screenshot per
    step if `--trace` is on, and the request journal. This is the human
    equivalent of the Playwright trace viewer.
- `--debug`:
  - on the first failure, stop and **leave the browser context alive** as
    session `test-debug`, with the mocks still active;
  - print `chrome-cdp --session test-debug snapshot` as the next command.

  The agent can then inspect and fix the problem with the normal interactive
  commands, and rerun the one test by `file.cdp:line`.
- `--watch` reruns the affected tests when a scenario, mock or data file
  changes, or when the web server's output files change.

## Milestone 11.8 — Authoring tools

- **Session → scenario:** the daemon keeps a history of the commands a session
  ran. `chrome-cdp history --as-scenario "adds item to cart" >>
  e2e/scenarios/cart.cdp` writes it as a test:
  - refs (`e12`) are rewritten to stable locators (role+name, then label, then
    testid, then text; always the most robust unique one);
  - read-only commands (`snapshot`, `console`) are dropped;
  - the `expect` commands the agent ran are kept.

  Exploring interactively, then saving the session as a test, is the core
  agent workflow.
- **Record mocks from a real backend:**
  - `chrome-cdp mock record --scope '/api/**' -o e2e/mocks/recorded.toml`
    (in a live session, or `test --record-mocks` for a whole scenario);
  - it writes one `[[mock]]` per distinct request, with bodies in
    `data/recorded/…json`;
  - volatile headers (`date`, `etag`, `set-cookie` unless needed) are removed.
    Detected ids and timestamps get a comment, so a human or agent can
    turn them into templates.
- `chrome-cdp mock import <file.har>` does the same conversion from a HAR file.
- `chrome-cdp mock check`:
  - lints all mock files: schema, missing data files, unreachable mocks
    (covered by an earlier match), invalid templates;
  - with `openapi = "api/openapi.yaml"` in config, it validates every mock's
    path, method, status and response body against the spec, which catches
    mock drift when the real API changes. It runs automatically at the start
    of `test` and fails fast.

## Milestone 11.9 — Beyond browser fetches

- **`chrome-cdp mock serve [sets…] --port 4010`:** serves the same mock files
  as a real HTTP server (`axum`). This is needed for SSR frameworks (Next.js,
  Remix, SvelteKit load functions), whose server-side fetches never pass
  through the browser.
  - In `[e2e.web_server]`, set `mock_server = { port = 4010 }`, then point the
    app at it with `env = { API_URL = "http://localhost:4010" }`.
  - State is shared across tests unless the app forwards a per-test header.
    `chrome-cdp` adds `x-chrome-cdp-test: <id>` to browser requests, and the
    mock server partitions state by it when present. Document the limitation.
- **WebSocket mocks:**

  ```toml
  [[websocket]]
  url = "/ws/**"
  on_open = [{ send = { type = "hello" } }]
  on_message = [{ match = { type = "ping" }, send = { type = "pong" } }]
  ```

  They are implemented with an init script that replaces `WebSocket` in the
  page. Fetch can't intercept WebSockets. Scenarios can push messages with
  `mock ws send /ws/** '{"type":"price","value":42}'`.
- **Server-Sent Events:** `respond = { sse = [ { data = {…} }, { delay = "1s",
  data = {…} } ] }`. This is implemented with an init-script `EventSource`
  shim, because Fetch fulfillment can't stream.

## Milestone 11.12 — Scenario tooling: format, lint, editors

The `.cdp` grammar and parser come from plan 08 §8.2. This milestone adds the
tools a real format needs.

- **`chrome-cdp fmt [paths…] [--check]`** formats `.cdp` files and mock TOML
  files in place. `--check` exits 1 and prints a diff instead, for CI.
  - `.cdp`: normalise the indentation, one blank line between tests,
    consistent quoting (single quotes unless the string needs `${…}` or
    contains `'`), long flags instead of short, and a stable order of
    directives.
  - Mock TOML: key order within `[[mock]]` (name, match, respond/sequence),
    and inline tables kept inline (`toml_edit`, so comments survive).
  - It is idempotent (a property test: `fmt(fmt(x)) == fmt(x)`) and preserves
    comments. Formatting only changes layout, never meaning: the AST before
    and after is identical, and a test checks this on every fixture.
- **`chrome-cdp check [paths…]`** lints scenarios, and also runs the
  `mock check` rules from §11.8, so one command validates the whole `e2e/`
  folder. All results have `file:line:col`, a severity, a rule id, and a
  "did you mean". Scenario rules:
  - syntax errors;
  - unknown commands, flags, directives and devices;
  - invalid locators;
  - `@include` targets that are missing or cyclic, and missing include
    parameters;
  - undefined variables, per backend (a `${var:x}` missing from any
    backend's vars file);
  - refs (`e12`) in committed scenarios, an error because refs are
    session-only (the hint is the stable locator `history --as-scenario`
    would have written);
  - mock-only commands outside `@backend mock` (§11.11);
  - `wait <ms>` sleeps (a warning);
  - a test with no `expect` (a warning);
  - duplicate test names in a file.

  It takes `--format text|json|github`; `github` prints workflow annotations,
  so errors appear inline on PRs. `chrome-cdp test` runs `check` first and
  refuses to start on errors.
- **Machine-readable spec:** `chrome-cdp schema scenario` prints the list of
  directives, the commands with their flags, and the variable forms as JSON,
  generated from the clap definitions. Editors and agents use it, and it can't
  drift from the binary.
- **Editor support (stretch; can ship after v1):**
  - a `tree-sitter-cdp` grammar (its own repo, tested against the
    same fixtures as the Rust parser);
  - a VS Code extension with highlighting, format on save
    (calls `chrome-cdp fmt`), and diagnostics (calls `chrome-cdp check --format
    json`);
  - `chrome-cdp lsp` later, if the extension proves popular.

  The mock TOML files are covered by any TOML editor plus the published JSON
  Schema (a `#:schema` comment written by `init e2e` enables Taplo/Even Better
  TOML validation).

## Milestone 11.10 — Example project, CI & docs

- `examples/e2e-shop/`: a small frontend-only SPA (Vite + React, prebuilt
  `dist/` committed so CI needs no Node) with a complete `e2e/` folder. It
  covers:
  - product listing;
  - a product page (`:id` data files);
  - login (state);
  - cart (collection);
  - checkout with a retry (sequence);
  - an API-down error page (a layered mock set);
  - an SSR-free search (query matching);
  - live prices (WebSocket);
  - one intentionally unmatched request case in a test fixture, to check the
    diagnostics.

  Also add a small Next.js SSR example for `mock serve`, built in CI only.
  This example is the **acceptance suite** for this plan, and plan 10 runs it
  on every CI runner.
- The GitHub Actions workflow template created by `init e2e`:
  - installs chrome-cdp;
  - caches the browser;
  - runs `chrome-cdp test --reporter text,junit --shard ${{ matrix.shard }}`;
  - uploads `.results/` on failure.
- Docs:
  - `docs/e2e/` with a quickstart, the mock file reference (generated from the
    schema), scenario reference, determinism, and debugging failures;
  - a recipes page (auth, pagination, file upload, optimistic UI, polling,
    feature flags, error states);
  - "Migrating Playwright tests + `page.route` mocks".
- `chrome-cdp skill` (plan 08) gets an E2E section. It teaches agents the
  workflow: explore → `history --as-scenario` → add mocks/data → `test` →
  read `report.md` → fix.

## Milestone 11.11 — Running scenarios against a real backend

The same scenario files run in every mode. Only the backend changes.

**Backends.** A backend is `mock` (built in) or a named `[e2e.backends.<name>]`
entry with `mode = "real"`. The entry can override `base_url`, `web_server`,
`jobs`, `timeout`, `determinism` and `vars`, and can set `setup`/`teardown`
commands (reset a database, run migrations, seed data).

- Select one with `chrome-cdp test --backend local`, or
  `CHROME_CDP_BACKEND`, or `default_backend` in config.
- `--backend all` runs the suite once per backend, and the report groups
  results by backend.

**Mock behaviour in real mode:**

- Mock sets are **off**. In-scope requests pass through to the real server
  and still go into the request journal, so `expect request` and the
  `requests.txt` artifact keep working.
- **Hybrid mode:** a backend can keep chosen mock sets on, for example to
  mock a third-party payment provider, email or maps API while the app's own
  API is real:

  ```toml
  [e2e.backends.local]
  mocks = ["third-party/stripe"]   # applied in real mode; everything else passes through
  ```

  In hybrid mode, `unmatched` is forced to `passthrough` outside those sets'
  scopes.

**Scenarios that only make sense in one mode:**

- `@backend mock` marks a test as mock-only (e.g. "API returns 503 once" or
  "WebSocket pushes a price"); `@backend real` marks the opposite. Both work on
  a whole file or a single test. Tests that don't apply to the selected backend
  are **skipped with a reason**, so they're never silently counted as passed.
- Mock-only commands (`mock state`, `mock seed`, `mock ws send`,
  `expect mock-state`) inside a test that is not marked `@backend mock` fail
  in real mode with a hint to add the marker. `mock check` also warns about
  them statically.

**Test data that differs between backends:**

- `${var:name}` reads from the backend's `vars` file, for example
  `e2e/vars/mock.toml` (defaults) and `e2e/vars/staging.toml`. Scenarios say
  `expect text=${var:product_name} visible` instead of hard-coding
  mock-specific data. `mock check` flags literals that appear in mock data
  files but not in any vars file, as a hint that the test may only pass with
  mocks.
- Secrets come from `${env:…}` or `${secret:…}`. Secrets are masked in all
  output, reports, `steps.jsonl` and screenshots of password fields.
- Unique data for each run: `${run.id}` and `${test.id}`. For example,
  `fill label=Email e2e+${test.id}@example.com`, so parallel tests and
  repeated runs against a shared real backend don't collide.

**Arranging state on a real backend:**

- `http <METHOD> <url> [--json '…'] [--header k:v] [--expect-status 2xx]
  [--save name]` is a new command that sends a request from the test's browser
  context, using its cookies and auth, but outside the page.
  - Use it in `@before` to create the data a test needs through the real API,
    and in `@after` to clean up.
  - `--save order` keeps the response as `${saved.order.id}`.
  - In mock mode, the same `http` calls are answered by the mocks, including
    collections, so arrange steps are portable across modes.
- Login against real auth: use `@include steps/login.cdp` with secrets. The
  resulting session can be cached per backend and worker
  (`@before-all … state save .results/auth/${backend}.json`) to avoid logging
  in for every test.

**Determinism in real mode** is best-effort, and the report says so:

- time freezing is off by default (it breaks token expiry and server-side
  time checks);
- animations stay off, and random seeding stays on;
- default retries are 1 (configurable) and flaky tests are always reported.

**Keeping mocks honest (`--verify-mocks`):**

- `chrome-cdp test --backend local --verify-mocks` runs against the real
  backend. For every real in-scope request, it also asks the mock engine
  which mock *would* have answered, and compares the two.
  - It compares status and the **shape** of the JSON body: keys, types,
    array element shape, and null vs. present. It does not compare values.
  - The report lists drift per mock, with the mock's file:line, e.g.
    `products.json: real response has field "price_cents" (number); mock has
    "price" (number)`, plus real requests with no mock at all.
- `--verify-mocks --update` rewrites the drifting data files from the real
  responses, keeping existing values where the shape still matches. The
  changes can then be reviewed as a normal diff.
- A nightly CI job running `--backend staging --verify-mocks` is the
  recommended setup, and the `init e2e` workflow template includes it,
  commented out.

**Safety:**

- A backend can set `protected = true` (recommended for anything
  production-like). Such a backend refuses to run unless the scenarios in the
  run are tagged `@safe` (read-only), or `--allow-writes` is passed.
- `http` DELETE/PUT/POST to a protected backend is also blocked outside
  `@safe`-exempted arrange steps.

## Tests

- Mock engine unit tests: every matcher (including GraphQL and JSON subset
  with wildcards), precedence and layering, templates, a missing data file
  giving 404, CORS preflight, sequences, and collections (CRUD, pagination,
  id generation).
- `mock explain` and unmatched diagnostics: golden files.
- Runner tests on fixture suites:
  - pass, fail, skip, only, and flaky (with retries);
  - `@include` with parameters;
  - per-test mock layering;
  - context isolation under `--jobs 8` (every test writes to the same
    collection and must see only its own writes);
  - `--shard` covers every test exactly once across shards;
  - `file.cdp:line` selection;
  - web server start/reuse/teardown (including a server that crashes on start).
- Determinism: the same suite run 20× gives identical `steps.jsonl`
  (timestamps excluded) and identical screenshots.
- `history --as-scenario` round trip: script an interactive session, export
  it, run the export with `test`, and it passes.
- `mock record` round trip: record against the fixture backend, switch the
  backend off, and the recorded mocks make the suite pass.
- `mock check --openapi`: catches a deliberately wrong status and a wrong body
  field in fixtures.
- Real-backend mode: the fixture backend (the axum server from plan 10) is a
  `real` backend for `examples/e2e-shop`. The same suite passes with
  `--backend mock` and `--backend local`, `@backend` markers skip correctly,
  and `http` arrange steps behave the same in both modes. Hybrid mode (real API
  plus a mocked "payment provider") passes too.
- `--verify-mocks`: deliberately drifted mock data (a renamed field, a changed
  type, a missing endpoint) is reported with the right file:line, and
  `--update` fixes the data files without touching values that are still
  valid.
- `fmt`: golden files for messy input, idempotence and AST-equality property
  tests, comment preservation, and `--check` exit codes.
- `check`: one fixture per lint rule, with the expected `file:line:col`, rule
  id and hint, and a golden file for `--format github`.
- Protected backends: writes are refused without `--allow-writes`, and
  secrets never appear in any artifact (grep the `.results/` tree for them).

## Acceptance criteria

- [ ] `examples/e2e-shop` passes on all CI runners, with `--jobs 4`, and
      20× repeated with no flaky results.
- [ ] The same `examples/e2e-shop` scenario files pass against the real fixture
      backend (`--backend local`), with only the tests marked `@backend mock`
      skipped.
- [ ] `--verify-mocks` detects every deliberately introduced drift in the
      drift fixture set.
- [ ] A fresh clone plus `chrome-cdp test` in `examples/e2e-shop` works with no
      other setup beyond installing chrome-cdp.
- [ ] Overhead of the runner itself (excluding page work): < 100 ms per test.
      The 30-test example suite finishes in < 15 s with `--jobs 4`.
- [x] Every failed test produces a `report.md` that names the failing
      step, the cause, and a hint (audited on a set of deliberately broken
      tests).
- [ ] The agent benchmark (plan 10) includes "write E2E tests with mocks for
      this frontend", "make this suite also pass against the real backend", and "a change broke the checkout E2E test; fix the app".
      With only `chrome-cdp skill`, Claude Code completes both.
- [ ] `docs/e2e/` is complete, and the mock file JSON Schema is published.
- [ ] `chrome-cdp fmt --check` and `chrome-cdp check` pass on
      `examples/e2e-shop`, and the `init e2e` CI template runs both before
      `test`.

## Decisions log

- The scenario format (`.cdp`) is our own, and deliberately has no control
  flow. Rationale: every step line is a CLI command agents already know, and
  `history --as-scenario` depends on that. It is kept honest by a formal
  grammar (plan 08 §8.2), `fmt`, `check`, and `schema scenario` (§11.12).

- Mock file format is TOML, the same as `chrome-cdp.toml`. YAML was rejected
  because the Rust YAML crates are poorly maintained. Response bodies live in
  separate JSON data files, so TOML's weakness with nested data rarely
  matters. Revisit if agents write TOML mocks unreliably in the benchmark.

- 11.1: The code lives in a new crate, `crates/e2e` (config, web servers, `init`), so the mock engine and the runner of later milestones get a home outside the 5k-line `cli.rs`. The binary only renders `init e2e`.
- 11.1: The project config may be named `chrome-cdp.toml` (this plan) or `.chrome-cdp.toml` (plan 08 §4, as built). Both are found walking up; one directory with both is an error, since which wins would be a guess. `init e2e` writes `chrome-cdp.toml`, or appends to the one that exists.
- 11.1: `[e2e]` is checked by the commands that use it (`e2e::Config`), not on every command as plan 08's keys are, so a typo in it cannot break `click`. Its errors point at `file:line:col`. The CLI's config loader reads only `[e2e].base_url`, which is also the `goto /path` base when the file has no top-level `base_url`. `[e2e]` in the user config is an error.
- 11.1: The whole documented `[e2e]` schema parses now, including `determinism`, `backends` and `default_backend` (which must name `mock` or a backend). Values are checked for shape only; e.g. `freeze_time`'s timestamp is parsed by 11.6. Keys of later milestones (`openapi`, `allow_console_errors`, `mock_server`) are added by those milestones, so until then they are unknown-key errors.
- 11.1: Web server details the plan left open: `reuse_existing` defaults to true, as in the example; a server that answers while `reuse_existing = false` is an error, not a second start. A `cwd` key (relative to the config) serves monorepos. `BROWSER=none` is set unless `env` sets it, so create-react-app opens no window. The readiness GET follows no redirects (a 3xx counts as up), accepts self-signed certificates and ignores proxy variables. Several servers start in order; each gets `web-server-N.log`.
- 11.1: Teardown is SIGTERM to the group, then SIGKILL after 5 s. Dropping `WebServers` kills the groups, and each group has the Chrome watchdog (`browser::launch::Watchdog`, now public), so even a SIGKILL of chrome-cdp leaves no dev server. Ctrl-C is handled by `WebServers::run(work, interrupt)`, which also stops the work when a server exits; the runner (11.4) passes `tokio::signal::ctrl_c()`. No command starts web servers until the runner lands.
- 11.1: `init e2e` takes `--dir` and `--framework vite|next|cra|astro|sveltekit|none`. Frameworks are checked in the order Next, SvelteKit, Astro, CRA, Vite (SvelteKit and Astro apps depend on Vite too). The package manager comes from `packageManager`, then the lock file, else npm. Vite and SvelteKit get `--strictPort`, so a taken port fails instead of moving away from `url`. Beyond the plan's list it writes a data file for the mock set and adds `e2e/.results/` to `.gitignore`; both the config and `.gitignore` are only appended to.
- 11.1: The CI template installs chrome-cdp with `cargo install --locked chrome-cdp` until plan 10 publishes release installers, sets `CHROME_CDP_AUTO_INSTALL=1` (the runner's own Chrome is used when it has one) and caches `~/.cache/chrome-cdp`. The `fmt --check` and `check` steps (11.12) and the `#:schema` comment in the mock file (the schema is published in 11.10) wait for their milestones.
- 11.1: The example scenario uses 11.4's `@test` syntax; a test checks each of its steps with the CLI's script checker.
- 11.2: The mock engine is a new crate, `mock`, with no browser in it: the daemon's router asks it about each paused request, the CLI uses it for `mock explain` without a session, and 11.9's `mock serve` can answer real HTTP requests with it. The output types live in `protocol::schema`, their text forms in `mock::text`.
- 11.2: The CLI turns set names into files (the `[e2e]` mocks directory, or `--dir`, else `./e2e/mocks`) and the daemon reads and checks them, so a bad file fails `mock use` at `file:line:col` and nothing is applied. Mock sets belong to the session, like routes, and are tried after the `route` rules. Per-browser-context sets for parallel tests are 11.4's; the router's `Table::mocks` is where they go.
- 11.2: `mock use a b` applies `default_mocks` then `a b`, replacing the sets in use, as a scenario's `@mocks a b` layers onto the defaults. `--add` layers onto the sets in use, `--no-defaults` leaves the defaults out, and a set named twice keeps its later place. `mock reset` resets what 11.2 has (hit counters and random values); 11.3 adds its state.
- 11.2: Data files are relative to the mocks directory, not to the mock file, so `errors/api-down.toml` shares `data/` with `base.toml`. A set given as a path to a `.toml` file uses that file's directory.
- 11.2: `scope` entries starting with `/` are path globs and ones with `://` are full-URL globs; anything else is an error. Scope only decides what counts as unmatched: a mock answers what it matches, in scope or not, and a set without `scope` owns nothing. The first set (in the order they are tried) whose scope has the request applies its `unmatched` policy.
- 11.2: `unmatched = "fail"` answers 501 with a JSON error that names the request, the set and the `mock explain` command, plus an `X-Chrome-Cdp-Unmatched: <set>` header, and the journal marks the request unmatched; 11.5 fails the test from the journal. `abort` fails it with the network error `failed`.
- 11.2: Globs: `*` stays within a segment, `**` crosses them, and `/**` also matches nothing (`/api/**` matches `/api`). `:name` captures a segment, or its part before the literal text that follows (`:id.json`). A trailing `/` is ignored, and parameters are percent-decoded. `regex` is searched in the full URL, and its named groups are parameters too.
- 11.2: Body matching: `body` is a glob or `{ regex = "…" }`. `form` parses URL-encoded bodies only; multipart fields are a follow-up. `graphql` reads a JSON POST body or GET query parameters, and takes the operation from `operationName` or else the query's first named operation. `json` compares numbers by value (1 matches 1.0).
- 11.2: Templates are minijinja with undefined values as errors. A string that is one `{{ expr }}` and nothing else keeps the value's type, so `qty = "{{json.qty}}"` stays a number. Beyond the plan's variables there are `method`, `url`, `path`, `body`, `form` and `now_ms`. `{{uuid}}` and `{{faker.*}}` hold one value per answer (so `faker.name` and `faker.email` agree), from a SplitMix64 generator seeded by `[e2e.determinism] random_seed` (else 0). `{{now}}` is the session's `time freeze` time if there is one; 11.6 can freeze it per test.
- 11.2: A template error answers 500 with the error, and the journal says `mock error: …`. `*.tmpl.json` data files have their parsed strings rendered, as inline `json` is, so the data needs no escaping. `body_file` files with `.tmpl.` in their name are rendered as text; `file` never is. A templated data path that would leave the mocks directory is a mock error. A missing data file answers 404 `{"error": "not found: <path>"}`.
- 11.2: Loading checks the TOML (unknown keys too), one of `path`/`url`/`regex`, one body matcher, one body, `abort` and `passthrough` alone, statuses, header names, template syntax, and that fixed data files exist (and that a `json_file` is JSON). A `match` with no condition is an error; `path = "/**"` says "everything" on purpose.
- 11.2: `cors = "auto"`: an answer to a request with an `Origin` allows it and also lists the mock's own headers in `Access-Control-Expose-Headers`, since the page cannot read them cross-origin otherwise (route rules' answers get this too). Preflights are allowed for a mock that answers the method, and for anything in an `auto` set's scope, so the page sees the 501 of an unmatched request rather than a CORS error. With `off`, CORS headers are left to the mock, and preflights are matched like other requests.
- 11.2: "Re-read on change": before a decision, at most every 250 ms, the daemon compares the set files' modification times, and reloads a changed file, keeping hit counts by mock name. A file that no longer loads keeps its previous version, and `mock list` shows why. Data files are read for each request, so they are always current.
- 11.2: `mock explain` asks the running session, which also names a `route` rule that answers first. When no session uses mock sets, it reads `--mocks` or `default_mocks` itself and says so in a warning. A relative URL is joined to `base_url`, else to `http://localhost` with a warning.
- 11.2: `mock log` is the routing journal (shared with `routes --log`; 1,000 requests) with only the mocked and unmatched requests, or `--unmatched` only, or `--all`. The journal entries gained `mock` (set, name, file, line) and `unmatched` (the set).
- 11.2: `schema mocks` prints the file format's JSON Schema, generated from the parser's own serde types and their doc comments, so the schema cannot drift from the parser. `schema` lists it as "not a command". Publishing it (and the `#:schema` comment in `init e2e`'s mock file) is 11.10's job.
- 11.2: The `route` code now takes its network-error names, content types and header-name check from `mock`, so `route --abort` and a mock's `abort` accept the same names.
- 11.3: A `sequence` replaces `respond`; the mock's hit count says which answer is next, so `mock reset` starts it over and a reload keeps its place. Each answer takes its own `delay`. `after_sequence = "unmatched"` makes a used-up mock stop matching, so the next mock (or the set's `unmatched` policy) takes the request, and `mock explain` says why.
- 11.3: `[state]` values are one namespace shared by every set in use, so `checkout.toml` can require what `base.toml`'s login sets. Plain `mock use` starts them from the sets' `[state]` (later sets win); `--add` and a reloaded file only add values not there yet. A `when` or `set_state` name no set in use declares is a warning from `mock use`, not an error, since it may come from a set applied later.
- 11.3: `when` matches as `match.json` does, with `"*"` meaning "set to anything". TOML has no `null`, so `unset_state = ["user"]` removes values. `set_state` strings are templates, and `{{state.x}}` is available to every template; an answer sees the values from before its own `set_state`. A template error in `set_state` is a mock error (500) and changes nothing.
- 11.3: A set's collections are tried after its `[[mock]]`s, so a mock in front of a collection wins. A collection also takes `items = [...]` inline instead of a `seed` file; every starting item needs a unique id. Ids are compared as text (`/todos/7` finds `"id": 7`). `new_id` (`increment` or `uuid`) defaults to `increment` when every starting id is a number (or there are none), else `uuid` (seeded, so reruns match).
- 11.3: REST details the plan left open: POST answers 201 with the item and a `Location`, and 409 for an id that exists; PUT replaces and PATCH merges (JSON merge patch), both keeping the id; DELETE answers 200 with the removed item rather than 204, so an app that calls `res.json()` works; a body that is not a JSON object is 400; a method the path does not take is 405 with `Allow`. Lists always carry `X-Total-Count`; `wrap` answers `{ "<wrap>": [...], "total": N }`. `filter_query` compares fields as text and ignores parameters no item has (cache busters); paging needs `page_param`, counts pages from 1 and defaults to 20 items (`page_size`).
- 11.3: `mock reset` also reads collections' seed files again. A changed mock file keeps the collections' items and hit counts by name.
- 11.3: `mock state [NAME] [--set name=value]` shows (and sets) the state; `--set` takes names a `[state]` declares or a mock has set. `mock seed <collection> [FILE|-]` looks for FILE in the mocks directory, then relative to the current directory; without FILE the collection gets its seed items again.
- 11.3: `expect mock-state <name> count|value|contains <v>` needs no page: it looks at the session's mocks every 25ms until the check holds or the timeout passes, since the page changes them by sending requests. Values are JSON, else text. Its `--json` output leaves out `tab`. "Reset for each test" is the runner's job (11.4): each test's browser context gets its own `Mocks`.
- 11.4: Browser contexts are keyed by Chrome's `browserContextId` (the "scope"). A context made with `context new` starts with nothing: its routes, mock sets and state, init scripts, exposed functions, storage seeds and emulation are its own, and the session's (set without `--context`) never apply in it, nor its in the default context. Permissions, HTTP credentials and the dialog, download, popup and service-worker settings stay the session's. Contexts are made with `disposeOnDetach`, and a new browser forgets them.
- 11.4: `--context` travels in the RPC request beside the params (`protocol::rpc::Request::context`); the daemon runs the call inside the engine's task-local `in_context`, so no request type changed. Ids are `c1`, `c2`, … and never reused. Each context has its own active tab: with `--context`, a page command acts on it (opening a blank tab when the context has none), and `--tab` naming another context's tab is `NotFound`. Without `--context`, `--tab` may name any tab. `context close` takes ids, `--all`, or `--context`'s.
- 11.4: `Fetch.enable` sent again to a page that already intercepts drops its paused requests, which hung a navigation in one context while another ran `mock use`. Interception changes now go only to the page sessions whose state changes.
- 11.4: The scenario model is `e2e::scenario` (no clap: loading, `@include`, variables, selection, shards, unit-tested without a browser); the runner is the CLI's `test` module, because steps are CLI command lines. Steps run in the CLI process through the same `run(cli)` a single command uses, as `batch` does, with `--session` and `--context` set.
- 11.4: Directive details the plan left open: a command outside a block is an error; settings go at the top of the file, in a test's body, or after its name (`@test "x" @skip "why" @tags a`); hooks hold only commands and `@include`s; a file has at most one of each hook; a test with no steps is an error unless skipped. File and test settings layer: `@mocks` and `@tags` add up (a set named again moves last), the rest is replaced, and `@viewport` and `@device` replace each other. `file.cdp:LINE` picks the test from its `@test` line to the line before the next top-level line.
- 11.4: `@include file k=v…`: the parameters are expanded with the including line's variables; a step file's lines see only their own parameters, and its `data:` paths are relative to it; it holds commands and `@include`s only; a cycle is an error. `${var:}`, `${secret:}`, `${saved.…}`, `${test.id}` and `${run.id}` stay errors until 11.11.
- 11.4: `@before-all` runs once per file, before the first of its tests a worker takes (others wait), in a context of its own with the file's settings; since contexts are fresh, it hands state to tests through files (`state save`/`state load`). A failed `@before-all` fails every test of the file. `@after-all` runs after the file's last test, and its failure fails that test. `@after` runs after a failed test too, with 10s past the time limit.
- 11.4: Tests run in a fixed order (files by path, tests as written); `jobs` worker threads take the next one, and lines print in that order as results come in. `--shard i/n` takes consecutive near-equal parts. The time limit cuts each step's `--timeout` to the time left when it would run past it; a step that ends past the limit fails with `Timeout`.
- 11.4: The run uses a session of its own, `test-<pid>` (or the one `--session`/`CHROME_CDP_SESSION` names), opened with `open [--headed]` and closed at the end if the run started it; `--jobs-isolation browser` uses `<session>-1…N`. A session the run named itself also has its directory removed after `close`, so runs leave none behind; a `--session` one is left as it was. Without a project config, the defaults apply, rooted at the current directory. Web servers start first, log to `artifacts`, and a server that exits or Ctrl-C stops the run (the tests not started are not run).
- 11.4: Every file is read and every step checked before a browser starts; all problems come back as one `BadArgs` error with `file:line:col` (exit 2), as with `run`. The script checker no longer takes `expect mock-state`'s first word for a locator (`run` and `batch` had the same bug).
- 11.4: Output: a line per test (`✓`, `✗`, `~` flaky, `-` skipped or not run) with a failure's step, `@include` chain and error under it, then `N tests: … (time)`. A failed run is an `ExpectFailed` error naming the failed tests, whose hint reruns the first by `file:line` and whose details are the `--json` data (`chrome-cdp schema test`). Hints of step errors lose `--session test-<pid>`, since that session is closed when anyone reads them; 11.7's `--debug` keeps a session alive instead. `--list` prints the tests without a browser.
- 11.4: `expect-snapshot` and `expect-screenshot` steps without `--dir` use `e2e/snapshots` of the project; `--update-snapshots` sets `--update` on them. `run file.cdp` still resolves baselines as before.
- 11.4: Flags of later milestones are not added yet: `--backend` (11.11), `--reporter`, `--watch`, `--debug`, `--trace` (11.7) and `--repeat` (11.6); `--max-chars` bounds the JSON.
- 11.4: The "about 50 ms" per context and the runner-overhead criterion are timings: deferred, benchmarks run after all plans. The runner tests cover pass, fail, skip, `@only`, flaky with retries, `@include` with parameters, per-test mock layering, isolation under `--jobs 8`, shards, `file:line`, `--bail`, a Chrome per job, snapshot baselines, and web servers that are reused or crash on start; starting and tearing down a real server is covered by 11.1's `web_server` tests.
- 11.5: `expect request` takes `--json-body '<subset>'`, not `--json`, which is the global output flag; `mock explain` and `route` already use `--json-body`. `METHOD` may be `*`; `PATH` is a path glob (as a mock's `path`), `**/…` for any prefix, a full-URL glob, or `*`, and a `?k=v` after it adds query conditions. `--not` is refused: `expect no-request` says it.
- 11.5: `expect request` reads the network logs of the session's tabs (with `--context`, the context's: each test's own; `--tab` one), not the routing journal, so it works with mocks, routes or neither (11.11's real mode) and sees headers and bodies; a body too large for the log is read from Chrome. A redirect's hops count once. `--since <mark>` counts the mark's tab's requests after it and other tabs' from the time it was set (marks now keep that time).
- 11.5: How many: at least one by default. Too few are waited for until `--timeout`; too many (`--count`, `--max`, `no-request`) fail at once, since more requests cannot undo them. A failure lists up to 5 matching requests and the 5 closest others, each with the condition it fails and its `network show` id.
- 11.5: "Closest" (mocks of an unmatched request, requests of a failed `expect request`) is ranked by the path segment by segment (a segment matching, or a few letters away by edit distance, 60% alike or more), then the exact path over the method, then the query, header and body conditions. Only candidates with as many segments (give or take one) all alike are named, so a shared `/api` alone is not "close". The router works the closest mocks (at most 2) out when the request goes unmatched, so a `when` or a used-up sequence is judged as it was then, and keeps them in the journal entry with the set's `policy` (`closest`, `policy` in `mock log --json`; `mock log` shows them).
- 11.5: The runner reads the context's mock log after every step; an unmatched request in the scope of an `unmatched = "fail"` set (`passthrough` and `abort` ones do not fail) fails the test at the step after which it was seen (a late request may be seen after a later step). When that step failed on its own, its error gets the unmatched requests as notes instead, since a 501 is the likely cause. The hint is `mock explain <METHOD> <url> --mocks <the test's sets>`, which works after the run's session is gone.
- 11.5: The automatic check is a new flag, `expect --no-errors --all-tabs`: every tab of the context, each since it opened, so an error on a page the test navigated away from still counts. It runs after the test's own steps pass and before `@after`; its failure is reported as `(checked after the test's steps)` (`hook: "check"`). `@allow-console-errors` (a setting of a file or a test) and `allow_console_errors = true` in `[e2e]` turn it off.
- 11.5: `mark` ignored `--context` (it marked the session's active tab), so `--since` in a test never found its mark; it now takes the context's tab as other commands do.
- 11.5: Since unmatched requests now fail tests, a body mocks cannot see would fail a good test. The worry that Chrome leaves large bodies out of `Fetch.requestPaused` did not hold on Chrome 154: 20 MB and `Blob` bodies arrive whole, so no `Network.getRequestPostData` fallback was added; `mocks_match_large_request_bodies` guards it. Multipart forms with files remain a follow-up.
- 11.6: Time stays the init-script clock of plan 07 (Chrome's virtual time stalls page loads), so timers keep real time and a page never stalls. Moving the frozen time on (`time advance`, or a later `time freeze`) also moves `performance.now()` and runs the `setTimeout`/`setInterval` callbacks due by then, each once, in the order they are due (Playwright's `clock.fastForward`), which is what testing expiry, polling and debounce needs. `requestAnimationFrame` is left alone, and timers set in an open page before the freeze are not tracked; pausing timers (`clock.pauseAt`, `runFor`) is not done.
- 11.6: Defaults when `[e2e.determinism]` says nothing: time frozen at `2026-01-01T10:00:00Z` (the plan's example), seed 0 (as the mocks' already was), animations off. The plan's fixed `en-US` and `UTC` became two new keys, `locale` and `timezone` (a name, or `false` for the browser's own), since an app for another market wants its own. `freeze_time` is checked when the config is read (`parse_time` moved to `protocol::format` for it). `Determinism::with` layers a backend's settings for 11.11.
- 11.6: The runner sets a test's context up with the CLI's own commands after `mock use`: `viewport WxH --dpr 1` (a `@device` keeps its pixel ratio), `time freeze`, one `init-script add` (seeded random, no animations), `media --reduced-motion reduce`, `locale`, `tz`. Hooks' contexts get the same. Mocks' `{{now}}` already followed the context's frozen time and their random values the seed, reset per test by the context's `mock use`, so mock latency needed nothing new.
- 11.6: Seeded random is sfc32, started over in every document, so a reload sees the same values. It covers `crypto.getRandomValues` (integer arrays) too, since uuid and nanoid libraries use it; workers keep real randomness.
- 11.6: Animations get `animation-duration: 0s` (with no delay, one iteration) rather than the plan's `animation: none`, so they end in their final state and still fire `animationend`, which apps that remove a dialog after its animation wait for; transitions are `none`, and `scroll-behavior` is `auto` too. The sheet is adopted (`document.adoptedStyleSheets`), so it applies before the first paint without a DOM node; shadow roots are not covered.
- 11.6: `--repeat N` conflicts with `--retries`, and turns retries (config and `@retries`) off, since they would hide what it looks for. Runs of a test spread over the workers and merge into one line and result: failed if a run failed (`failed 2 of 20 runs`, with the first failure), `failed_runs` and `repeat` in `--json`, and the hint reruns with `--repeat`.
- 11.6: Fonts: `docs/e2e/Dockerfile` (Node for web servers, a fixed font set, a pinned Chrome for Testing) is documented in `docs/e2e/determinism.md`; publishing a prebuilt image is plan 10's release work. `expect-screenshot` warns when the baseline of this OS is missing but another OS's exists (a note on the error in CI), and the runner shows `expect-screenshot` warnings under the test's line and in `--json` (`warnings`); other steps' warnings wait for 11.7's reports.
- 11.7: Artifacts go to `<artifacts>/<file>/<test-slug>/`, `<file>` being the scenario's path below the literal folder its `tests` glob starts in (`checkout.cdp` for `e2e/scenarios/checkout.cdp`), with `.cdp` kept so a folder of the same name cannot collide; a slug two tests of a file share gets `-<line>`. A retry writes to `try-N/`, a `--repeat` run to `run-N/`, a failed `@before-all`/`@after-all` to `<file>/@before-all/`; a run first removes the directories of the tests it selects. `--json` gained `artifacts` (per test) and `report` (per failure).
- 11.7: `steps.jsonl` records the runner's setup commands (`hook: "setup"`, without their output, whose ids change per run) and its `expect --no-errors` check (`"check"`) besides the steps; outputs lose the run's `--session test-<pid>`, so runs differ only in timestamps (and context ids with `--jobs` > 1). Every step's warnings are kept there and in `report.md`'s step lines; the text output still shows only `expect-screenshot`'s.
- 11.7: `requests.txt` merges each tab's network log with the routing journal (`mock log --all`), so it lists requests no mock saw too (real mode, 11.11) and names the mock, route or unmatched set of those it did. The page is captured (`failure.png`, `snapshot.txt`) before `@after` runs; the journals after it, before the context closes. A reading command that fails leaves its file out and the report says why.
- 11.7: `report.md` shows the snapshot excerpt around the refs the error's message names, else the name in the step's locator, else the refs of its notes, else the top 30 lines. A hint that reads the page (`snapshot`, `errors`, `console`, `network`, `screenshot`) is rewritten to point at the file holding it, since the context is gone; this settles part of the "session-bound hints" follow-up.
- 11.7: `--reporter` takes `text,json,junit,html`, each optionally `NAME=FILE` (default: `results.json`, `junit.xml`, `report.html` in the artifacts directory). stdout always has the text (or `--json`) output, so `text` is accepted but changes nothing. JUnit marks flaky tests with Surefire's `<flakyFailure>`; the HTML page inlines screenshots as `data:` URLs so it is one file. A report that cannot be written is a warning, not a failed run. The text reporter's "failure summaries inline up to a size limit" is the existing per-test failure block (each error is already bounded) plus the `report:` line.
- 11.7: `--debug` names the run's session `test-debug` (closing one an earlier `--debug` run left), turns `--bail` on and retries off, and on the first failure skips that test's `@after` and leaves its context open. Hints need `--context` too (the plan's `chrome-cdp --session test-debug snapshot` would act on the session's default context), so the hint is `chrome-cdp snapshot --session test-debug --context cN`, and the failed step's hints gain `--context`. The session's idle timeout (30 min) still applies. A web server the run started is stopped at its end (a note says so); keeping it would mean the CLI never exits.
- 11.7: `--watch` polls files (every 250 ms, then until they stay unchanged for 300 ms) instead of using file-system events, which needs no new dependency and behaves the same on Linux and macOS. "The web server's output files" became a new `[e2e] watch` key (globs, walked into `dist/` and `build/` too); `init e2e` writes it commented out. It conflicts with `--debug` and `--list`; Ctrl-C ends it with the last run's exit code (without web servers too), closing the session.
