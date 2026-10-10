# Quickstart: E2E tests with a mocked backend

`chrome-cdp test` runs a web frontend's end-to-end tests against mocks of
its backend: no real API, no database, no network, and the same result
every run. A project commits two kinds of plain files:

- **scenarios** (`e2e/scenarios/*.cdp`): what a user does and what should
  happen, in the commands you already use with chrome-cdp;
- **mocks** (`e2e/mocks/*.toml` and `e2e/mocks/data/`): how the backend
  answers.

The same scenarios can also run against a real backend
([backends.md](backends.md)). A complete project to copy is
[`examples/e2e-shop`](../../examples/e2e-shop).

## 1. Set up

In your frontend's folder (the one with `package.json`):

```
$ chrome-cdp init e2e
created  /home/me/app/chrome-cdp.toml            web server: npm run dev -- --port 5173 --strictPort
created  /home/me/app/e2e/scenarios/example.cdp
created  /home/me/app/e2e/mocks/base.toml
created  /home/me/app/e2e/mocks/data/items.json
created  /home/me/app/.github/workflows/e2e.yml
created  /home/me/app/.gitignore                 e2e/.results/
next: chrome-cdp test starts the app and runs the example scenario
$ chrome-cdp test
web server http://localhost:5173 (started in 1.9s)
running 1 test from 1 file, 1 at once
✓ e2e/scenarios/example.cdp:6 the home page loads without errors (0.9s)
1 test: 1 passed (3.1s)
```

`init e2e` finds the framework (Vite, Next.js, Create React App, Astro,
SvelteKit) and writes the dev server's command into `chrome-cdp.toml`; it
never overwrites a file. `chrome-cdp test` starts the dev server (or uses
the one already running), runs each test in a fresh browser context, and
stops the server at the end. Chrome comes from the machine, or
`chrome-cdp install`.

An app whose build is committed or built in an earlier CI step needs no
dev server: `static = "dist"` in `[e2e.web_server]` makes chrome-cdp serve
the folder itself ([`chrome-cdp help config`](../../crates/chrome-cdp/src/help/config.txt)).

## 2. Mock the API

`e2e/mocks/base.toml` is the mock set every test gets (`default_mocks`).
Requests in its `scope` that no mock answers **fail the test**, so a
missing mock never goes unnoticed:

```toml
#:schema https://raw.githubusercontent.com/glotlabs/chrome-cdp/main/docs/e2e/mocks.schema.json
[settings]
scope = ["/api/**"]

[[mock]]
name = "list todos"
match = { method = "GET", path = "/api/todos" }
respond = { json_file = "data/todos.json" }

[[mock]]
name = "todo by id"
match = { method = "GET", path = "/api/todos/:id" }
respond = { json_file = "data/todos/{{params.id}}.json" }
```

A fake REST resource that remembers what the app posts takes three
lines:

```toml
[[collection]]
name = "todos"
path = "/api/todos"
seed = "data/todos.json"
```

Mocks are answered inside the browser, so they need no port and work for
cross-origin API hosts. [mocks.md](mocks.md) is the full file reference;
[recipes.md](recipes.md) has login, pagination, uploads, polling, feature
flags and error states.

## 3. Write a test by exploring

Nobody has to write a scenario from scratch. Use the app with the usual
commands, with the mocks on, then save what worked:

```
$ chrome-cdp open http://localhost:5173
$ chrome-cdp mock use
$ chrome-cdp snapshot -i
$ chrome-cdp fill e4 'Buy milk'
$ chrome-cdp press Enter
$ chrome-cdp expect 'role=listitem' count 3
$ chrome-cdp history --as-scenario "adds a todo" >> e2e/scenarios/todos.cdp
```

`history --as-scenario` turns refs into stable locators, drops the
read-only commands, and keeps the `expect`s:

```
@test "adds a todo"
  goto /
  fill 'role=textbox[name="New todo"]' 'Buy milk'
  press Enter
  expect 'role=listitem' count 3
```

A scenario is a list of chrome-cdp command lines grouped into tests by
directives (`@test`, `@before`, `@mocks`, `@include`, …; see
[scenarios.md](scenarios.md)). Check an assertion about what the app sent
with `expect request POST /api/todos --json-body '{"title":"Buy milk"}'`.

## 4. Run, read, fix

```
$ chrome-cdp test
✗ e2e/scenarios/todos.cdp:1 adds a todo (1.4s)
    failed at e2e/scenarios/todos.cdp:5:3: expect 'role=listitem' count 3
    error: expect failed: role=listitem count 3
      …
    report: /home/me/app/e2e/.results/todos.cdp/adds-a-todo/report.md
```

Each failed test leaves a `report.md`: the failing step and its line, the
error with its hint, the page around the target, console errors, and
unmatched or failed requests. Fix the app (or the test) from it without
running anything again; then rerun the one test by `file:line`. See
[debugging.md](debugging.md) for `--debug` (keep the failed page open)
and `--watch`.

Before committing, `chrome-cdp test --repeat 20` runs each test 20 times
to catch flaky ones, and `chrome-cdp fmt` and `chrome-cdp lint` tidy and
check the scenario and mock files.

## 5. Run it in CI

`init e2e` wrote `.github/workflows/e2e.yml`. It installs chrome-cdp,
caches the browser, checks the files with `chrome-cdp fmt --check` and
`chrome-cdp lint --format github` (problems show up inline on the pull
request), runs `chrome-cdp test --reporter text,junit --shard N/M` on two
shards, and uploads `e2e/.results/` when a test fails. A commented-out
nightly job runs the scenarios against a real backend with
`--verify-mocks` to catch mocks that no longer match the API.

## Next

- [scenarios.md](scenarios.md): directives, running, output;
- [mocks.md](mocks.md): every key of a mock file;
- [recipes.md](recipes.md): common app flows, mocked;
- [authoring.md](authoring.md): `history --as-scenario`, `mock record`,
  `mock import`, `mock gen`, `mock check`, `fmt`, `lint`;
- [determinism.md](determinism.md): frozen time, seeded random values;
- [backends.md](backends.md): the same scenarios against a real backend;
- [beyond-fetch.md](beyond-fetch.md): WebSockets, Server-Sent Events,
  `mock serve` for server-side rendering.
