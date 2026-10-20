# Plan 23 — Load testing with real browsers

Status: DONE (23.1 to 23.5), except the 200-user target on a 16-core
Linux runner, which was not at hand (measured on a 16-core Mac instead)
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

- [x] `chrome-cdp load <profile>` runs open- and closed-model profiles
      against a real backend, and refuses `--backend mock`.
- [ ] 200 concurrent users on the reference journey on a 16-core /
      32 GB Linux runner without saturating (23.1), and at least 1.5×
      23.1's number after 23.2 (`xtask bench load`). Not run on a Linux
      runner. On the 16-core Mac: 200 users without saturating by 23.5's
      check, 4× 23.1's 50.
- [x] chrome-cdp's own process stays under 1 core and 500 MB at 23.1's
      target (0.18 cores and 50 MB at 200 users, on the Mac).
- [x] Against the load server with `--latency /api/cart=200ms`, the
      reported p50 for `POST /api/cart` is within 10% of 200 ms, and
      `--errors /api/checkout=2%` is reported as 2% ± 0.5% over
      10,000 requests.
- [x] Under `[arrivals]`, a server slowed to 5 s per response shows
      the slowdown in iteration latency and `dropped` counts, not a
      lower arrival rate.
- [x] `think` is a no-op under `test`; every scenario in `testdata/`
      and e2e-shop runs under `test` exactly as before.
- [x] A threshold that fails gives exit code `threshold`; a saturated
      run with `on_saturation = "abort"` gives `saturated`.
- [x] A run where the generator is forced to saturate (a CPU-burning
      init script) is flagged within 10 s, and its thresholds leave
      out the saturated seconds.
- [x] `report.html` opens offline and shows the over-time charts and
      the tables.
- [x] `cargo xtask release check` passes: the CLI and schemas only grew
      (with no release tag yet, it has no earlier schemas to compare).

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
- 23.1: until 23.3's profiles, `load PATH… [--grep] [--tags]` takes its
  journeys as `test` takes tests, and runs a closed model: `--users N`
  users, each taking the journeys in turn, until `--duration` (after
  `--ramp-up`) or `--iterations`. 23.3 keeps these flags for a run
  without a profile.
- 23.1: the engine needed no daemon-free constructor: `Engine::new`
  holds no session state of the daemon's, and the history hook lives
  in the daemon's dispatch, not the engine. What a load step must not do
  is a task-local, `engine::in_lean`, like plan 22's `in_settle`: an
  action under it waits as usual and builds no report (no focus marks,
  no console errors, issues or failed requests).
- 23.1: a load step's engine request is built by the same functions the
  CLI commands use (`session::*_request`, `http::request`, `action`),
  split out of the commands, so `load` and the CLI cannot drift. A step
  whose words do not change between iterations (no `${test.id}` or
  `${saved.…}`) gets its request once, before the run; the others are
  parsed again in each iteration.
- 23.1: the commands a load step can be are a list (navigation, the
  input actions, plain `expect`, `wait`, `http`, `eval`, `cookies`,
  `storage`, `think`); everything else is an error at plan time, with
  a reason for the interactive ones. `expect --layout` and `--audit`
  are not load steps either.
- 23.1: think time does not count towards an iteration's `@timeout`, so
  a journey's timeout is the same under `test` and `load`.
- 23.1: a load run exits 0 when it ran, whatever failed (23.4's
  thresholds are the gate), except when every iteration failed: then
  the journey is broken, and the numbers mean nothing.
- 23.1: `${test.id}` of an iteration is `<run.id>-u<user>-<iteration>`;
  `@before-all` and `@after-all` run once in a context of their own in
  the first Chrome, before the users start and after they end.
- 23.1: the runtime's threads get 8 MB stacks: page commands nest deep
  futures, which the daemon runs on its main thread's stack, and a debug
  build overflowed tokio's 2 MB.
- 23.1: building this found that `console` and `network` with
  `--context` and no `--tab` read the browser's active tab, not the
  context's; fixed (`Engine::log_tab`).
- 23.1: `bench load` sums Chrome's memory as PSS on Linux and RSS
  elsewhere; RSS counts Chrome's shared framework once per process and
  reads several times too high on macOS. Users per machine is the most
  users of a run that held (under 1% failed, under 90% of the CPUs,
  step p95 within twice the smallest run's) until 23.5's capacity check.
- 23.1: one Chrome's connection (or its one browser process; the bench
  does not tell them apart) gets in the way past 10 to 25 users. At 50
  users, the step p95 was 201 ms with 10 users per Chrome, 448 ms with
  25 and 1354 ms with 50 (`bench load`, macOS, 16 CPUs). The default
  `--users-per-browser` is 10, and a run with more than 25 users in a
  Chrome warns. It is not an error: `bench load` keeps measuring 50.
- 23.1: on that Mac, chrome-cdp itself used at most 0.1 cores and 54 MB
  (400 users), far inside the target, and users per machine (the
  stand-in rule) was 50. From 100 users on, the Chromes held at about 8
  of 16 CPUs while steps slowed: macOS's own services (WindowServer,
  kernel_task, WindowManager, fontd) took several cores for the headless
  Chromes, and free memory ran out and the machine swapped. `bench load`'s
  "All CPUs" counts only chrome-cdp and its Chromes, so 23.5 must watch
  the whole machine's CPU and memory, as it plans. The 200-user target is
  for a Linux runner, which was not at hand.
- 23.1: 10 users in one Chrome ran 24 renderer processes, not 10. A
  likely cause is that a context's tab starts on `about:blank` and the
  first `goto` moves it to a renderer of the site's; another is the
  closed contexts' renderers outliving them. 23.2 should find out, and
  try making the context with its first URL.
- 23.2: chrome-headless-shell is most of the gain. On the same Mac
  (`bench load`, 10 users per Chrome, 60 s), a user cost 148 MB of RSS
  and 2.2% of a core in the shell against 442 MB and 8% in full Chrome,
  and a step's p50 was 5 ms against 105 ms. Users per machine went from
  50 (23.1) to 200 with the shell (4x, past the 1.5x target), while full
  Chrome with 23.2's flags stayed at 50. 400 users in the shell ran
  without a failure; their step p95 was 471 ms in one run (more than
  twice the smallest run's, so not counted) and 279 ms in the final run
  (counted: 400), so 400 is the edge on this Mac.
- 23.2: each candidate flag was measured by leaving it out of the set
  (50 users, 10 per Chrome, full Chrome; the shell showed no difference
  for any of them). `--disable-gpu` cost about 100 MB and 4% of a core
  per user, `--disable-software-rasterizer`, `--mute-audio`,
  `--disable-extensions`, `--disable-domain-reliability` and turning
  off `BackForwardCache` changed nothing, so all of them are out. Kept:
  `--disable-component-extensions-with-background-pages` (about 20 MB a
  user) and turning off `SpareRendererForSitePerProcess` (about 10 MB and
  0.5% of a core a user). The flag set together saved 24 MB and 0.8% of
  a core per user in full Chrome. The reference journey is a single-page
  app, so the back-forward cache could not show a difference; it stays
  on, as turning it off also changes what a back navigation asks the
  server for.
- 23.2: `--window-size=1280,720` makes the window each page's viewport
  instead of emulating one per tab; it measured the same as 23.1's
  emulated 1280x800 and saves an `Emulation.setDeviceMetricsOverride`
  per context. A journey's `@device` or `@viewport` still sets its own.
- 23.2: `[load] block` patterns are Chrome's URL Pattern syntax, not
  `route`'s: Chrome fails them itself with `Network.setBlockedURLs`, in
  every page, frame and worker, set up before the target runs. `route`
  pauses every request of the browser for chrome-cdp to decide over the
  one CDP connection per Chrome that 23.1 found to be the bottleneck,
  and it turns the HTTP cache off, which would undo `cache =
  "returning"`. `third-party` becomes an allow pattern for the backend's
  origin and each of `allow_hosts`, then a block of every other URL; a
  team's own patterns go first.
- 23.2: `cache = "returning"` keeps a user's context between iterations
  (a fresh one after a Chrome restart, a `memory` error, or a journey
  that emulates nothing after one that did). On the reference journey
  with an `@before` of `cookies clear` (its cart would otherwise grow),
  making a context took 45 to 85 ms per iteration with the shell at 25
  to 200 users, and 690 ms at 400 users; returning users made one each
  and used about half the CPU per user (1.2% of a core), and 400 of them
  held.
- 23.2: a load run's Chromes keep few response bodies (1 MB per page,
  not 64 MB, and no durable copies) and its tabs keep 300 console
  entries and requests: a returning user's tab lives for the whole run,
  and its users never read a body. Before that, chrome-cdp's own RSS
  grew to 314 MB at 400 returning users.
- 23.2: `renderer_memory` is checked against the page's heap as
  `Runtime.getHeapUsage` reports it (V8's, Blink's, array buffers),
  between steps and at most once a second per user, not the renderer
  process's RSS: CDP does not tell which renderer process a context's
  page runs in. It is the memory a leaking app grows.
- 23.2: the shell also held 200 users with 25 users per Chrome, at 11%
  less memory per user (132 MB) and a lower p95 at 400 users (317 ms).
  The default stays at 10 per Chrome until the shell is measured at 50
  per Chrome and on the Linux runner.
- 23.2: 23.1's 24 renderers for 10 users: 10 users in one Chrome ran up
  to 14 renderer processes in the shell and 37 in full Chrome, and the
  same with `cache = "returning"`, whose contexts are never closed, so
  it is not closed contexts' renderers outliving them. The shell's 1.4
  per user is what the default runs; why full Chrome runs three or more
  per context was not found (their command lines do not differ), and
  making the context with its first URL was not tried.
- 23.2: chrome-cdp's own memory grows with a returning user's tab: 40
  returning users held 34 MB after 20 s, 73 MB after 120 s and 98 MB
  after 240 s (fresh contexts each iteration stay flat), still growing
  more slowly. It is not the console, network or issue logs (capped, and
  turning issues off changed nothing). At 400 returning users for 60 s it
  was 315 MB, inside 23.1's 500 MB, but a long run needs it found.
- 23.3: TOML puts a key written after a table's header in that table, and
  the plan's own example has `think` after `[arrivals]`. A `think` in
  `[arrivals]` or `[users]` is taken as the profile's (an error when the
  top has one too), and a top-level key inside `[[journeys]]` is an
  unknown field whose hint says where it belongs.
- 23.3: an open model's k-th arrival (from 0) is due when the stages'
  rate, summed from the start, reaches k + ½: at 5/s the arrivals are at
  0.1 s, 0.3 s, …, a stage at a rate of 0 brings none, and the stages'
  arrivals add up to their rate × duration. The schedule is even, not
  random (Poisson): a run is repeatable, and a burst is a stage.
- 23.3: a closed model's user v runs while the stages' count is above
  v − 1, and starts when a rising count reaches v − 1: the ramp of a run
  without a profile starts user v at `ramp_up × (v − 1) / users`, as
  23.1's did. A user below the count finishes its iteration and waits
  for the count to rise again, or ends.
- 23.3: without a profile, users take the journeys in turn, as in 23.1;
  a profile's are picked by weight, at random from a seed per user.
- 23.3: `@before-all` and `@after-all` still run before the first user
  starts and after the last one ends. Every journey of a profile has a
  weight above 0, so each of their files' hooks is needed; running them
  lazily, at the first iteration of a file's journey, would only move
  the setup into the measured time.
- 23.3: an open model's Chromes are enough for `max_users`; those for the
  users it expects at once (the peak rate × the journeys' mean length in
  the last run on the backend, from `e2e/.results/load/journeys.json`,
  and a quarter more) start before the users, the others when an
  arrival first needs one; without a last run, all of them. An arrival
  takes a free place in the first Chrome that has one. Under `cache =
  "returning"` an arrival takes over a context an earlier user of its
  Chrome left, so an open model has returning visitors too.
- 23.3: `[load] grace` is in the config, with the other settings of how
  the machine runs users; the profile says what load to make. A second
  Ctrl-C aborts the users' tasks: their iterations are counted as cut
  short and nothing else, and `@after-all` does not run. With web
  servers, the test runner's server watch runs as for `test --watch`, so
  a server outlives Ctrl-C until the report is written.
- 23.3: a run that `on_failure = "stop-run"` ends exits 1 (`ExpectFailed`,
  the run's data as details): the site is down. One that Ctrl-C or
  `data_exhausted = "stop"` ends exits 0, saying why in `stopped`.
- 23.3: until 23.4's histograms, each iteration's time from its due time
  is kept in memory as one number (8 bytes an iteration) for the
  summary's p50, p95 and max.
- 23.3: under `test`, `${vu.id}` is `<run>-u1` and `${iteration.id}` the
  test's id, so a journey that names its user runs as a test too;
  `${row.…}` has no meaning there and is an error. `lint` and a load
  run's test plan take any `${row.…}`; the load plan checks it against
  each journey's data (and a file's `@before` and `@after` against the
  columns all its journeys' data have).
- 23.3: `think` never reaches the daemon (it runs in the CLI, as a
  no-op), so `history --as-scenario` never has it.
- 23.3: `load --plan` does not look for a Chrome, only works out which
  kind the run would start, so it runs where no browser is installed.
- 23.3: measured (`docs/benchmarks.md`, the same Mac): `bench load`'s
  closed runs did not move from 23.2's, and an open model at 29 arrivals
  a second (193 users at once) started each iteration 0.8 ms after it
  was due at p50 and 3.9 ms at most, with no arrival dropped.
- 23.4: the histograms are `hdrhistogram` 7 without its default features
  (no serialization, so no new crates past `byteorder` and `num-traits`;
  MIT or Apache-2.0, which `deny.toml` allows), at 2 significant figures
  (1%), recording µs and resizing to the largest value seen: about 18 KB
  a histogram up to 10 s. A percentile reads the middle of its value's
  bucket, not its top. Merging runs across machines would need the
  serialization feature, or `summary.json` to carry the histograms.
- 23.4: a request counts when it ends (`loadingFinished` or
  `loadingFailed`), with its time from start to end and its phases from
  `Network.responseReceived`. A status of 400 or more, or no response, is
  an error; a request the page canceled or `[load] block` failed is not
  counted; one a cache answered is counted but not timed, as the server
  never saw it. Chrome ends a `fetch` only once the page reads its body,
  so a fetch whose body the page never reads is never counted.
- 23.4: the engine follows a load run's requests for the metrics on its
  own (`engine::load::Inflight`), not through a tab's network log: that
  keeps a load run's last 300 requests, and a page with 500 fetches in
  flight lost the oldest before they ended (the error-rate test found
  it).
- 23.4: Web Vitals come from `perf`'s observer, which every top-level
  document already gets in chrome-cdp's world. `vitals.js`, also there in
  a load run's pages, reports a page view once: on `pagehide` through a
  binding in that world, or when the run reads it at the end of an
  iteration (`Engine::page_vitals`, one evaluate), which covers a page a
  context closes without a `pagehide`. A page that is not http(s) (the
  context's first `about:blank`) is no page view. INP needs an
  interaction, so a page view without one has none.
- 23.4: an endpoint keeps its host when it is not the backend's origin
  (`GET cdn.example.com/lib.js`), so third parties do not mix with the
  site; `[load] endpoints` patterns only match the backend's origin. A
  long hex segment is 16 hex digits or more with a digit among them, so
  words like `deadbeef` stay. Past 500 groups, the warning names the most
  common group of the overflow with its last segment as `:name`.
- 23.4: `summary.json` is what `--json` prints (`LoadData`, grown with
  `requests`, `endpoints`, `step_times`, `vitals`, `request_errors`,
  `exceptions`, `thresholds`, `files`, `compare` and `metrics_listen`;
  `iteration_ms` gained p90 and p99), so a script reads one schema. 23.3's
  in-memory list of iteration times is gone: journeys' times come from
  the histograms.
- 23.4: the status line is printed in text mode only, from the last 10
  seconds; it leaves out the CPU, which 23.5 watches.
- 23.4: a threshold's value is its limit, `"< 300ms"`, or a table `{ limit
  = "< 300ms", abort = true }`. Every group but an endpoint is checked at
  plan time; an endpoint can only be known from requests, so one no
  request had fails at the end with the closest endpoints that some had.
  A time with no value fails ("no value"): a gate that cannot measure
  does not pass. Thresholds are checked each second over the run so far
  (one that starts failing is said), and `abort` stops the run with
  reason `threshold` after 30 s of failing; that threshold then fails
  whatever the whole run's value. `ThresholdFailed` is exit code 6, after
  `DialogOpen`; 23.5's `saturated` takes the next. In text mode the run's
  summary is printed before the error, as the run did run.
- 23.4: syntax problems of `[thresholds]` are the profile's, reported with
  its other problems before the scenarios are read; unknown groups are
  reported with the journeys' problems, as both need the scenarios.
- 23.4: `--compare` takes a run's id, `last` (the newest run with a
  `summary.json`) or a path, and is read before the users start. It
  compares every number both runs have (each group's p50, p95 and p99,
  the requests' error rate and rate, the Web Vitals' p75) and marks a
  change above 10%, but not a time that moved by less than 1 ms: on the
  load server, 1.2 ms against 0.9 ms is noise.
- 23.4: `--metrics-listen` is a small HTTP/1.1 responder on the run's
  runtime (no new dependency): any `GET` gets the numbers, as summaries
  over the run so far (p50, p90, p95, p99, sum and count) rather than
  histograms, which Prometheus would need bucket bounds for.
- 23.4: `report.html` draws its charts as inline SVG with a few lines of
  script for the hover readout; one series a chart, so no legend and no
  second axis. A second without requests is a gap in the p95 line.
- 23.4: the load server has no `/api/checkout` (the shop's checkout posts
  to its payment provider), so the error-rate criterion is tested with
  `--errors "GET /api/products=2%"`: 20 iterations of 501 requests, which
  read 2% ± 0.5%. `POST /api/cart` at 200 ms read a p50 within 10%.
- 23.4: measured (`docs/benchmarks.md`, the same Mac): with the metrics,
  chrome-cdp used 0.08 cores and 35 MB at 100 users and 0.18 cores and
  50 MB at 200 (23.3: 0.07 and 0.16 cores, 36 and 49 MB), with the same
  iterations and step p95: users per machine did not move.
- 23.5: the machine is read from `/proc` on Linux. On macOS it is one
  `iostat -w 1` for the run (CPU use and load average every second), and
  each second `sysctl kern.memorystatus_level` (the free memory the
  system counts) and one `ps` (every process's parent, RSS and CPU time),
  about 13 ms each: the workspace denies `unsafe` outside two modules, so
  no mach or sysctl calls. Elsewhere only the timers' lag is watched, and
  `capacity.machine` says so.
- 23.5: the reads run in a task of their own, and each second takes the
  latest. Read inline, with the machine at 100% CPU, starting `ps` took
  over a second, most seconds went without a CPU reading, and the 5 s in
  a row never came. A Chrome's CPU in a second is then rough (`ps` itself
  is starved): it is timed at the middle of the read and capped at the
  machine's CPUs.
- 23.5: each Chrome is its browser process's tree (`Engine::status`'s
  pid), not every process under chrome-cdp, which has the web servers
  too. `Performance.getMetrics` per target was not used: the process tree
  has each Chrome's CPU and memory, and a call per page each second would
  load the CDP connection 23.1 found to be the bottleneck.
- 23.5: the scheduling lag is how late the run's timers fire: a closed
  model's user waiting for its start, every `think`, an open model's
  arrivals, and the run's own second. Each second's p99 is the limit's.
- 23.5: the lag inside the page is a timer every 100 ms, not a
  `setTimeout(0)` loop, which Chrome clamps to 4 ms and which would wake
  250 times a second in every page, spending the CPU it measures. A page
  view reports its timers' p99 and most with its Web Vitals. It is said,
  not a limit: a page whose own code is heavy lags on any machine.
- 23.5: a second turns out to be saturated only when its limit has held
  for 5 s, 4 s later. Each group keeps the seconds of a run of crossed
  limits (`over`) and, once it trips, the saturated ones (`left_out`),
  both made only when a limit is first crossed; a threshold reads the
  whole run without `left_out` (HDR histograms subtract). `metrics.jsonl`
  is written as the run goes: its run line says `saturated` from the
  second it trips, and that second's capacity line has `saturated_from`;
  the charts mark the earlier seconds too. Saturation ends with the first
  second within the limits.
- 23.5: `[load.capacity]` and `on_saturation` are in the config, with the
  other settings of how the machine runs users; the profile says what
  load to make. A threshold whose group had no value outside the
  saturated seconds fails, saying so, like any threshold with no value.
- 23.5: what the machine held is the most users at once before the first
  saturated second, and per Chrome over the Chromes started then; a run
  that never saturated held its peak. The hint lowers an open model's
  peak rate (or a closed model's users) to 90% of what held, then names
  chrome-headless-shell (when the run was in full Chrome), `[load] block
  = ["third-party"]` (when not set) and a bigger machine.
- 23.5: `--calibrate` needs a profile, and ramps a closed model from one
  user at 2 users a second up to twice the profile's most users (or
  `--users`, over `--duration`) on its journeys by weight, whatever the
  profile's model, as an open model's users at once depend on the site.
  It stops at the first of the generator saturating and the site failing
  (5 s in a row of more than 5% of at least 10 requests, or 10% of the
  iterations, failed; a second with too little to judge neither ends nor
  adds to the run, as the full test suite starved some), checks no thresholds, and exits 0. It is kept by
  `<profile> on <backend>`; a later run warns before it starts when its
  users at once go above 80% of what the machine held, only when the
  generator gave first (when the site did, the machine's limit is not
  known).
- 23.5: the integration tests run beside each other, and the machine is
  often above 90% CPU while they do, so a test project gets limits that
  never trip unless the test is about them. The forced saturation is an
  `eval` that starts a busy loop in each page (a load step cannot add an
  init script), with users for a quarter of the machine's CPUs and a CPU
  limit below their share, so the rest of the suite keeps its CPUs.
- 23.5: `bench load`'s runs hold when under 1% of their iterations failed
  and the capacity check never found the generator saturated, which
  replaces 23.1's stand-in rule.
- 23.5: measured (`docs/benchmarks.md`, the same Mac, chrome-headless-shell,
  a Chrome per 10 users): users per machine by the capacity check is 200,
  not the stand-in's 400. At 400 users chrome-cdp and its Chromes used 9.4
  of 16 CPUs, but the machine was at 100% and the generator saturated for
  61 s, by its CPU; at 200 the machine peaked at 65%. chrome-cdp used 0.18
  cores and 50 MB at 200 users and 0.08 cores and 36 MB at 100, as in
  23.4: the watch costs nothing measurable. A run whose pages burn the CPU
  was flagged at its 6th second.
