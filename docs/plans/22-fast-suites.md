# Plan 22 — Fast suites

Status: NOT STARTED
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
