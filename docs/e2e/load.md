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

This page covers milestones 23.1 and 23.2 of the plan
(docs/plans/23-load-testing.md): the runtime, users that loop their
journeys, what a run keeps, and a lean browser (`[load]` settings). Load
profiles (arrival rates, journey weights, per-user data), the metrics
report and thresholds come later.

## Journeys

The paths (and `--grep`, `--tags`) select tests as `test` selects them;
those tests are the users' journeys. Each user runs them in turn, over and
over, until `--duration` is up or it has done `--iterations`; the
iterations that are running then finish. `--ramp-up 1m` starts the users
evenly over a minute instead of all at once.

Each iteration runs as `test` runs the test: in a fresh browser context
with one tab, with the test's `@device` or `@viewport` if it has one, the
file's `@before` steps, the test's own and its `@after` steps (after a
failure too). `@before-all` and `@after-all` run once,
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
had. Reusing a context also saves making one per iteration; the summary
says how many contexts were made and how long one took.

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

The summary says how many iterations ran and failed per journey, the
steps by result, the errors by kind (`browser_crash` and `memory` among
them), the browser contexts made and how long one took, and how many
times a Chrome was started again. `--json` gives the same as data (`chrome-cdp schema
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
