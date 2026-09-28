# Plan 11 — End-to-end testing with a mocked backend

Status: IN PROGRESS (11.1 done)
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
- [ ] Every failed test produces a `report.md` that names the failing
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
