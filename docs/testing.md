# Testing with `.cdp` files

A `.cdp` file is a list of chrome-cdp command lines: what you would type,
without the `chrome-cdp` in front. The same syntax covers three uses,
from a one-off to a test suite:

| Use | Command | For |
|---|---|---|
| many steps in one call | `chrome-cdp batch` (stdin) | an agent doing a known flow in one round trip |
| a script | `chrome-cdp run flow.cdp` | a repeatable flow: log in, seed data, a smoke check |
| tests | `chrome-cdp test` | scenario files with `@test` blocks, mocks and reports |

The grammar is specified in [scenario-syntax.md](scenario-syntax.md); this
page shows how to use it.

## `batch`: one call, many steps

```sh
chrome-cdp batch <<'EOF'
goto localhost:3000/login
fill label=Email ada@example.com
fill label=Password "${env:APP_PASSWORD}"
click 'role=button[name="Sign in"]'
wait --url /dashboard
EOF
```

Every line is checked before the first one runs, so a typo fails before
the browser does anything. Each step prints `[3/5] click … ✓` and its
usual output; the first failure stops the batch (`--continue` goes on) and
sets the exit code. `--json` prints one JSON line per step.

## `run`: a script in a file

```
# login.cdp: sign in as ${email}
goto /login
fill label=Email ${email}
fill label=Password "${env:APP_PASSWORD}"
click 'role=button[name="Sign in"]'
expect --url /dashboard
expect 'role=heading' text "Welcome"
```

```sh
chrome-cdp run login.cdp --var email=ada@example.com
```

- `#` starts a comment. Quoting works as in a shell: `'…'` is literal,
  `"…"` understands `\"`, `\\`, `\n` and `${…}`. A trailing ` \`
  continues a line.
- `${name}` comes from `--var name=value`, `${env:NAME}` from the
  environment, and `${data:users.json#/0/email}` from a JSON file next to
  the script.
- Errors point at the line: `login.cdp:4:1: no element matches …`.
- There are no loops or conditions, by design: a script reads the same in
  a diff as it runs. Call chrome-cdp from a shell for anything more.

`chrome-cdp lint login.cdp` checks a file without running it (unknown
commands and flags, bad locators, refs, which break in a script), and
`chrome-cdp fmt login.cdp` tidies it.

## Assertions: `expect`

A script checks things with `expect`, which retries until it holds (5 s by
default) and fails with exit code 1 and what it saw:

```sh
chrome-cdp expect e6 visible
chrome-cdp expect 'role=alert' text Saved
chrome-cdp expect '#email' value alice@example.com
chrome-cdp expect '.row' count 3
chrome-cdp expect '.spinner' visible --not
chrome-cdp expect --url '**/dashboard'
chrome-cdp expect --no-errors                     # no exception or console error
chrome-cdp expect --no-issues                     # none of Chrome's issues (see `issues`)
chrome-cdp expect --no-failed-requests --all-tabs  # in every tab, since each opened
chrome-cdp expect --a11y                          # accessibility rules (docs/accessibility.md)
chrome-cdp expect --perf 'lcp<2.5s cls<0.1'       # performance budgets on `perf`'s numbers
chrome-cdp expect --audit --baseline checkout     # no audit finding the baseline does not accept (docs/audits.md)
chrome-cdp expect request POST /api/orders --json-body '{"qty":2}'
chrome-cdp expect-snapshot checkout               # the tree against a saved baseline (or template)
chrome-cdp expect-screenshot header 'role=banner' # pixels against a saved baseline
```

`chrome-cdp help expect` lists every matcher.

## `test`: scenario files

A scenario file groups steps into tests with directives:

```
# e2e/scenarios/login.cdp
@before
  goto /login

@test "signs in"
  fill label=Email ada@example.com
  fill label=Password "${env:APP_PASSWORD}"
  click 'role=button[name="Sign in"]'
  expect --url /dashboard

@test "rejects a wrong password"
  fill label=Email ada@example.com
  fill label=Password wrong
  click 'role=button[name="Sign in"]'
  expect role=alert text "Wrong email or password"
```

```sh
chrome-cdp test e2e/scenarios/login.cdp
```

```
running 2 tests from 1 file, 1 at once
✓ login.cdp:4 signs in (1.2s)
✓ login.cdp:10 rejects a wrong password (0.9s)
2 tests: 2 passed (2.3s)
```

Each test runs in a fresh browser context (its own cookies and storage),
so tests do not leak into each other. The runner adds checks of its own:
a test fails on an uncaught exception or `console.error` in the page, and
on a request that a mock set owns but no mock answered, not only on a
failed `expect`. A failed test leaves a `report.md` with the failing
step, a screenshot, the console and the network around it, in
`e2e/.results/`.

`chrome-cdp test` works on any `.cdp` file with `@test` blocks. For a
project, `chrome-cdp init e2e` sets up the rest: a `[e2e]` section in
`chrome-cdp.toml` with the dev server to start and its `base_url`, mock
sets for the backend, and a CI workflow. The E2E docs cover it all:

- [e2e/quickstart.md](e2e/quickstart.md): a first test against a mocked
  backend in five minutes;
- [e2e/scenarios.md](e2e/scenarios.md): every directive, the checks the
  runner adds, running, filtering, retries and output;
- [e2e/mocks.md](e2e/mocks.md): mock files;
- [e2e/debugging.md](e2e/debugging.md): reports, `--debug`, `--watch`.

## Writing tests without typing them

Explore the app by hand (or let an agent do it), then save what worked:

```sh
chrome-cdp goto /login
chrome-cdp snapshot -i
chrome-cdp fill e3 ada@example.com
chrome-cdp click e6
chrome-cdp expect --url /dashboard
chrome-cdp history --as-scenario "signs in" >> e2e/scenarios/login.cdp
```

`history --as-scenario` writes the commands as a `@test`, with each ref
replaced by the most robust locator that found its element (in an
iframe, after a `frame=` step with the frame's name or URL), and leaves
out reads such as `snapshot`. `chrome-cdp convert login.spec.ts` turns a
simple Playwright test into a `.cdp` file ([migrating-from-playwright.md](migrating-from-playwright.md)).

## In CI

Scripts and tests exit non-zero on failure, so any CI runs them.
[ci.md](ci.md) has a GitHub Actions workflow.
