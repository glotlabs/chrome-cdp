# Debugging failed tests

`chrome-cdp test` is built so that an agent can fix a failure without
running anything again: every test leaves its artifacts on disk, and a
failed one leaves a `report.md` that explains it.

## Artifacts

Each test writes to `<artifacts>/<file>/<test>/` (`artifacts` in `[e2e]`,
default `e2e/.results`; `<file>` is the scenario's path below the folder
its `tests` glob starts in, `<test>` its name as a slug):

| File | What |
|---|---|
| `steps.jsonl` | One JSON object per step, the runner's setup included: `n`, `start_ms`, `duration_ms`, `at` (`file:line:col`), `command`, `hook`, `ok`, `output`, `warnings`, `error`, and with `--trace` a `screenshot` |
| `requests.txt` | Every request of every tab: method, status, URL, type, time, and the mock (or route) that answered it, or the set it went unmatched in |
| `console.txt` | The console of every tab |
| `failure.png` | A failed test: the page when it failed (before `@after`) |
| `snapshot.txt` | A failed test: `snapshot -i` when it failed |
| `report.md` | A failed test: the summary below |
| `trace/NNN.png` | `--trace`: the page after step NNN |

A retry's files go to `try-2/`, `try-3/`, …; `--repeat`'s runs to
`run-1/`, `run-2/`, …; a failed `@before-all` or `@after-all` to
`<file>/@before-all/`. A run removes what an earlier run left for the tests
it selects. `--json` names each test's `artifacts` directory and each
failure's `report`.

## report.md

The path of a failed test's `report.md` is printed under its line and
again at the end of the run. It holds:

- the failing step, its `file:line:col` and the `@include` lines it came
  through;
- the error with its notes and hint (a hint that would read the page, such
  as `chrome-cdp snapshot`, points at the file that holds it instead, since
  the test's browser context is closed);
- the last steps before it, with their durations;
- the part of `snapshot -i` around the step's target (the refs the error
  names, or the name in its locator);
- console errors, and requests that failed, got a status of 400 or more,
  or no mock answered;
- the paths of the other artifacts, and the command that runs the test
  again.

## Reporters

stdout always has the text output (or `--json`). `--reporter` adds files,
in the artifacts directory unless `NAME=FILE` says where:

- `json`: `results.json`, the `--json` data;
- `junit`: `junit.xml`, a `<testsuite>` per scenario file; flaky tests
  pass with a `<flakyFailure>`;
- `html`: `report.html`, one self-contained page with every test's steps
  as a timeline, its screenshots (every step's with `--trace`), its request
  journal and console.

```
chrome-cdp test --reporter text,junit=out/junit.xml,html --trace
```

## --debug

`chrome-cdp test --debug` stops at the first failure and leaves that
test's browser context open in the session `test-debug`, with its mocks
still answering (its `@after` does not run). Retries are off. The error's
hint is the command to look at it:

```
error: 1 of 4 tests failed
  failed: e2e/scenarios/checkout.cdp:12 retries when the API is busy
  debug: e2e/scenarios/checkout.cdp:12 left its browser context c3 open in session test-debug, with its mocks; it closes after 30 minutes idle
  hint: chrome-cdp snapshot --session test-debug --context c3   (the page as the test left it), or after a fix, chrome-cdp test e2e/scenarios/checkout.cdp:12 --debug
```

Every command works there (`--session test-debug --context c3`, or
`export CHROME_CDP_SESSION=test-debug CHROME_CDP_CONTEXT=c3`): `snapshot`,
`console`, `network`, `mock log`, `mock explain`, `click`, …. The next
`--debug` run closes it first; `chrome-cdp close --session test-debug`
closes it by hand. A web server the run started is stopped at its end, so
start the app yourself (`reuse_existing` uses it) when the page needs it.

## --watch

`chrome-cdp test --watch` runs the tests, then waits for files to change
and runs the tests they affect again, in the same browser and with the web
servers still up, until Ctrl-C:

- a scenario, a step file one `@include`s, or a file one of its steps
  reads (`${data:users.json#/0}`, `state load state/user.json`): that
  file's tests;
- a mock file: the tests that use its set; a mock data file: every test
  with mock sets;
- the config, or a file of `watch = ["dist/**"]` in `[e2e]` (a build's
  output, say): every test.

A scenario that no longer checks prints its error and waits for a fix.
