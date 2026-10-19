# Load testing with real browsers

`chrome-cdp load` runs many users through the project's scenario tests at
once, each user a real browser running the app's own JavaScript, against
a real backend. It shows how the site holds up when many people use it,
and what those people see while it does.

```sh
chrome-cdp load e2e/scenarios/shop.cdp:4 --backend staging --users 50 --duration 5m
chrome-cdp load e2e/scenarios/shop.cdp --grep browse --users 200 --ramp-up 1m
chrome-cdp load e2e/scenarios/shop.cdp --users 20 --iterations 5 --raw
```

This page covers milestone 23.1 of the plan (docs/plans/23-load-testing.md):
the runtime, users that loop their journeys, and what a run keeps. Load
profiles (arrival rates, journey weights, per-user data), the metrics
report and thresholds come later.

## Journeys

The paths (and `--grep`, `--tags`) select tests as `test` selects them;
those tests are the users' journeys. Each user runs them in turn, over and
over, until `--duration` is up or it has done `--iterations`; the
iterations that are running then finish. `--ramp-up 1m` starts the users
evenly over a minute instead of all at once.

Each iteration runs as `test` runs the test: in a fresh browser context
with one tab, the file's `@before` steps, the test's own and its `@after`
steps (after a failure too). `@before-all` and `@after-all` run once,
before the first user starts and after the last one ends. `${test.id}` is
unique per iteration (`<run>-u<user>-<iteration>`), so journeys that
create accounts or orders do not collide, and `${saved.…}` reads what the
iteration's own `http … --save` steps saved.

A load step is lean: it runs its action or check and waits for the page
to settle as a test step does, and that is all. It builds no action
report, takes no snapshot, checks no mocks and collects no logs. `expect`
keeps its retries and timeout.

These commands are load steps: navigation (`goto`, `back`, `forward`,
`reload`), the input actions (`click`, `fill`, `press`, …), `expect`,
`wait`, `http`, `eval`, `cookies`, `storage` and `think`. Any other step,
and a ref like `e12`, is an error before any browser starts, with every
such step listed: `snapshot`, `screenshot`, `debug` and the like are for
one interactive user.

## Think time

People pause between pages. `think` makes a user pause too:

```sh
chrome-cdp think 3s        # three seconds
chrome-cdp think 2s..8s    # a time picked evenly between the two
chrome-cdp think           # the load run's --think (2s..8s by default)
```

In a scenario it is a step like any other. Under `test`, `run`, `batch`
and on the command line it does nothing, so the same scenario runs as a
test at full speed. Think time does not count towards an iteration's
`@timeout`.

## Backends

A load run needs a real backend: `--backend NAME` of
`[e2e.backends.NAME]`, else the config's `default_backend`. It refuses the
mock backend, since load testing the mocks measures chrome-cdp, not the
site. The backend's web server starts as for `test`, unless something
already answers on its URL, and its setup and teardown commands run
around the run. A protected backend
([backends.md](backends.md#protected-backends)) runs only journeys
marked `@safe`, and `http` writes only in their hooks, unless
`--allow-writes`.

## Browsers

The users share `--browsers` Chromes, each user in a browser context of
its own, at most `--users-per-browser` (10 by default) in one Chrome.
Past 25, users in one Chrome slow each other down: every call of theirs
goes through its one browser process and CDP connection, and the run
warns.
Without `--browsers`, the run starts as many as that takes. All of a
Chrome's contexts share its one CDP connection.

`load` drives its Chromes itself, without a daemon: each user is a task
on one runtime sized to the machine's cores, and its steps call the
browser engine directly. When a Chrome crashes, its users' steps fail
with a `browser_crash` error; the next iteration of the first of them
starts it again, and they all go on in the new one.

## What a run keeps

Each run has a folder, `e2e/.results/load/<run>/` (under the config's
`artifacts`):

- `failures/<n>-vu<user>-i<iteration>/`: the first `--keep-failures`
  (20) failed iterations keep the files a failed test keeps:
  `failure.png`, `snapshot.txt` (the page's controls), `console.txt`,
  `requests.txt` and `report.md`, which names the step and its error.
  After that, failures are only counted, so a broken site does not fill
  the disk.
- `steps.jsonl`, with `--raw`: one line per step, with the user, the
  iteration, the journey, the step's place and text, its start and end
  in ms since the users started, and its result (`ok`, `failed`,
  `timeout` or `error`) with the error's kind.

The summary says how many iterations ran and failed per journey, the
steps by result, the errors by kind, and how many times a Chrome was
started again. `--json` gives the same as data (`chrome-cdp schema
load`). The run exits 0 when it ran, and with an error when every
iteration failed: the journey is broken, not slow.

## The load server

`testdata/load/server` is a small server for trying `load` out and for
its own tests and benchmarks: the example shop's app and API with
latency and errors you set per endpoint.

```sh
cargo run --release -p load-server -- --port 4176 --latency /api/cart=200ms±50 --errors 'POST /api/orders=2%'
```

`testdata/load` is a project whose backend is that server, with the
reference journey (browse, open a product, add it to the cart).
`cargo xtask bench load` runs it at several user counts; see
[benchmarks.md](../benchmarks.md#load-suite).
