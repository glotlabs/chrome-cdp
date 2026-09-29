<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp test`

Run the project's E2E scenarios, each test in a fresh browser context with its mocks

```text
chrome-cdp test [OPTIONS] [PATH]...
```

Arguments:

- `<PATH>`: Scenario files or folders, or FILE:LINE for the test at that line. Default: the config's `tests` globs

Options:

- `--grep <REGEX>`: Only the tests whose `file > name` matches this regular expression
- `--tags <TAGS>`: Only tests with one of these tags; `!tag` leaves a tag out, e.g. smoke,!slow
- `--jobs <N>`: How many tests run at once. Default: `jobs` in the config, else 1
- `--jobs-isolation <HOW>`: Where the tests run at once: in browser contexts of one Chrome (the default), or in a Chrome each (one of `context`, `browser`; default `context`)
- `--shard <INDEX/TOTAL>`: Run only part INDEX of TOTAL near-equal parts, e.g. 2/4 (for CI machines that split a suite)
- `--retries <N>`: Run a failed test again up to N times. Default: `retries` in the config, else 0; a test's @retries wins
- `--repeat <N>`: Run each test N times, without retries, to find flaky tests: a test fails if any of its runs does
- `--bail`: Stop after the first failed test
- `--headed`: Show a browser window
- `--update-snapshots`: Write the baselines of expect-snapshot and expect-screenshot steps (in e2e/snapshots) instead of comparing with them
- `--list`: List the tests the run would have, without running them
- `--trace`: Take a screenshot after every step, into the test's trace/ folder (and the html report)
- `--debug`: On the first failure, stop and leave that test's browser context open (session test-debug), with its mocks, to look at with the usual commands. Retries are off
- `--watch`: After the run, run the affected tests again whenever a scenario, mock, data file or a file of [e2e] watch changes, until Ctrl-C
- `--reporter <NAMES>`: Also write reports to files, comma-separated: json, junit, html (in the artifacts directory, or NAME=FILE); text is stdout
- `--backend <NAME>`: The backend: mock (the mock sets), a real one of [e2e.backends.NAME], or all of them in turn. Default: the config's default_backend, else mock (env `CHROME_CDP_BACKEND`)
- `--verify-mocks`: With a real backend: compare each answer with the mock that would have given it, and report the mocks whose JSON shape drifted
- `--update`: With --verify-mocks: rewrite drifted JSON data files from the real answers, keeping the values whose shape still fits
- `--allow-writes`: Run the tests not marked @safe on a protected backend too; `http` writes are then allowed in their steps
- `--record-mocks`: With a real backend: write each passed test's fetch and XHR requests as a mock set, recorded/&lt;file&gt;/&lt;test&gt; in the mocks directory (replacing an older one)
- `--max-chars <N>`: Bound the JSON output to this many characters (0 = no limit) (default `200000`)

```text
Finds the scenarios by the `tests` globs of [e2e] in chrome-cdp.toml
(default e2e/scenarios/**/*.cdp), starts the [e2e.web_server]s unless
something already answers on their URLs, and opens a browser session of
its own. Each command the config runs (a web server's, a backend's setup
and teardown) is first shown on stderr: `running: <command> (from
<config path>)`. Every test gets a fresh browser context: its own cookies,
storage, routes and mock sets (default_mocks, then the file's and the
test's @mocks), so tests cannot leak into each other. Each also sees the
same time (2026-01-01T10:00:00Z), random values, locale (en-US), time
zone (UTC) and no animations; [e2e.determinism] changes that, and `time
advance 5m` moves the time on and runs the timers due by then. `--jobs N`
runs N tests at once in one Chrome; a test that fails, then passes on a
retry, is reported as flaky; `--repeat 20` runs each test 20 times to
find flaky ones. A request no mock answers, in a set's scope with
unmatched = "fail", fails the test at the step after which it was
sent, naming the closest mocks; so does a console error in any of its
tabs (unless @allow-console-errors, or allow_console_errors = true in
[e2e]). Exit code 0: every test passed; 1: a test failed.
Each test leaves steps.jsonl, requests.txt and console.txt in
e2e/.results/<file>/<test>/; a failed one also failure.png, snapshot.txt
and report.md (the step, the error, the page around it, console errors,
failed requests), whose path is printed under its line. --trace adds a
screenshot after every step; --reporter json,junit,html writes those
files too (docs/e2e/debugging.md).
A scenario that does not check (bad syntax, unknown command, missing
@include) fails before any test runs, with exit code 2.

Backends (docs/e2e/backends.md): --backend NAME runs the same scenarios
against a real backend of [e2e.backends.NAME] (its base_url, web_server,
setup commands and vars); the mock sets are off but for the backend's
own `mocks`, and time is not frozen. `--backend all` runs every backend
in turn. @backend mock (or real, or a name) limits a test or file to
those backends; the others skip it, with the reason. ${var:name} comes
from the backend's vars file, ${secret:NAME} from the environment
(masked in all output), ${test.id} is unique per test, and `http POST
/api/items --save item` arranges data (${saved.item.id}). --verify-mocks
compares every real answer with the mock that would have given it, and
reports mocks whose JSON shape drifted (--update fixes their data
files); --record-mocks writes each passed test's requests as a mock set.
A protected backend runs only @safe tests unless --allow-writes.

Scenario syntax (docs/e2e/scenarios.md):
  @mocks base checkout          # file settings: @mocks @tags @skip @only
  @before                       #   @timeout @viewport @device @retries
    goto /                      #   @allow-console-errors
  @test "adds an item" @tags smoke
    click text=Add
    expect text=1 item visible
    expect request POST /api/cart --json-body '{"qty":1}'
  @test "logs in"
    @include ../steps/login.cdp email=a@example.com

Examples:
  chrome-cdp test
  chrome-cdp test e2e/scenarios/checkout.cdp:12     # the test at line 12
  chrome-cdp test --grep cart --tags smoke,!slow
  chrome-cdp test --jobs 4 --shard 2/4 --retries 1
  chrome-cdp test e2e/scenarios/cart.cdp --repeat 20   # flaky before a commit?
  chrome-cdp test --debug      # keep the first failed test's page open
  chrome-cdp test --watch      # run the affected tests again on each change
  chrome-cdp test --reporter text,junit,html --trace
  chrome-cdp test --backend local              # against the real API
  chrome-cdp test --backend staging --verify-mocks
  chrome-cdp test --list
```

