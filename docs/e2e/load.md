# Load testing with real browsers

`chrome-cdp load` runs many users through the project's scenario tests at
once, each user a real browser running the app's own JavaScript, against
a real backend. It shows how the site holds up when many people use it,
and what those people see while it does.

```sh
chrome-cdp init load                  # writes e2e/load/example.toml
chrome-cdp load example --plan        # what it would run, starting nothing
chrome-cdp load example               # runs it
chrome-cdp load e2e/scenarios/shop.cdp:4 --backend staging --users 50 --duration 5m
```

A run is described by a [profile](#profiles), a file next to the
scenarios that can be reviewed and committed, or, for a quick run, by
flags ([without a profile](#without-a-profile)). This page covers
milestones 23.1 to 23.3 of the plan (docs/plans/23-load-testing.md): the
runtime, a lean browser (`[load]` settings), and the load model. The
metrics report and thresholds come later.

## Profiles

A profile is `e2e/load/<name>.toml`, run with `chrome-cdp load <name>`
(or `chrome-cdp load path/to/profile.toml`). `chrome-cdp init load`
writes an example with the config's real backend and some of the
project's tests, every other key in comments.

```toml
backend = "staging"                 # a real backend of chrome-cdp.toml
think = "2s..8s"                    # what a bare `think` step waits

[[journeys]]
test = "e2e/scenarios/shop.cdp:browse a category"   # file:test name
weight = 70
[[journeys]]
test = "e2e/scenarios/shop.cdp:12"                  # file:line works too
weight = 25
[[journeys]]
test = "e2e/scenarios/checkout.cdp:checkout as a signed-in user"
weight = 5
data = "e2e/load/users.csv"         # one row per iteration, see below

[arrivals]                          # an open model: new users per second
stages = [
  { rate = "0..20/s", duration = "2m" },    # ramp
  { rate = "20/s",    duration = "10m" },   # hold
  { rate = "20..0/s", duration = "1m" },    # ramp down
]
max_users = 400                     # the most at once; above it, arrivals are dropped and counted
```

Paths are relative to the project root. In TOML a key after a table's
header (`[[journeys]]`, `[arrivals]`) belongs to that table, so the keys
of the profile itself come first; a `think` after `[arrivals]` or
`[users]` is taken as the profile's.

| Key | What it is |
| --- | --- |
| `backend` | the real backend of `[e2e.backends]` to load; `--backend` overrides it; default the config's `default_backend` |
| `[[journeys]]` | the tests users run: `test` (`FILE:NAME` or `FILE:LINE`), `weight` (a whole number above 0, default 1), and `data`, `data_order`, `data_exhausted` ([per-user data](#per-user-data)) |
| `[arrivals]` | the open model: `stages` of `{ rate, duration }` (or one `rate` and `duration`), and `max_users` (default 100) |
| `[users]` | the closed model: `count` with `duration` (and `ramp_up`), or `stages` of `{ users, duration }`; `iterations` caps each user's |
| `think` | what a bare `think` step waits: `"3s"`, `"2s..8s"` (evenly between) or `"exp(4s)"` (exponential, 4 s on average); default `"2s..8s"` |
| `think_between_steps` | `true`: a `think` after every navigation and action the journey did not follow with one |
| `on_failure` | `"continue"` (the default) or `"stop-run"`, with `max_failures` (default 100) ([when a step fails](#when-a-step-fails)) |

A rate is a number of arrivals per second, minute or hour: `"20/s"`,
`"90/m"`, and a ramp from one to another, `"0..20/s"`. A stage's users
are a count, `50`, or a ramp, `"0..50"`. Durations are `"30s"`, `"2m"`,
`"1h"`.

With a profile, `--users`, `--duration`, `--ramp-up`, `--iterations`,
`--think`, `--grep` and `--tags` are errors: the profile says what they
would. `--browsers`, `--users-per-browser`, `--browser`,
`--keep-failures` and `--raw` are about the machine and the run's files,
and work as without one.

## Two models

A profile has exactly one of `[arrivals]` and `[users]`.

- **Open, `[arrivals]`** (the default to reach for): users arrive at the
  stages' rate whether or not earlier ones finished, as on a real site.
  Each user does one iteration and leaves. At most `max_users` run at
  once; an arrival above that is dropped and counted as `dropped`, never
  queued.
- **Closed, `[users]`**: a number of users, each looping its journeys.
  User 5 runs while the stages' count is above 4: a ramp from 0 to 50
  starts the users evenly over it, a ramp down stops them (after the
  iteration they are in), and a count that rises again starts them again.

Open is the default because a closed model hides how slow a slow server
is: when each response takes five seconds instead of one, a closed
model's users start their next iteration five times less often, so the
server gets fewer requests just when it struggles, and the run reports a
milder slowdown than real users would see. Under an open model the
arrivals keep their rate, and the slowdown shows in the iterations' time
and in dropped arrivals.

## Iterations

An iteration is one journey, run as `test` runs the test: the user picks
a journey by weight, takes the next row of its data, gets a fresh browser
context with one tab (or keeps its own under `cache = "returning"`), with
the test's `@device` or `@viewport` if it has one, and runs the file's
`@before` steps, the test's own steps and its `@after` steps (after a
failure too), then closes the context. A file's `@before-all` and
`@after-all` run once, before the users start and after the last one
ends.

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

`${test.id}` and `${iteration.id}` are unique per iteration
(`<run>-u<user>-<iteration>`), `${vu.id}` per user (`<run>-u<user>`), so
journeys that create accounts or orders do not collide, and
`${saved.…}` reads what the iteration's own `http … --save` steps saved.
Everything else of the [scenario variables](../scenario-syntax.md)
(`${var:…}`, `${secret:…}`, …) works as under `test`.

### Without a profile

`chrome-cdp load PATH… [--grep] [--tags]` takes its journeys as `test`
takes tests, and runs a closed model: `--users` users (10), each taking
the journeys in turn, over and over, until `--duration` (1m) is up or it
has done `--iterations`; the iterations that are running then finish.
`--ramp-up 1m` starts the users evenly over a minute instead of all at
once, and a bare `think` waits `--think` (2s..8s).

## Think time

People pause between pages. `think` makes a user pause too:

```sh
chrome-cdp think 3s        # three seconds
chrome-cdp think 2s..8s    # a time picked evenly between the two
chrome-cdp think exp(4s)   # exponential, 4 s on average (cut at 40 s)
chrome-cdp think           # the profile's think (2s..8s by default)
```

In a scenario it is a step like any other. Under `test`, `run`, `batch`
and on the command line it does nothing, so the same scenario runs as a
test at full speed. Think time does not count towards an iteration's
`@timeout`. An exponential time is mostly short pauses with a long one
now and then, as people's are.

`think_between_steps = true` in a profile adds the profile's `think`
after every navigation and input action the journey did not follow with
a `think` of its own, for scenarios written as tests, without pauses.

## Per-user data

A journey's `data` is a CSV file (its first line names the columns) or a
JSON array of objects (their keys are the columns). Each iteration takes
the next row, as `${row.<column>}`:

```csv
email,password
load1@example.com,secret1
load2@example.com,secret2
```

```cdp
@test "checkout as a signed-in user"
  goto /login
  fill label=Email ${row.email}
  fill label=Password ${row.password}
```

- `data_order = "sequential"` (the default) takes the rows in order
  across all users; `"random"` picks one at random each time.
- `data_exhausted = "wrap"` (the default) starts again at the first row
  after the last; `"stop"` ends the run when the rows are used up, each
  once, for accounts that can be used only once. The run then stops as
  after Ctrl-C ([stopping](#stopping)), and says why.

A `${row.…}` column the journey's data does not have is an error before
the run, and so is one in a file's `@before` or `@after` that some
journey of the file lacks. `@before-all` and `@after-all` have no row.
Under `test` a test has no row, so a journey that uses one runs only
under `load`.

## When a step fails

The iteration stops at the failed step, records the error, runs its
`@after` steps, and the user starts its next iteration (closed) or leaves
(open). The first `--keep-failures` failures keep their page
([what a run keeps](#what-a-run-keeps)).

For a site that is clearly down, `on_failure = "stop-run"` with
`max_failures = 100` ends the whole run once that many iterations
failed: no new users, the running iterations finish, and the run exits 1
with its report as the error's details.

## Timing

Under `[arrivals]`, an iteration's time counts from when its arrival was
due to start, not from when it got going, so a generator that falls
behind shows as slower iterations instead of hiding it. The arrivals'
times come from the stages alone (at 5/s the k-th arrival is due at
`(k + ½) / 5` s), never from how fast the site answers. When `max_users`
users are running, the arrival is dropped and counted.

The summary gives each journey's iterations with their p50, p95 and
longest time, the arrivals and dropped ones, and the most users at once.
`--raw` writes every iteration's due, start and end time to
`iterations.jsonl`.

## Before anything starts

`load` resolves the profile, the journeys, their data files and every
step's variables, and fails with every problem at once, each at its
place (`e2e/load/shop.toml:12:8`, `e2e/scenarios/shop.cdp:4:3`), before
any browser starts. A journey whose test is not there names the closest
tests; a profile named wrong, the closest profile.

`chrome-cdp load <name> --plan` prints what the run would do and starts
nothing, not even a Chrome:

```text
load plan: open model of e2e/load/shop.toml against backend staging
stages:
  1. 0..10/s over 1m
  2. 10/s for 5m
  about 3,300 arrivals over 6m, each one iteration; at most 200 users at once
journeys (by weight):
     9  e2e/scenarios/shop.cdp:1 browses, 8.2s an iteration last time
     1  e2e/scenarios/shop.cdp:6 signs in (data e2e/load/users.csv), 11.4s an iteration last time
users: about 85 users at once (the peak rate × an iteration's length last time), at most 200
Chromes: 20 (chrome-headless-shell), up to 10 users each; 11 start before the users, the others when first needed
```

The expected users at once are the peak rate times how long an
iteration took in the last run of the same journeys on the same backend
(`e2e/.results/load/journeys.json`); before there is one, the plan says
so. An open model starts that many users' Chromes (and a quarter more)
before the users arrive, and the others when first needed.

## Stopping

Ctrl-C (or SIGTERM) stops new arrivals and iterations, and lets the
running iterations finish for up to `[load] grace` (30 s), then writes
the report. A second Ctrl-C stops them at once, and still writes the
report for what ran; the iterations cut short are counted as such and
left out of the rest, and `@after-all` does not run. The summary's
`stopped` says why the run ended early: `interrupted`, `max_failures`,
`data_exhausted`, or `web_server` (the backend's web server exited).

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
Without `--browsers`, the run has as many as the most users at once
take: a closed model's peak count, an open model's `max_users`. A closed
model's user keeps one Chrome; an open model's arriving user takes a
free place in the first Chrome that has one, and the Chromes it is not
expected to need start only when first needed. All of a Chrome's
contexts share its one CDP connection.

Each Chrome is chrome-headless-shell when it is installed (`chrome-cdp
install --headless-shell`), which takes less memory per user than full
Chrome; without it the run warns and uses full Chrome. `--browser chrome`
picks full headless Chrome for an app that needs it, and `--headed`
shows full Chrome's windows. `$CHROME_CDP_CHROME` beats both. A load
run's Chromes get a few flags more than `open`'s, for many users at once:
no audio, no extensions, no background work, and a 1280x720 window,
which is each page's viewport unless its test has a `@device` or
`@viewport` ([chrome-flags.md](../chrome-flags.md#load-runs)).

`load` drives its Chromes itself, without a daemon: each user is a task
on one runtime sized to the machine's cores, and its steps call the
browser engine directly. When a Chrome crashes, its users' steps fail
with a `browser_crash` error; the next iteration of the first of them
starts it again, and they all go on in the new one.

## The `[load]` settings

The project config's `[load]` section sets what each user's browser
fetches, keeps and may grow to:

```toml
[load]
block = ["third-party"]              # or URL patterns; default: nothing
allow_hosts = ["cdn.example.com"]    # what "third-party" lets through
cache = "new"                        # or "returning"
renderer_memory = "512MB"            # default: no limit
grace = "30s"                        # how long iterations may finish after Ctrl-C
```

### Blocking

`block` lists URL patterns whose requests fail at once in every user's
context, as if the network refused them. A pattern is an absolute URL in
the [URL Pattern](https://urlpattern.spec.whatwg.org/) syntax, with `*`
for any part: `*://*.doubleclick.net/*`, `*://*:*/*.woff2`,
`https://chat.example.com/*`. Chrome checks them itself, so the requests
that go on are not slowed down, and the HTTP cache keeps working. (This
differs from `route` and `block` on the command line, whose patterns
pause every request for chrome-cdp to decide.)

`"third-party"` is a preset: every request to a host other than the
backend's `base_url` origin and the `allow_hosts` (`cdn.example.com`, or
`*.example.com` for its subdomains, on any port) fails. Your own patterns
come first, so they can block same-origin requests too.

The default blocks nothing, so the site gets the load real users give
it. Blocking the site's own images or fonts changes that load: the
server sees fewer requests than it would. The preset is for third-party
analytics, ads and chat widgets, which are someone else's servers, and
which a load test should not flood.

### Caches

`cache = "new"` (the default) gives each iteration a fresh browser
context, so every iteration downloads the app as a first visit does.
`cache = "returning"` gives each user one context for the whole run: its
cookies, storage and HTTP cache stay from one iteration to the next, as
for someone coming back, and the app's cacheable files are fetched once
per user. Storage is only cleared when the journey says so (`cookies
clear`, `storage clear`). A returning user starts a fresh context when
its Chrome was started again, when its page outgrew `renderer_memory`,
or when its next journey has no `@device` or `@viewport` and the last one
had. Under `[arrivals]`, where each user does one iteration, an arriving
user takes over a context an earlier user of its Chrome left, so the
site sees returning visitors. Reusing a context also saves making one
per iteration; the summary says how many contexts were made and how long
one took.

### Memory limits

`renderer_memory` caps each user's page: when its heap (V8's and
Blink's, with array buffers) grows past it, the user's iteration fails
with a `memory` error, and the user goes on in a fresh context. It is
read between steps, at most once a second per user. It catches an app
that leaks as the journey loops before the leak takes the machine down.

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
  `timeout` or `error`) with the error's kind. The runner's own lines
  have a `hook` of `setup` (a context it could not make) or `memory` (a
  page above `renderer_memory`).
- `iterations.jsonl`, with `--raw`: one line per iteration, with the
  user, the iteration, the journey, when it was due, started and ended,
  and whether it failed.

The summary says how many iterations ran and failed per journey and how
long they took, the arrivals and dropped ones, the steps by result, the
errors by kind (`browser_crash` and `memory` among them), the browser
contexts made and how long one took, how many times a Chrome was started
again, and why the run stopped early, if it did. `--json` gives the same
as data (`chrome-cdp schema load`). The run exits 0 when it ran, and with
an error when every iteration failed (the journey is broken, not slow)
or `on_failure = "stop-run"` ended it.

Each run also keeps its journeys' mean iteration length, by backend, in
`e2e/.results/load/journeys.json`, for the next `--plan`.

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
