# Plan 22 — Fast suites

Status: IN PROGRESS (22.1, 22.2 and 22.3 done)
Depends on: 05 §5.5 (the post-action report and its settle), 08 §8.2
(`batch`), 10 §10.4 (the benchmarks), 11 (the runner, `--jobs`,
`--shard`), 18 §18.3 (flake diagnosis)

## Goal

A team with thousands of tests picks the runner that gives them a
result first. Right now that isn't us. The runner's shape is right:
one Chrome, a fresh context per test, and workers that take the next
test from a shared counter. But each test is slow, and the runner
doesn't use what it knows about tests to schedule them.

Measured on 2026-10-01 (`tests/bench.rs`, release build, Chrome 154,
4 cores, e2e-shop: 18 tests, 126 steps, `--jobs 1`):

| | Result | Budget (plan 11) |
|---|---|---|
| Whole suite, serial | ~43.5 s | — |
| Time inside steps | 39.6 s, about 315 ms per step | — |
| Runner overhead per test | **224 ms** | < 100 ms (fails) |
| Fingerprints per step | 0.4 ms | < 10 ms |
| `--ui-coverage` per step | ~0 | < 50 ms |

The `--jobs 4` suite time was never measured: the benchmark asserts
the overhead first and stopped there.

Where the time goes, biggest first:

1. **Minimum waits after navigations and actions.** By default `goto`
   waits for the load event, then until the network has been idle for
   500 ms (`IDLE_QUIET`) and the DOM unchanged for 300 ms (`DOM_QUIET`),
   so every `goto` costs at least ~0.8 s on a page that is already done.
   Every click or fill waits for at least 150 ms of quiet
   (`ACTION_QUIET`). A checkout test does three `goto`s: about 2.4 s
   of its 4.6 s. These waits suit an agent that looks at the page after
   each command. In a test, the next `expect` already retries until it
   passes, and actions already wait until their element can be acted on.
2. **Round trips per test.** A seven-step test makes about 30 calls to
   the daemon, and each builds a new tokio runtime, connects a new
   socket and does a hello (`session.rs` `Ctx::with_default`,
   `client.rs` `Client::open`):
   - `context new`, `mock use`, `viewport`, then one call for each
     determinism setting;
   - after every step, `mock log --unmatched --limit 0`, which returns
     the whole log for the client to filter (`exec.rs` `unmatched_in`);
   - after the test, `expect --no-errors`, then `context list`,
     `console` and `network` for each tab, `mock log --all` and
     `context close`, all written to disk, even when the test passed
     (`artifacts::logs`);
   - in the daemon, every kept action also works out its refs' stable
     locators for the session history (`server.rs`), and nothing reads
     a test session's history.
3. **`--jobs` defaults to half the cores** (`e2e::config::Jobs::count`).
   Tests spend most of their time waiting on the page, not on our CPU,
   so the right number is probably higher. Nobody has measured it.
4. **The runner ignores how long tests take.** Tests run in file order,
   and `--shard i/n` splits the list into equal-count blocks
   (`select::shard`). With thousands of tests, a slow test that starts
   last makes the whole run end late, and CI shards end unevenly.

This plan fixes each of these, in that order, and measures each step
on a suite of 1,000+ tests.

## Rules for every milestone

- Plan 12 §7 applies: the CLI and `--json` schemas only grow, every
  new flag and config key has help with an example, a `docs/` page and
  a golden schema.
- **Measure before and after.** Each milestone records its numbers in
  `docs/benchmarks.md` (the `bench:` sections). A change that makes a
  number worse is reverted or justified in the decisions log.
- **No new flakes.** Each milestone passes `cargo xtask e2e --repeat 20`
  and `e2e-shop --repeat 20` with zero failures. Plan 18.3's flake
  fixtures still get the same diagnosis.
- **Failures look the same.** A failed test still gets `report.md`,
  `failure.png`, `snapshot.txt`, the console, the network and the mock
  log. Only passing tests may skip work.
- **Interactive commands don't change.** The faster waits are for test
  runs; `goto` and `click` from the shell keep today's waits.

## Milestone 22.1 — Measure at scale

Most of this plan's changes are tuning, and tuning needs numbers from
a suite with thousands of tests, not 18.

- **The scale suite**, `testdata/scale/`: a generator
  (`cargo xtask scale-suite [--tests N]`, default 1,000) that writes
  scenarios against e2e-shop's built app (`dist/`) and mocks. The tests
  are a realistic mix: short (one `goto`, two `expect`s), medium (sign
  in, act, check) and long (checkout), about 60/30/10. Some files have
  `@before-all`. The output is deterministic for a given N, and is not
  committed.
- **A phase breakdown.** `steps.jsonl` already marks setup and check
  steps. Add the runner's own work after the steps (`teardown`: log
  collection, `context close`) as records too, so the overhead can be
  split by phase without a profiler.
- **The benchmark reports everything before it asserts.**
  `e2e_runner_overhead_and_the_shop_suite` prints the overhead and the
  `--jobs 4` suite time, then checks the budgets.
- **A new `xtask bench scale`** runs the scale suite with `--jobs` 1,
  2, 4, 8, 16 and 32, and writes a table to
  `docs/benchmarks.md`: wall time, time per test, overhead per test by
  phase, peak RSS of Chrome and the daemon.
- Record today's numbers as the baseline for the later milestones.

## Milestone 22.2 — Runner overhead

Target: < 30 ms per test (the plan 11 budget is 100 ms).

- **One connection per worker.** Each worker thread keeps one runtime
  and one daemon client for its session and reuses them for every call.
  If the connection fails, it reconnects once, then fails the test as
  today. `crate::run` gets a path that takes an open client; the CLI
  path stays as it is.
- **Set up a context in one call.** `context new` takes the test's mock
  sets, its viewport or device, and the determinism settings, and
  applies them before it answers (new optional parameters on
  `CONTEXT_NEW`). Each setting is still recorded as a `setup` step in
  `steps.jsonl` with the time it took, so reports and flake diagnosis
  read the same.
- **Only new unmatched requests.** `mock log` gets `--after SEQ`, and
  the runner passes the last sequence number it saw. Better still, a
  step's response carries how many unmatched `fail` requests the
  context has, and the runner asks for the log only when that number
  went up. Pick one in the milestone; the decisions log says why.
- **Logs only when they're needed.** A test that passed on its first
  try skips the console, network and mock-log collection. They are
  still collected:
  - for a failed try (as today);
  - for every try of a test that had a failed try, and every run under
    `--repeat`, since flake diagnosis compares failing runs with passing
    ones;
  - with `--trace`, `--ui-coverage` in real mode, `--record-mocks` or
    `--verify-mocks`;
  - with `[e2e] keep_logs = true`.
- **No history in test sessions.** The runner's requests say "no
  history", and the daemon skips `stable_locators` and the history
  entry for them. Sessions the user opens are unchanged.
- **The no-errors check reuses what it has.** When logs are collected,
  the console errors come from the same read; when not, `expect
  --no-errors --all-tabs` stays a single call.

## Milestone 22.3 — Waits that fit tests

Target: e2e-shop's serial wall time at least 40% lower than 22.1's
baseline, with no new flakes.

- **A test settle.** In test runs, a navigation (a `goto`, or one an
  action caused) waits for the load event, then until the requests the
  page started since the navigation are done and the DOM has gone 50 ms
  without a change, for at most 1 s. It no longer waits for 500 ms of
  quiet across the whole network. After an action that didn't navigate,
  the minimum quiet period is 50 ms instead of 150 ms. Both are
  constants with a one-line reason next to them, like today's.
- **`[e2e] settle = "test" | "full"`**, default `"test"`. `"full"`
  keeps today's waits for a suite that needs them; the docs say it is
  for apps that change the page late without anything an `expect` can
  wait on. A step can still ask for more: `goto --wait settle`,
  `wait --idle`, `wait --settle`.
- **The interactive defaults don't change.** The CLI's `goto` and
  actions keep `IDLE_QUIET`, `DOM_QUIET` and `ACTION_QUIET`. The test
  settle is chosen by the runner through the request, not by a global
  switch.
- **Late console errors.** With a shorter settle, a console error from
  a request that ends after the last step can arrive after the
  no-errors check. The check waits for the context's own requests to
  finish (at most 1 s) before it reads the console, so it catches them
  as before.
- **Flake diagnosis knows the setting.** `flake::Setting` gets the
  settle mode. When a test is `unsettled` under `"test"`, the
  diagnosis's hint suggests an `expect` on what the step waited for
  (the request or the element), and mentions `settle = "full"` last.
- **Gates:** `e2e-shop --repeat 20`, `cargo xtask e2e --repeat 20`,
  and the scale suite `--repeat 5` all pass. Record the scale suite's
  numbers.

## Milestone 22.4 — How many at once

- **Measure the curve.** With 22.2 and 22.3 in, run `xtask bench scale`
  on the Linux runners (2, 4 and 16 cores) and on macOS. Find where
  wall time stops dropping and where Chrome's memory becomes the limit,
  with one browser (`--jobs-isolation context`) and with a browser per
  job.
- **Check the daemon scales.** With one browser, every worker talks to
  one daemon. Find out whether anything in it serializes the workers
  (a lock held across an await, one CDP connection for all contexts)
  using `tokio-console` or a span timer on each RPC. Fix what the
  numbers show.
- **Pick the default.** Replace `available_parallelism() / 2` with what
  the curve shows. The likely answer is a function of cores and free
  memory, capped. `test` prints the number it picked and why
  (`8 at once: 4 cores, 12 GB free`), as it prints `N at once` today.
- **Above some number of jobs, one browser may stop being the best
  choice.** If the curve says so, `--jobs-isolation auto` (the new
  default) uses a browser per K jobs. Otherwise this item is dropped
  and the decisions log says why.

## Milestone 22.5 — Scheduling by duration

- **Durations are kept.** Each run updates `e2e/.durations.json` (in
  `[e2e] artifacts`' parent, configurable as `[e2e] durations`): for
  each test, by file and name, a moving average of its passing
  durations. Tests that no longer exist are dropped. The file is meant
  to be cached in CI, not committed; `init` adds it to `.gitignore`.
- **Longest first.** With durations known, workers take tests longest
  first (tests with no duration go first, as if they were long). A
  file's `@before-all` still runs once before its first test, and its
  `@after-all` after its last, wherever they fall in the order.
- **Output order.** Today a line prints when its test and every test
  before it in the plan are done; with longest-first order that would
  hold most lines until the end. Lines print in the order tests finish.
  The summary, `--json` and the reporters keep the plan's order.
- **Sharding by duration.** `--shard i/n` with durations known splits
  the tests so each shard's total time is as equal as greedy packing
  gives (longest first, into the shard with the least time). The split
  depends only on the selected tests and the durations file, so every
  CI machine computes the same shards from the same cached file.
  Without the file, sharding works as today. `test --list --shard i/n`
  shows each shard's expected time.
- **Failed first.** `--failed-first` runs the tests that failed last
  time first (the durations file keeps the last status). It is cheap
  once the file exists, and on a red build it shows the failure in
  seconds.

## Not in this plan

- **Running only the tests a change affects.** Mapping source files to
  tests needs coverage data per test (plan 15 has source maps, not
  per-test coverage). This is worth a plan of its own once suites this
  large exist.
- **Reusing a context between tests.** It would save the `context new`
  and setup call, but tests would share storage, service workers and
  caches. A fresh context costs little once setup is one call (22.2).
- **Distributed runs without CI shards** (a coordinator handing tests
  to machines). Duration-based shards cover most CI setups.
- **Skipping unchanged tests whose last run passed.** It needs to know
  what the app changed, so it is the same problem as the first item.

## Acceptance criteria

- [ ] `xtask bench scale` exists, runs the 1,000-test suite, and its
      table is in
      `docs/benchmarks.md`, with the 22.1 baseline next to the final
      numbers.
- [ ] Runner overhead < 30 ms per test (`tests/bench.rs`).
- [ ] e2e-shop with `--jobs 1` is at least 40% faster than the 22.1
      baseline.
- [ ] `e2e-shop --repeat 20`, `cargo xtask e2e --repeat 20` and the
      scale suite `--repeat 5` pass with zero failures.
- [ ] A failing test's artifacts are the same files with the same
      content as before this plan (goldens on `report.md` and the file
      list).
- [ ] Plan 18.3's flake fixtures get the same diagnosis.
- [ ] With durations known, the slowest shard of `--shard i/4` on the
      scale suite takes at most 10% longer than the average shard.
- [ ] Interactive `goto` and `click` take the same time as before (the
      micro suite).
- [ ] `cargo xtask release check` passes: the CLI and schemas only grew.

## Tests

- Unit: the durations file (moving average, dropped tests, a bad
  file is a warning and is rewritten), longest-first ordering with
  `@before-all`/`@after-all`, greedy sharding (deterministic, covers
  every test once, balanced on hand-made durations), which tries
  collect logs.
- Integration (real Chrome): the runner with one client per worker
  survives a daemon restart in the middle of a run; setup in one call
  gives the same `steps.jsonl` setup records; `--after SEQ` returns
  only new entries; a test whose failing request ends after its last
  step still fails the no-errors check under `settle = "test"`; output
  order with durations.
- Golden: `test` output with durations (finish order, the summary in
  plan order), `test --list --shard` with expected times, the new
  config keys' errors.
- Benchmarks: `tests/bench.rs` (overhead, e2e-shop) and
  `xtask bench scale`.

## Decisions log

- The faster waits are a test-run setting, not new CLI defaults. An
  agent reading the page after `goto` needs the page to have settled,
  because it reads once and does not retry. A test retries every
  `expect`, so the settle only adds time.
- Logs of passing first tries are skipped, not made cheaper: nobody
  reads them, and flake diagnosis only needs runs of tests that failed
  at least once, or ran under `--repeat`, which still keep them.
- Durations live in a cached file, not in the scenarios or the config:
  they change on every run, and a committed file would conflict on
  every branch.
- 22.1: the `teardown` records (the log reads and `context close`) are
  in `steps.jsonl` only, not in the steps `report.md`, flake diagnosis
  and `--json` read, so a failure's report and diagnosis stay the same
  ("Failures look the same"). The HTML reporter's timeline reads the
  file, so it shows them, tagged `[teardown]` as setup steps are tagged
  `[setup]`.
- 22.1: the scale suite copies e2e-shop's `dist/`, mocks, vars and
  steps instead of pointing at them, so it is a project of its own that
  `chrome-cdp test` runs as is, and serves on port 4183 so it can run
  next to e2e-shop.
- 22.2: one connection per worker is a property of the worker's thread
  (`session::Worker`), not a second way into `crate::run`: while it
  lives, every command on the thread takes the worker's runtime, and
  `daemon::client::connect` hands back the connection the thread's last
  command left, so each step still goes through the CLI's own code. A
  connection is not kept after a timeout (a late answer could still
  come), a broken socket, or `open`, `connect`, `close` and `tab close`
  (the daemon's state changed); a kept one whose daemon has another pid
  now is dropped, and a kept one that breaks on its first call connects
  again once. The command line keeps a runtime and a connection per
  command.
- 22.2: `context new`'s setup is a list of calls, each a method and its
  params as the setting's command sends them (only `mock_use`,
  `emulate` and `init_script_add`), so the runner makes them from the
  same words as before and a setting's record and failure read the
  same; the daemon times each one. The mock server's partition for the
  test is now made before `context new` rather than after.
- 22.2: new unmatched requests: `mock log --after SEQ`, not a count in
  each step's answer. On the kept connection the look is one cheap call
  (under a millisecond), and a count would grow every command's
  envelope for the runner's sake.
- 22.2: the no-errors check stays one `expect --no-errors --all-tabs`
  call whether or not the logs are read. It reads the daemon's buffers
  (0.06 ms a test in the baseline, nothing measurable since), and it
  runs before `@after` while the logs are read after it; taking the
  errors from the console read would move that read and redo the
  check's own deduplication of workers' errors and its failure text,
  which "Failures look the same" forbids changing.
- 22.2: "no history" was already the runner's behavior: only `main` and
  `run`/`batch` give a call its words, and a call without words is
  neither kept nor given stable locators. The worker now says so for its
  whole thread (`daemon::client::keep_history`), which also covers a
  step's own `run` or `batch`; the protocol needs no new field.
- 22.2: the round trips were a small part of the overhead (126 to 123
  ms per test on the scale suite with one job). Nearly all of the rest
  was Chrome's: about 80 ms to make a new context's tab ready and about
  20 ms to dispose of a context. So in a session of its own (not with
  `--session` or `--debug`, where the user sees the contexts) the runner
  takes both off the test's path: `context new` with `ahead` takes a
  context the daemon made while the last test ran, fresh and unused, so
  this is not the "reusing a context" this plan leaves out; and `context
  close` with `background` forgets the context at once and leaves its
  disposal to when the next context is made ahead. Two things the
  measurements showed: Chrome answers one call at a time, so the next
  context is made (and the closed one disposed of) only once the test's
  own context has its settings, else they slow the settings down; and
  request interception is the whole session's, so a background close
  leaves it on rather than switching every tab off and on again. Tabs of
  a context made ahead have no context name until taken, which is how
  `tabs`, `status` and the fallback tab leave them out. At most 8 are
  made ahead. The cost, kept rather than reverted: Chrome's memory (1.9
  GB instead of 1.5 with one job, 18 instead of 12.4 GB with 32), and
  with 32 jobs, past where wall time stops dropping, a slower run (189 s
  against 180 s). How many to make ahead, and when to dispose, belongs
  with 22.4's curve.
- 22.2: making the next context now happens during a test's first steps
  rather than in its setup, so a loaded machine sees more of Chrome's
  work while steps run. The flake gate showed it on the planted flakes
  (18.3), whose margins were under a second: a good run's 1 s click
  timed out, and a bad run's slow answer came between Pay and the log
  read. Their margins are now about 1.5 s each; the diagnoses and
  goldens are the same. Suites with sub-second timeouts may want them
  looked at too; 22.3's shorter waits make that likelier.
- 22.3: the runner chooses the settle through the request's envelope, a
  `settle` field beside the params (like `root` and `fingerprint`), set
  for a worker's thread (`daemon::client::set_settle`) as 22.2's "no
  history" is, and scoped in the daemon to the call it came with. Not a
  field of each request type: every navigation and action of a call
  settles the same way, a step's `run` or `batch` included, and an
  action's request has no business knowing about test runs. Without the
  field a call settles as the command line does, so older CLIs and every
  interactive command are unchanged.
- 22.3: a step's own `goto --wait settle` (also `back`, `forward`,
  `reload`) asks for the full settle: the script notes that the line
  names `--wait`, and the call goes out without the test settle. The
  default `--wait` stays `settle`, so the help and the command line
  surface are the same; the test settle is what `settle` means in a test
  run unless the step says it.
- 22.3: after a navigation in a test run, not settling within 1 s is not
  a warning, unlike the command line's 5 s: the page is there, and the
  next `expect` waits for what it needs. A warning on every step of a
  page that polls would bury the useful ones.
- 22.3: the no-errors check waits for every tab's requests in flight
  (not only those since some step) to end and 50 ms of quiet, at most
  1 s, and reads the console then. A request that ended within the last
  50 ms still gets the 50 ms, since its answer's handler logs the error a
  little after the response. The check took 0 ms on e2e-shop before and
  after: its tests leave nothing open.
- 22.3: an `unsettled` flake's fix under `"test"` asks first for an
  `expect` of what the open request's answer shows, then for `wait
  --response`, and names `settle = "full"` last. The diagnosis itself
  (the cause, evidence, confidence) does not depend on the setting, so
  the planted flakes are diagnosed as before; only the fix's text
  differs, and under `"full"` it is the old one. What the step waited
  for is always a request here: the rule finds requests open when a step
  acted, so "the element" is what the answer shows.
- 22.3: the numbers. e2e-shop with `--jobs 1`: 12.8 s against 22.1's
  40.8 s (69% less; the target was 40%). The scale suite: 560 s with one
  job (1950 s before), 162 s with 8. Above 8 jobs the setup per test went
  up (16 jobs: 779 ms against 285 ms) while wall time still went down
  (166 s against 188 s): with tests this short, Chrome does not make the
  next contexts ahead as fast as workers take them. Kept, not reverted:
  it is the cost of the run's contexts arriving sooner, not new work, and
  22.4 measures how many to make ahead and how many jobs to run. The
  gates: e2e-shop `--repeat 20` (360 runs) and the scale suite `--repeat
  5` with 16 jobs (5,000 runs) passed with no failure.
- 22.3: what the `cargo xtask e2e --repeat 20` gate found. The planted
  race was once diagnosed `unknown`: its passing runs' answers seemed to
  come after the step that showed them. A request was timed by when the
  daemon heard of it, and a busy daemon hears of a page's requests late;
  the 500 ms of idle a `goto` waited for hid that, and the test settle
  does not. A request's time is now Chrome's `wallTime` (when within
  10 s of when the daemon heard of it). A debug test also counted a
  logpoint's hits before its console entry arrived; it now waits for
  the hit.  With both fixed, `cargo xtask e2e --repeat 20` passed with no
  failure (20 runs of 1,447 tests). Interactive commands kept their
  times: the micro suite's `click` 326 ms before 22.3 and 325 ms after,
  and 20 `goto`s of a page that fetches after load 846 ms (median) with
  both builds.
