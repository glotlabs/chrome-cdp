# Plan 23 — Load testing with real browsers

Status: NOT STARTED
Depends on: 03 §3.4 (the wait engine and settle), 06 §6.2 (each tab's
network log), 07 §7.3 (init scripts), 11 (scenarios, the runner,
`--jobs`, real backends, `${…}` variables), 22 (the test settle, one
client per worker, the scale measurements)

## Goal

A team wants to know how their site holds up when many people use it
at once, and what those people see while it does. An LLM writes the
scenarios before the run, as `.cdp` files like any other test. Then
`chrome-cdp load` runs hundreds of real-browser users through them
against a real backend, at a rate the team picks, and reports what the
server did (request latency and errors per endpoint) and what users
saw (step times, Web Vitals, failed expectations). It fails the run
when a threshold is crossed, so it can gate a deploy.

What makes chrome-cdp a good fit is that each user is a real browser
running the app's real JavaScript. That makes each user expensive, so
this plan is mostly about fitting as many users as possible on one
machine, and about noticing when the machine running them is the
bottleneck instead of the site.

`chrome-cdp test --jobs N` can't do this today:

- **A thread per job.** `exec.rs` spawns one OS thread per worker
  (`scope.spawn`), and every step goes through `crate::run` to the
  daemon like a CLI command. Plan 22 cuts the cost per call, but a
  thread and a daemon client per user doesn't reach hundreds of users.
- **Test work after every step.** The mock-log check, fingerprints,
  coverage, the no-errors check and log collection are right for
  tests and wasted in a load run.
- **No load model.** There's no rate, ramp, think time, journey mix
  or per-user data. A test runs once; a load user loops.
- **No load metrics.** The runner keeps pass/fail and step durations,
  not latency distributions per endpoint, error rates over time, or
  thresholds.
- **No sense of its own limits.** When Chrome saturates the CPU, step
  times go up and nothing says the cause is the client, not the server.

## Scope

In: one machine running many browser users (23.1, 23.2), the load
model (23.3), metrics, reports and thresholds (23.4), and watching the
load generator's own capacity (23.5).

Out (see "Not in this plan"): spreading load across machines,
protocol-level replay, and checks aimed at LLM-written scenarios
beyond what `test` and `lint` already do.

## Rules for every milestone

- Plan 12 §7 applies: the CLI and `--json` schemas only grow, every
  new flag and config key has help with an example, a `docs/` page and
  a golden schema.
- **`test` doesn't change.** Scenarios keep one meaning. A step added
  for load (`think`) does nothing under `test` and is accepted by
  `lint`. A load run reuses the scenario parser, the variable resolver
  and the backends, not the test runner's per-step checks.
- **Real backends only.** `load` refuses `--backend mock`. Load testing
  the mock server measures chrome-cdp, not the site. A `protected`
  backend needs `--allow-writes` or only `@safe` journeys, as `test`
  does (docs/e2e/backends.md, "Protected backends").
- **The load server fixture.** `testdata/load/server`: a small Rust
  HTTP server that serves e2e-shop's built app and a fake API with
  latency and error rates you can set per endpoint (`--latency
  /api/cart=200ms±50`, `--errors /api/checkout=2%`). Every milestone's
  tests and benchmarks run against it, offline.
- **Measure before and after.** Each milestone records its numbers in
  `docs/benchmarks.md` under `bench: load`. The headline number is
  **users per machine**: the most concurrent users, on a fixed profile
  and journey, before 23.5's capacity check trips.

## Milestone 23.1 — A runtime for many users

Target: 200 concurrent users on one 16-core / 32 GB Linux runner on
the reference journey (e2e-shop browse → product → cart, 2 s think
time), with chrome-cdp itself (not Chrome) under 1 core and 500 MB.

- **`chrome-cdp load` runs the engine in-process.** It doesn't go
  through the daemon: the load process launches its Chromes through
  `browser::launch` and drives them with an `engine::Engine` of its
  own. Steps call the engine directly, skipping the RPC layer, history
  and output formatting. If the engine assumes it lives in a daemon
  (session-scoped state, the history hook in `server.rs`), add a
  constructor that turns those off instead of forking code.
- **Users are tasks, not threads.** One multi-threaded tokio runtime
  sized to the cores, one task per virtual user (VU). A VU owns a
  browser context with one tab, runs a journey, and loops (23.3 says
  how). Scheduling uses `tokio::time`, never `sleep` on a thread.
- **One CDP connection per Chrome.** All of a Chrome's contexts share
  its pipe through flattened target sessions (`cdp::Session` already
  carries a `sessionId` per target). Measure whether one connection
  gets in the way above some number of contexts. If it does, the
  decisions log records the number and 23.1 caps contexts per Chrome
  below it.
- **Shards: N Chromes × M contexts.** A pool of Chrome processes, each
  holding up to M users' contexts. `--browsers N` and
  `--users-per-browser M` set it; the default picks M from 23.2's
  measurements and N from the users the run needs. A VU's context
  lives in one Chrome for the VU's life. When a Chrome crashes, its
  VUs record a `browser_crash` error, and the pool starts a new
  Chrome and the VUs start their next iteration in it.
- **Load steps are lean.** A step runs its action or expectation and
  its wait (plan 22's test settle, never the interactive one). It
  doesn't build an action report, take a snapshot, check the mock
  log, compute fingerprints, or collect logs. `expect` keeps its
  retries and timeout.
- **What a failed step keeps.** The first `--keep-failures N` (default
  20) failed iterations save `failure.png`, the page's
  `snapshot.txt`, the console and the network log, to
  `e2e/.results/load/<run>/failures/`, the same files `test` writes.
  After N, failures are only counted, so a broken site doesn't fill
  the disk.
- **Which commands work.** Navigation, actions, `expect`, `wait`,
  `http`, `eval`, `cookie`/`storage` and `think` (23.3). Commands that
  only make sense for one interactive user (`snapshot` without a file,
  `screenshot`, `pick`, `debug`, refs like `e12`) are an error at
  plan time, listing every step that uses one, before any browser
  starts.
- **A step's timing.** Every step records start and end on the
  runtime's clock, and its result (`ok`, `failed`, `timeout`, `error`
  with a kind). 23.4 aggregates them; 23.1 writes them raw to
  `steps.jsonl` behind `--raw` so the milestone can be checked before
  the aggregation exists.
- **`xtask bench load`** runs the reference journey at 25, 50, 100,
  200 and 400 users, with one Chrome per 10, 25 and 50 users, and
  writes users per machine, Chrome's RSS and CPU per user, and
  chrome-cdp's own CPU and RSS to `docs/benchmarks.md`.

## Milestone 23.2 — A lean browser

Target: at least 1.5× 23.1's users per machine, with the same request
pattern reaching the server for the app's own origin.

- **chrome-headless-shell by default.** `load` uses it when it is
  installed and tells the user to run `chrome-cdp install
  --headless-shell` when not, falling back to Chrome with a warning.
  `--browser chrome` picks full headless Chrome for apps that need it.
- **A load flag set.** On top of `flags.rs`'s defaults: no GPU and
  software compositing off where Chrome allows it, no audio, a small
  default viewport (1280×720; a journey can set its own), no
  extensions, no background tasks. Each flag is in `flags.rs` under
  `Only::Load` with a one-line reason, as today's are. Measure each
  flag; drop the ones that don't save memory or CPU.
- **Blocking only what isn't load.** `[load] block` takes URL
  patterns (the `route` pattern syntax) whose requests fail fast in
  every VU's context. `block = ["third-party"]` is a preset: every
  request not to the backend's `base_url` origin or the hosts in
  `[load] allow_hosts`. The default blocks nothing. The docs say that
  blocking same-origin images or fonts changes the load the site
  gets, and that the presets are for third-party analytics, ads and
  chat widgets, which are someone else's servers.
- **Caches that match real users.** `[load] cache = "new" | "returning"`.
  `new` (the default) gives each iteration a fresh context, so every
  iteration downloads the app like a first visit. `returning` keeps a
  VU's context, cookies and HTTP cache across its iterations, and
  clears storage only when the journey says so. Context reuse also
  saves the context setup per iteration; record how much.
- **Memory limits per Chrome.** A renderer above `[load]
  renderer_memory` (default unset) is a `memory` error for its VU,
  which closes the context and starts a new one. It catches a leaking
  app instead of letting it take the machine down.
- **Measure.** Rerun `xtask bench load` with each change, and record
  the new users per machine and memory per user.

## Milestone 23.3 — The load model

A load run is described by a profile, not by flags alone, so the LLM
that writes the scenarios can write the profile too, and it can be
committed and reviewed.

- **Profiles.** `e2e/load/<name>.toml`, run with `chrome-cdp load
  <name>` (or a path). `chrome-cdp init load` writes an example. A
  profile has:

  ```toml
  backend = "staging"                 # a real backend from chrome-cdp.toml

  [[journeys]]
  test = "e2e/scenarios/shop.cdp:browse a category"   # file:test name
  weight = 70
  [[journeys]]
  test = "e2e/scenarios/shop.cdp:search"
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

  think = "2s..8s"                    # the default think time, uniform; "3s" fixed; "exp(4s)" exponential
  ```

- **Two models.** `[arrivals]` (open: users arrive at a rate whether
  or not earlier ones finished, as on a real site) and `[users]`
  (closed: `count = 50` users, each looping its journeys, with
  `stages` ramping the count). Exactly one of the two. The docs
  explain why open is the default: under a closed model, a slow
  server slows the arrivals too and hides how bad it is.
- **One iteration.** A user picks a journey by weight, gets a fresh
  context (or keeps its own under `cache = "returning"`), runs the
  test's `@before` steps, its steps and its `@after` steps, and
  closes the context. Under `[arrivals]`, a user does one iteration;
  under `[users]`, it loops. A file's `@before-all` and `@after-all`
  run once, before the first and after the last iteration that needs
  them, as in `test`.
- **`think`.** A new step: `think 3s`, `think 2s..8s`, `think` (the
  profile's default). It is a no-op under `test` and in `history
  --as-scenario`, so the same scenario runs as a test at full speed.
  A profile can also set `think_between_steps` to add the default
  think time after every action that a scenario didn't write out.
- **Per-user data.** A journey's `data` is a CSV (headers become
  names) or a JSON array of objects. Each iteration takes the next
  row, as `${row.email}`; `data_order = "sequential" | "random"`,
  and `data_exhausted = "wrap" | "stop"` (stop ends the run with a
  message, for single-use accounts). `${vu.id}` and `${iteration.id}`
  are unique per user and per iteration, like `${test.id}`.
  Everything plan 11's resolver supports (`${var:…}`, `${secret:…}`,
  `${saved.…}`) works as before.
- **When a step fails.** The iteration stops, records the error, runs
  `@after`, and the user starts its next iteration (closed) or ends
  (open). `on_failure = "stop-run"` with `max_failures = 100` ends the
  whole run early instead, for a site that is clearly down.
- **Timing that doesn't lie.** Under `[arrivals]`, an iteration's
  latency counts from when it was due to start, not from when a free
  slot let it start. When `max_users` is reached, the arrival is
  dropped and counted as `dropped`, never queued silently.
- **Before anything starts.** `load` resolves the profile, the
  journeys, the data files and every step's variables, and fails with
  every problem at once, as `test` does at plan time. `load --plan`
  prints what would run: the stages, the expected peak users
  (rate × the journey's length from a previous run, if there is one),
  and the Chromes it would start.
- **Duration and stopping.** Ctrl-C (or SIGTERM) stops new arrivals,
  lets running iterations finish for up to `[load] grace` (30 s), then
  writes the report. A second Ctrl-C stops at once and still writes
  the report for what ran.

## Milestone 23.4 — Metrics, reports and thresholds

- **What is measured.**
  - **Requests**, from each context's Network events (the engine's
    network tracker): method, URL, status, and the timing from
    `Network.responseReceived`'s `timing` (DNS, connect, TLS, time to
    first byte, download), whether it came from the cache, and failed
    requests by error text.
  - **Steps**, from 23.1's records: duration and result per step, by
    journey and the step's line in the scenario.
  - **Iterations**: duration and result per journey.
  - **What users saw**: LCP, CLS and INP per page view, from an init
    script (`inject`, plan 07) with `PerformanceObserver`, read when
    the page is left or the iteration ends. TTFB and the load event
    come from navigation timing.
  - **Errors** by kind: HTTP 4xx and 5xx (per endpoint), request
    failures, timeouts, failed `expect`s, uncaught exceptions, and
    23.1/23.2's `browser_crash` and `memory`.
- **Endpoints, not URLs.** Requests are grouped by method and a path
  pattern: numeric, UUID and long hex segments become `:id`
  (`GET /api/products/:id`), and query strings are dropped. `[load]
  endpoints = ["GET /api/search?q=*", "/blog/:slug"]` adds patterns
  that win over the automatic ones. Static assets group by type
  (`static js`, `static css`, `static img`) unless `group_assets =
  false`. Above 500 groups, the rest go into `other` with a warning
  naming the pattern most likely missing.
- **Histograms.** Every duration goes into an HDR histogram (the
  `hdrhistogram` crate; check its license with `cargo deny`), per
  group, per 1 s window and for the whole run. Memory stays bounded
  by groups × windows, not by the number of requests.
- **While it runs.** One status line on stderr, rewritten every
  second when stderr is a terminal and printed every 10 s otherwise:
  `03:12 stage 2/3 · 412 users · 20.1/s arrivals · req p95 184 ms ·
  errors 0.4% · cpu 61%`. A threshold that starts failing is printed
  when it does.
- **Outputs**, in `e2e/.results/load/<run>/`:
  - `summary.json` (`--json` prints it): per endpoint, step, journey
    and Web Vital, the count, rate, error rate, and p50, p90, p95, p99
    and max, plus thresholds and capacity (23.5).
  - `metrics.jsonl`: one line per second per group, for plotting.
  - `report.md` (plan 11's report style) and `report.html`: one
    self-contained page with charts over time (arrivals, users,
    request p95 and error rate) and the tables of `summary.json`. No
    network access needed to open it.
  - The failures 23.1 kept.
- **Thresholds.** In the profile:

  ```toml
  [thresholds]
  "p95(GET /api/products/:id)" = "< 300ms"
  "p99(step checkout.cdp:14)"  = "< 2s"
  "error_rate"                 = "< 1%"
  "p75(lcp)"                   = "< 2.5s"
  "rate(dropped)"              = "== 0"
  ```

  A threshold names a statistic of a group; an unknown group is an
  error at plan time with the closest names. `abort = true` on a
  threshold stops the run as soon as it fails for 30 s in a row. The
  exit code is 0 when every threshold passed, and a new
  `threshold` exit code otherwise, added to plan 08's table.
- **Comparing runs.** `chrome-cdp load --compare <run|path>` puts a
  previous run's numbers next to this one's in the report and marks
  changes above 10%.
- **Prometheus.** `--metrics-listen 127.0.0.1:9464` serves the live
  numbers in Prometheus' text format for teams that already graph
  their servers there. It only listens on the address given, and only
  while the run lasts.

## Milestone 23.5 — Knowing its own limits

A browser load generator usually runs out of CPU before the site does.
When it does, every number goes up, and a report that blames the
server is wrong. This milestone makes chrome-cdp notice and say so.

- **What is watched**, every second: the machine's CPU use and load
  average, free memory, each Chrome's CPU and RSS (from the process
  tree; `Performance.getMetrics` per target where it is cheap),
  chrome-cdp's own CPU and RSS, and the **scheduling lag**: how late
  VU timers and planned arrivals fire, measured against when they
  were due.
- **Lag inside the page.** A tiny init script measures how late a
  `setTimeout(0)` loop runs in each page. A page whose main thread
  lags while the server's TTFB is flat means Chrome is starved, not
  the server.
- **Saturated or not.** The generator counts as saturated when, for
  5 s in a row, CPU is above 90%, free memory is below 10%, or the
  scheduling lag p99 is above 50 ms. Each limit is a `[load.capacity]`
  key. While saturated:
  - the status line says `CLIENT SATURATED (cpu 97%)`;
  - `metrics.jsonl` and the charts mark those seconds;
  - thresholds are evaluated without those seconds, and the report
    says how much was left out;
  - with `on_saturation = "abort"` (the default is `"warn"`), the run
    stops and exits with a new `saturated` exit code, so CI doesn't
    treat a bad measurement as a good or a bad build.
- **Saying what to do.** The summary gives the peak users and the
  limit that tripped, and how many users per browser and in total this
  machine handled on this journey without saturating. The hint gives
  the next command: lower the rate, use `block = ["third-party"]`, or
  run the profile on a bigger machine.
- **Finding the limit.** `chrome-cdp load <name> --calibrate` ramps
  users on the profile's journeys, against the profile's backend,
  until the generator saturates or the server starts failing, and
  reports which came first and at how many users. Its result is saved
  in `e2e/.results/load/calibration.json`, and later runs warn when
  their peak users is above 80% of it before they start.

## Not in this plan

- **Load across machines.** A controller handing out arrival rates to
  agents and merging their histograms. 23.4's HDR histograms and the
  `summary.json` format are chosen so they can be merged later.
- **Protocol-level replay.** Recording a journey's HTTP traffic once
  and replaying it without a browser for the bulk of the load, with
  automatic correlation of tokens and IDs.
- **Checks aimed at LLM-written scenarios** beyond today's `lint` and
  23.3's plan-time checks: a one-user dry run, a target allowlist, and
  making sure each journey makes its own account.
- **Mobile throttling (CPU and network) per user.** Emulation exists
  (plan 07), but throttled browsers cost more of the client's CPU, so
  it needs 23.5's measurements first.

## Acceptance criteria

- [ ] `chrome-cdp load <profile>` runs open- and closed-model profiles
      against a real backend, and refuses `--backend mock`.
- [ ] 200 concurrent users on the reference journey on a 16-core /
      32 GB Linux runner without saturating (23.1), and at least 1.5×
      23.1's number after 23.2 (`xtask bench load`).
- [ ] chrome-cdp's own process stays under 1 core and 500 MB at 23.1's
      target.
- [ ] Against the load server with `--latency /api/cart=200ms`, the
      reported p50 for `POST /api/cart` is within 10% of 200 ms, and
      `--errors /api/checkout=2%` is reported as 2% ± 0.5% over
      10,000 requests.
- [ ] Under `[arrivals]`, a server slowed to 5 s per response shows
      the slowdown in iteration latency and `dropped` counts, not a
      lower arrival rate.
- [ ] `think` is a no-op under `test`; every scenario in `testdata/`
      and e2e-shop runs under `test` exactly as before.
- [ ] A threshold that fails gives exit code `threshold`; a saturated
      run with `on_saturation = "abort"` gives `saturated`.
- [ ] A run where the generator is forced to saturate (a CPU-burning
      init script) is flagged within 10 s, and its thresholds leave
      out the saturated seconds.
- [ ] `report.html` opens offline and shows the over-time charts and
      the tables.
- [ ] `cargo xtask release check` passes: the CLI and schemas only grew.

## Tests

- Unit: profile parsing and its errors (unknown journey, weights that
  are not positive, both `[arrivals]` and `[users]`), the arrival
  scheduler (rates, ramps, dropped arrivals, timing from the due time),
  weighted journey choice (distribution over 100,000 picks), data
  feeders (`sequential`, `random`, `wrap`, `stop`), think-time
  distributions, endpoint grouping (`:id` rules, overrides, the 500
  cap), threshold parsing and evaluation, saturation detection on
  recorded samples.
- Integration (real Chrome, the load server): 50 users for 60 s on the
  reference journey; a Chrome killed mid-run is replaced and its VUs
  continue; Ctrl-C finishes running iterations and writes the report;
  `cache = "returning"` makes the second iteration's static requests
  come from the cache; `block = ["third-party"]` stops a third-party
  request and nothing else; Web Vitals are reported for every page
  view; `--keep-failures 3` keeps exactly three failures.
- Golden: `load --plan` output, the status line, `summary.json`'s
  schema, `report.md`, the new exit codes and their hints.
- Benchmarks: `xtask bench load` and `--calibrate` against the load
  server.

## Decisions log

- `load` drives the engine in-process instead of through the daemon.
  The daemon exists to keep one browser warm across shell commands
  from an agent; a load run is one long process that owns its
  browsers, and the per-call RPC is the overhead it can least afford.
- Open model by default: real sites get users who arrive whether or
  not the site is slow, and a closed model under-reports how bad a
  slowdown is.
- Blocking is opt-in and same-origin assets load by default, so the
  site gets the load real users give it unless the team decides
  otherwise.
- Saturated seconds are left out of thresholds rather than failing
  them: a saturated generator measures itself, and blaming the site
  for it would make the gate untrustworthy.
