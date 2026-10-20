# Benchmarks

Five suites measure how fast chrome-cdp is, how few tokens its output
costs, how well an agent does with it (plan 10, milestone 10.4), how
fast its test runner gets through a large suite (plan 22), and how many
real-browser users one machine can load a site with (plan 23).
`cargo xtask bench` runs them; this page says what each measures, how to run it, and what it
found last time. The results sections are rewritten by the suites
themselves (between the `<!-- bench:… -->` markers); edit the rest by hand.

| Suite | What it measures | Needs | Cost |
|---|---|---|---|
| `micro` | Per-command latency | Chrome | a few minutes; timing-sensitive |
| `tokens` | Snapshot size in tokens | Chrome, Node | a minute; deterministic |
| `agent` | Claude Code's success, tokens, turns, files read and time on ~30 tasks with chrome-cdp | Chrome, Node, `claude` logged in | hours and API spend |
| `scale` | `chrome-cdp test` on 1,000 generated tests at several `--jobs` | Chrome | about two hours |
| `load` | `chrome-cdp load` at 25 to 400 users, a Chrome per 10, 25 or 50 | Chrome, a big machine | about half an hour |

## Setup

All suites drive the Chrome that `CHROME_PATH` points at
(`cargo xtask chrome stable` installs Chrome for Testing and prints its
path). The token suite's counter lives in `bench/tokens/`, a Node package
used only by the benchmarks:

```sh
export CHROME_PATH=$(cargo xtask chrome stable)
(cd bench/tokens && npm ci)
```

`cargo xtask bench <suite>` builds the release `chrome-cdp` and `fixtures`
binaries, then runs the harness (`crates/bench`) with any further
arguments. `cargo xtask bench <suite> -- --help` lists them. Results go to
`target/bench/` as JSON, and into this page (`--no-publish` leaves it
alone).

## Micro suite

```sh
cargo xtask bench micro                 # measure, publish here
cargo xtask bench micro -- --check      # also fail on a >15% regression
cargo xtask bench micro -- --update-baseline
```

What it times, each as a whole `chrome-cdp` invocation (process start,
the socket round trip and the daemon's work: what an agent pays per step):

- **cold start**: `open about:blank` with no daemon or browser running
  (5 samples);
- **warm round trip**: `open -q` on a running session, a no-op (50);
- **snapshot latency by page size**: `snapshot` of the plan 04 corpus
  pages `simple`, `forms`, `big-app` and `huge` (~50,000 nodes);
- **locators** on `huge`: `text` of a `css=`, `text=`, `role=` and
  `xpath=` locator that finds one element near the end of the page;
- **click-to-report**: `click` on a counter button, including the action
  report it prints;
- **route overhead**: loading `bench/micro/requests.html` (50 images)
  with no routes, then with 20 rules (19 that match nothing, and one that
  sends every image on with a header); the overhead per request is the
  difference of the medians over 51 requests;
- **batch vs single calls**: the same 10 cheap steps as 10 invocations and
  as one `batch`.

Every number is a median (p50) and a 95th percentile over its samples.
The plans' budgets are checked in the second table: a cold start under
1.5 s and a warm round trip under 30 ms (overview §7, plan 02), a batch at
least 3× faster than separate calls (plan 08), and route overhead under
1 ms per request (plan 07).

`cargo xtask bench micro` also runs the timing tests that check plan
budgets inside the engine, each `#[ignore]`d in the normal test run: the
CDP client's 10,000 evaluations (plan 01), snapshot latency (plan 04),
route overhead's p95 with 20 rules (plan 07), and the E2E runner's own
overhead and the e2e-shop suite's wall time (plan 11). `--no-tests` skips
them.

**Regressions.** `--check` compares chrome-cdp's medians with the
baseline for the host (`bench/baselines/micro-<os>-<arch>.json`) and fails
when one is more than 15% worse. Timings depend on the machine, so record
a baseline (`--update-baseline`) on the machine that checks it, and
commit it. Without one, `--check` warns. The nightly `bench` job runs it.

<!-- bench:micro -->
Not run yet: timing benchmarks run once all plans are implemented.
<!-- /bench:micro -->

## Token suite

```sh
cargo xtask bench tokens                # count, publish here
cargo xtask bench tokens -- --check     # also fail on >15% more tokens
cargo xtask bench tokens -- --tokenizer anthropic
```

For each page of the snapshot corpus (`testdata/site/snapshot/`), it
counts the tokens of `chrome-cdp snapshot`, `snapshot -i`,
`snapshot --fold` and `snapshot --budget 2000`.

The tokenizer is the Anthropic token counting API (`claude-opus-5-5`'s
tokenizer) when `ANTHROPIC_API_KEY` or `ANTHROPIC_AUTH_TOKEN` is set,
otherwise the offline `o200k_base` BPE as an approximation; the results
say which. `--tokenizer anthropic|o200k` forces one.

Counts are deterministic, so `--check` is exact: it fails when a page's
`snapshot` or `snapshot -i` grows more than 15% over the baseline for its
tokenizer (`bench/baselines/tokens-<tokenizer>.json`).

<!-- bench:tokens -->
Last run: 2026-09-30 with Google Chrome for Testing 154.0.8037.57. Tokenizer: `o200k_base` (offline: an approximation; set ANTHROPIC_API_KEY for Claude's own counts).

| Page | `snapshot` | `snapshot -i` | `--fold` | `--budget 2000` |
|---|---:|---:|---:|---:|
| simple | 65 | 40 | 65 | 65 |
| login | 221 | 192 | 221 | 221 |
| forms | 525 | 519 | 525 | 525 |
| hidden | 116 | 79 | 116 | 116 |
| clickable | 229 | 194 | 229 | 229 |
| lists-tables | 665 | 131 | 665 | 665 |
| widgets | 352 | 253 | 352 | 352 |
| modal | 93 | 83 | 93 | 93 |
| shadow | 68 | 47 | 68 | 68 |
| frames | 132 | 100 | 132 | 132 |
| article | 226 | 43 | 226 | 226 |
| big-app | 4181 | 2782 | 894 | 2681 |
| huge | 7752 | 7130 | 140 | 2775 |
| cards | 2663 | 1228 | 349 | 2663 |
| **total** | **17288** | **12821** | **4075** | **10811** |
<!-- /bench:tokens -->

## Scale suite

```sh
cargo xtask scale-suite                     # write testdata/scale/ (1,000 tests)
cargo xtask bench scale                     # write it, run it, publish here
cargo xtask bench scale --tests 200 -- --jobs 1,4,16 --no-publish
cargo xtask bench scale -- --jobs 8,8:context,8:browser
cargo xtask bench scale -- --jobs 16,16,4 --shards 4
```

Plan 22 tunes the test runner, and tuning needs a suite of realistic
size. `cargo xtask scale-suite [--tests N]` writes one to
`testdata/scale/` (git-ignored; the same N always writes the same
files): N tests (1,000 by default) against e2e-shop's built app
(`dist/`) and mocks, copied in. About 60% are short (one `goto`, two
`expect`s), 30% medium (sign in, act, check) and 10% long (a checkout
of one or two products); files have 5 to 40 tests, and about a quarter
have an `@before-all` that hands the tests a `state save` file.

`cargo xtask bench scale` writes the suite (`--tests N` before the `--`),
then runs `chrome-cdp test --json` on it once for each `--jobs` value
(1, 2, 4, 8, 16 and 32; `-- --jobs 1,4` picks others), each in a fresh
sandbox with its own daemons and Chromes. A value runs with the runner's
default `--jobs-isolation` (`auto`, a Chrome per 3 jobs), or the one
after a colon: `16:context` (one Chrome), `16:browser` (a Chrome each).
The first column says how many Chromes a run used, when more than one.
For each run, the table has:

- **wall time**, and **per test**: the wall time over the number of
  tests, what a test costs the suite;
- **test p50**: the median of the tests' own durations;
- **overhead**: what the runner adds to a test beyond its steps (its own
  and its hooks'), on average, split by the records each test's
  `steps.jsonl` has of the runner's work: `setup` (`context new`, the
  mocks, the viewport, the determinism settings), `check` (`expect
  --no-errors`), `teardown` (reading the logs, `context close`) and
  `other` (what no record covers, and the records' rounding down to
  whole ms; the mock-log look after each step is inside the steps'
  times, not here);
- **peak RSS** of Chrome (its main process and every process under it)
  and of the daemon, read with `ps` every 250 ms;
- how many tests failed (or were flaky). A run that was not clean fails
  the suite, after the table is written.

The bench removes the suite's durations file (plan 22.5) first, so the
first run starts the tests in the plan's order; each run writes the file,
so the runs after it start the longest first, and their rows say
`longest first`. `--shards N` then runs each of N shards on its own, as
on a CI machine of its own, with the last `--jobs` value: split by count
(without the durations file) and by time (each shard from the same file,
the one the runs before it left), and writes a second table of each
shard's tests, expected time and wall time, and how far above the mean
the slowest shard took.

`--baseline` also writes the table into the baseline section, which
plan 22's later milestones compare with. Results go to
`target/bench/scale.json`.

Baseline (plan 22.1):

<!-- bench:scale-baseline -->
Last run: 2026-10-17 on macos-aarch64 (16 CPUs), Google Chrome for Testing 155.0.8059.84. 1000 tests (`cargo xtask scale-suite --tests 1000`).

| `--jobs` | Wall time | Per test | Test p50 | Overhead | setup | check | teardown | other | Chrome RSS | Daemon RSS | Failed |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1967 s | 1967 ms | 989 ms | 126 ms | 93.4 ms | 0.00 ms | 20.4 ms | 11.9 ms | 1.6 GB | 23 MB | 0 |
| 2 | 995 s | 995 ms | 1029 ms | 129 ms | 110 ms | 0.01 ms | 6.46 ms | 12.1 ms | 1.9 GB | 24 MB | 0 |
| 4 | 514 s | 514 ms | 1081 ms | 152 ms | 136 ms | 0.01 ms | 4.98 ms | 11.7 ms | 2.7 GB | 26 MB | 0 |
| 8 | 286 s | 286 ms | 1294 ms | 236 ms | 218 ms | 0.02 ms | 6.88 ms | 11.4 ms | 4.1 GB | 29 MB | 0 |
| 16 | 189 s | 189 ms | 1855 ms | 502 ms | 475 ms | 0.05 ms | 15.6 ms | 11.4 ms | 6.9 GB | 37 MB | 0 |
| 32 | 175 s | 175 ms | 3638 ms | 1668 ms | 1614 ms | 0.11 ms | 42.0 ms | 11.6 ms | 12.4 GB | 49 MB | 0 |
<!-- /bench:scale-baseline -->

The same day, `tests/bench.rs` on e2e-shop (18 tests, release build):
the serial suite took 40.8 s, the runner's overhead was 141 ms per test
(setup 98 ms, check 0 ms, teardown 30 ms, other 13 ms; the budget is
100 ms), and the suite took 11.9 s with `--jobs 4` (budget 15 s).

What the baseline shows: wall time stops halving above 8 jobs on 16
CPUs, and the runner's setup per test grows from 93 ms with 1 job to
1.6 s with 32 (nearly all of it `context new`), so something in making
a context serializes the workers (milestone 22.4's question). Chrome's
memory grows by about 350 MB per job.

Latest:

<!-- bench:scale -->
Last run: 2026-10-19 on macos-aarch64 (16 CPUs), Google Chrome for Testing 155.0.8059.84. 1000 tests (`cargo xtask scale-suite --tests 1000`).

| `--jobs` | Wall time | Per test | Test p50 | Overhead | setup | check | teardown | other | Chrome RSS | Daemon RSS | Failed |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 562 s | 562 ms | 260 ms | 17.4 ms | 10.6 ms | 0 ms | 0 ms | 6.79 ms | 1.9 GB | 28 MB | 0 |
| 2 | 311 s | 311 ms | 310 ms | 18.5 ms | 12.1 ms | 0 ms | 0 ms | 6.42 ms | 2.6 GB | 28 MB | 0 |
| 4 (2 Chromes) | 165 s | 165 ms | 329 ms | 20.6 ms | 14.1 ms | 0.01 ms | 0 ms | 6.55 ms | 5.1 GB | 52 MB | 0 |
| 8 (3 Chromes) | 104 s | 104 ms | 446 ms | 34.3 ms | 26.9 ms | 0.08 ms | 0.01 ms | 7.27 ms | 8.9 GB | 77 MB | 0 |
| 12 (4 Chromes) | 86.6 s | 86.6 ms | 623 ms | 61.0 ms | 52.2 ms | 0.32 ms | 0.04 ms | 8.45 ms | 12.7 GB | 102 MB | 0 |
| 16 (6 Chromes) | 82.4 s | 82.4 ms | 828 ms | 86.3 ms | 75.9 ms | 0.69 ms | 0.20 ms | 9.54 ms | 17.7 GB | 149 MB | 0 |
| 24 (8 Chromes) | 85.5 s | 85.5 ms | 1361 ms | 184 ms | 172 ms | 1.01 ms | 0.38 ms | 10.6 ms | 24.8 GB | 200 MB | 0 |
| 32 (11 Chromes) | 88.7 s | 88.7 ms | 1880 ms | 258 ms | 245 ms | 1.20 ms | 0.58 ms | 11.1 ms | 33.6 GB | 270 MB | 0 |
| 8 context | 170 s | 170 ms | 839 ms | 167 ms | 160 ms | 0.02 ms | 0.00 ms | 6.97 ms | 6.6 GB | 35 MB | 0 |
| 16 context | 167 s | 167 ms | 1795 ms | 769 ms | 762 ms | 0.04 ms | 0.00 ms | 7.51 ms | 9.3 GB | 42 MB | 0 |
| 32 context | 181 s | 181 ms | 4516 ms | 3159 ms | 3151 ms | 0.06 ms | 0.01 ms | 8.39 ms | 16.5 GB | 54 MB | 0 |
| 8 browser (8 Chromes) | 91.4 s | 91.4 ms | 392 ms | 23.6 ms | 15.6 ms | 0.29 ms | 0.08 ms | 7.66 ms | 14.6 GB | 181 MB | 0 |
| 16 browser (16 Chromes) | 81.7 s | 81.7 ms | 909 ms | 47.6 ms | 36.9 ms | 0.78 ms | 0.30 ms | 9.56 ms | 28.6 GB | 357 MB | 0 |
| 32 browser (32 Chromes) | 103 s | 103 ms | 2406 ms | 102 ms | 87.4 ms | 0.96 ms | 0.19 ms | 13.0 ms | 43.8 GB | 596 MB | 0 |
<!-- /bench:scale -->

What 22.2 changed. Fewer round trips alone (one daemon connection per
worker, a context and its setup in one call, only new unmatched
requests, no logs for passing first tries) took the overhead with one
job from 126 ms to 123 ms: the rest was Chrome making a context's tab
(about 80 ms) and disposing of it (about 20 ms). With the runner's
contexts made ahead and the closed ones disposed of meanwhile (in a
session of the run's own), it is 19.9 ms with one job and under 30 ms
up to 4 jobs, and wall time is lower up to 16 jobs (16: 188 s; the 22.1
build, run again the same day, took 197 s). The cost is Chrome's
memory, a context or two more per job, and with 32 jobs, past the point
where wall time stops dropping, 8 contexts made ahead and the closed
ones awaiting disposal slow the setup (2.2 s a test) and the run (189 s
against 180 s). The look for unmatched requests after each step is
inside the steps' times, so what it saves is not in this table.

The same day, `tests/bench.rs` on e2e-shop: an overhead of 26 ms per
test (before 22.2: 134 ms), of which setup 18 ms, teardown 0 ms and
other 8 ms; the suite took 12.1 s with `--jobs 4` (before: 11.8 s).

What 22.3 changed. A test run's steps settle for less (a navigation
until its own requests are done and the DOM was quiet for 50 ms, 1 s at
most; an action for 50 ms of quiet), since the next `expect` retries.
With one job the suite takes 560 s instead of 1950 s (72% less), and a
test's median 260 ms instead of 973 ms; up to 8 jobs, wall time is 42%
to 69% lower. Above 8 jobs it no longer drops (16: 166 s, 32: 181 s),
and the setup per test is higher than before (16 jobs: 779 ms against
285 ms): the tests are now shorter than the time Chrome takes to make the
next context, so the contexts made ahead run out and workers wait on
`context new`. That is Chrome's work for the whole run, not new work per
test; how many contexts to make ahead, and how many jobs, is milestone
22.4's question. The overhead per test with up to 2 jobs is 17.5 to 19.6
ms (before: 19.9 to 20.7).

The same day, `tests/bench.rs` on e2e-shop: with `--jobs 1` the suite
took 12.8 s (22.1's baseline: 40.8 s; `settle = "full"`, the old waits,
41.5 s), with `--jobs 4` 4.9 s (before: 12.1 s); the overhead was 25 ms
per test (setup 18 ms, check 0 ms, teardown 0 ms, other 8 ms).

What 22.4 changed. The runner's default is now as many tests at once as
CPUs (fewer when free memory is short, at most 16), in a Chrome per 3
of them (`--jobs-isolation auto`). Above 4 jobs in one Chrome (the
`context` rows), every test's calls queue in Chrome's browser process:
with 16 jobs it ran at 110% CPU while the machine was half idle, a
trivial browser call took 15 to 60 ms instead of 11, and the setup calls
that take under 1 ms with one job took 90 to 180 ms. The daemon was not
the limit: it used 6% of a CPU and answered `status` as fast as when
idle. With a Chrome per 3 jobs, wall time keeps dropping up to one job
per CPU (16: 82.4 s, against 167 s in one Chrome and 162 s for 22.3's
best, 8 jobs) and setup per test is 76 ms instead of 762 ms; past one
per CPU it rises again (32: 88.7 s). A Chrome per job is as fast at 16
(81.7 s) for 11 GB more; a Chrome per 2 or 4 jobs was measured too
(16 jobs: 87.8 s and 89.1 s). Chrome's memory is about 1.1 GB per job.
Only this machine was measured.

What 22.5 changed. Each run keeps its tests' durations, and the next
one starts the longest first, so a long test no longer starts last. On
this suite the gain is small, since its longest test (a checkout, about
1.5 s alone) is short next to the run: with 16 jobs 82.2 s against
82.8 s in the plan's order, run the same day. Starting the checkouts
together loads Chrome early, so a test's median and setup are higher
(1113 ms and 92 ms against 842 ms and 76 ms) for a slightly shorter run.
With 4 jobs, 163 s (22.4: 165 s). Shards split by time are within 1% of
each other, where shards by count were within 6% (the generated suite
spreads its long tests evenly over its files; a suite whose slow tests
sit together in a few files gains more):

<!-- bench:scale-shards -->
Each of 4 shards on its own with `--jobs 4`, by count (no durations file) and by time (the file the runs above left):

| Shard | By count: tests | Wall time | By time: tests | Expected | Wall time |
|---:|---:|---:|---:|---:|---:|
| 1/4 | 250 | 48.3 s | 250 | 247 s | 45.1 s |
| 2/4 | 250 | 45.9 s | 250 | 247 s | 44.9 s |
| 3/4 | 250 | 43.1 s | 250 | 247 s | 45.5 s |
| 4/4 | 250 | 44.8 s | 250 | 247 s | 45.6 s |

The slowest shard: by count 48.3 s, 6.1% above the mean; by time 45.6 s, 0.8% above the mean.
<!-- /bench:scale-shards -->

"Expected" is the shard's tests' times added up, as the durations file
had them after runs with 16 and 4 jobs (`cargo xtask bench scale -- --jobs
16,16,4 --shards 4`); a test takes longer when more run beside it, so it
is the proportions that matter.

## Load suite

```sh
cargo xtask bench load                        # 15 runs of 60 s, publish here
cargo xtask bench load -- --users 25,50 --per-browser 25 --duration 30 --no-publish
cargo xtask bench load -- --per-browser 10 --cache returning
```

The Chrome it drives is `CHROME_PATH` (or `--chrome`); point it at
chrome-headless-shell, which `load` runs by default, or at full Chrome to
measure `load --browser chrome`.

bench: load. Plan 23 load-tests sites with hundreds of real browsers on
one machine, and its headline number is **users per machine**: the most
users at once on a fixed journey before the machine running them (not
the site) is the limit. `cargo xtask bench load` runs `chrome-cdp load`
on the reference journey, `testdata/load/scenarios/shop.cdp` (browse the
products, open one, add it to the cart, look at the cart; 2 s of `think`
on each page), against the load server (`testdata/load/server`, started
in the harness's process with no latency or errors set), at 25, 50, 100,
200 and 400 users, each with a Chrome per 10, 25 and 50 users
(`--users-per-browser`), for 60 s after a 10 s ramp-up. Each run starts
from a fresh sandbox. For each, the table has:

- **iterations** and the share that **failed**;
- **step p50 and p95**: the steps' own times from `steps.jsonl`
  (`--raw`), `think` steps left out;
- **Chrome's memory per user**: the peak of all its processes' memory
  added up, over the users; PSS on Linux (shared pages split between the
  processes that share them), RSS elsewhere, which counts Chrome's shared
  framework once per process and so reads several times too high on
  macOS;
- **Chrome's CPU per user**, in % of a core, and **chrome-cdp's own CPU**
  (in cores) and peak **RSS**: CPU time from `ps` every 500 ms, each
  process's growth added up (a process that exits between two reads
  loses at most its last half second); **all CPUs** is the two together;
- **machine CPU**: the whole machine's CPU use at most, as `load`'s
  capacity check read it (plan 23.5), and **saturated**: the seconds that
  check found the load generator saturated, and the limit that tripped
  (CPU above 90%, free memory below 10%, or the run's timers 50 ms late
  at p99, for 5 s in a row);
- whether the run **held**: under 1% of its iterations failed, and the
  generator never saturated. Users per machine is the most users of a run
  that held. (Before 23.5 a run held when chrome-cdp and its Chromes used
  under 90% of the machine's CPUs and its step p95 stayed within twice
  the smallest run's.)

Plan 23.1's target is 200 users on a 16-core, 32 GB Linux runner, with
chrome-cdp itself under 1 core and 500 MB. Results go to
`target/bench/load.json`.

<!-- bench:load -->
Last run: 2026-10-20 on macos-aarch64 (16 CPUs), Google Chrome for Testing 155.0.8059.84 as chrome-headless-shell. `testdata/load/scenarios/shop.cdp`, 60 s per run after a 10 s ramp-up, `cache = "new"`.

**Users per machine: 200** (the most users of a run that held).

| Users | Per Chrome | Chromes | Iterations | Failed | Step p50 | Step p95 | Chrome RSS / user | Chrome CPU / user | chrome-cdp CPU | chrome-cdp RSS | All CPUs | Machine CPU | Saturated | Contexts | Context setup | Held |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|:---:|
| 25 | 10 | 3 | 260 | 0% | 3.90 ms | 157 ms | 152 MB | 2.24% | 0.02 | 25 MB | 0.58 | 12% | no | 260 | 42.8 ms | yes |
| 50 | 10 | 5 | 521 | 0% | 3.18 ms | 157 ms | 147 MB | 2.15% | 0.04 | 29 MB | 1.12 | 20% | no | 521 | 45.9 ms | yes |
| 100 | 10 | 10 | 1040 | 0% | 3.55 ms | 156 ms | 146 MB | 2.10% | 0.08 | 36 MB | 2.18 | 31% | no | 1040 | 55.9 ms | yes |
| 200 | 10 | 20 | 2074 | 0% | 17.6 ms | 156 ms | 145 MB | 2.27% | 0.18 | 50 MB | 4.73 | 65% | no | 2074 | 84.1 ms | yes |
| 400 | 10 | 40 | 3662 | 0% | 103 ms | 412 ms | 144 MB | 2.29% | 0.24 | 96 MB | 9.40 | 100% | 61 s (cpu) | 3662 | 492 ms | no |
<!-- /bench:load -->

The table above is chrome-headless-shell with a Chrome per 10 users.
With plan 23.5's capacity check it says 200 users per machine where
23.2's stand-in said 400: at 400 users chrome-cdp and its Chromes used
9.4 of the 16 CPUs, but the whole machine was at 100% (macOS's own
services take several cores for the headless Chromes, as 23.1 found),
and the generator was saturated for 61 s of the run, by its CPU. Watching
the machine every second cost nothing measurable: at 200 users chrome-cdp
used 0.18 cores and 50 MB (23.4: 0.18 cores and 50 MB), at 100 users
0.08 cores and 36 MB (0.08 and 35 MB).
`--cache returning` adds an `@before` of `cookies clear` to the journey,
whose cart would otherwise grow from one iteration to the next; "Contexts"
and "Context setup" are the browser contexts the users made and the mean
time one took.

Plan 23.2 (a lean browser), on the same Mac, a Chrome per 10 users
unless said, 60 s runs (memory is RSS, which reads high on macOS):

| Browser | Cache | Users per machine | Chrome RSS / user | Chrome CPU / user | Step p50 | Context setup | chrome-cdp at the most users |
|---|---|---:|---:|---:|---:|---:|---|
| Chrome (23.1) | new | 50 | 467 MB | 8.6% | 103 ms | | 0.08 cores, 44 MB |
| Chrome, 23.2's flags | new | 50 | 442 MB | 8.0% | 104 ms | 265 ms | 0.08 cores, 43 MB |
| chrome-headless-shell | new | 400 (200 in an earlier run) | 146 MB | 2.2% | 3-27 ms | 46-85 ms (485 ms at 400) | 0.23 cores, 84 MB |
| chrome-headless-shell, 25 per Chrome | new | 200 | 132 MB | 2.2% | 7-12 ms | 44-85 ms | 0.25 cores, 76 MB |
| chrome-headless-shell | returning | 400 | 164 MB | 1.2% | 1.4 ms | 32-48 ms, once per user | 0.25 cores, 315 MB |

The shell is most of the gain: a third of full Chrome's memory and a
quarter of its CPU per user, and its steps do not wait for frames. At
400 users with a Chrome per 10, the shell's step p95 was 279 ms in the
run above, just inside twice the smallest run's, and 471 ms in an
earlier run of nearly the same code, so 400 is the edge. Returning
users make one context each instead of one per iteration, fetch the
app's hashed files once, and use about half the CPU; their Chrome keeps
the cache, so memory per user is a little higher, and chrome-cdp's own
memory grows with each user's long-lived tab (plan 23's decisions log).

Plan 23.3 (the load model), on the same Mac with chrome-headless-shell
and a Chrome per 10 users. `bench load` runs a closed model without a
profile, which 23.3 rewrote around stages; its numbers did not move: at
100 users 1040 iterations, step p95 157 ms, chrome-cdp 0.07 cores and
36 MB; at 200 users 2074 iterations, step p95 155 ms, chrome-cdp 0.16
cores and 49 MB (23.2: 1040 and 2076 iterations, 157 and 155 ms, 0.07
and 0.17 cores, 35 and 49 MB). An open model (`[arrivals]`, 0 to 29
users a second over 10 s, then 29 a second for 60 s, `max_users = 400`)
on the same journey had 1885 arrivals, none dropped, at most 193 users at
once and 0 failures, each iteration 6.6 s from when it was due; an
iteration started 0.8 ms after its due time at p50, 2.4 ms at p99 and
3.9 ms at most (`iterations.jsonl`), so the arrivals kept their schedule
at about the users per machine of 23.2.

Plan 23.4 (metrics, reports and thresholds), on the same Mac with
chrome-headless-shell and a Chrome per 10 users: every request, step,
iteration and page view now goes into the run's histograms, each page
reports its Web Vitals, and the run writes `metrics.jsonl` every second
and its reports at the end. At 100 users: 1040 iterations, step p95
157 ms, chrome-cdp 0.08 cores and 35 MB; at 200 users: 2074 iterations,
step p95 156 ms, chrome-cdp 0.18 cores and 50 MB (23.3: 0.07 and 0.16
cores, 36 and 49 MB). Chrome's CPU per user did not move (2.06% and
2.23%), so the metrics cost chrome-cdp about a hundredth of a core per
100 users, and users per machine is unchanged.

On the macOS runs, from 100 full-Chrome users on, the limit was the
machine: the Chromes held at about 8 of 16 CPUs while macOS's own
services (WindowServer, kernel_task, WindowManager, fontd) took several
cores for them and free memory ran out, which "All CPUs" does not count.
More users per full Chrome made steps slower at every size (plan 23's
decisions log), so `load` defaults to 10.

## Agent task benchmark

This is the headline number for v1 (overview §7): how often an agent
with chrome-cdp succeeds at real browser tasks, with how many tokens, and
how fast.

```sh
cargo xtask bench agent -- --check      # no agent: test the tasks themselves
cargo xtask bench agent                 # every task × 5 runs
cargo xtask bench agent -- --task login --reps 1
cargo xtask bench agent -- --offline --model claude-opus-5-5
```

The tasks live in `bench/agent/tasks/` (see its README): 30 of them, on
the fixture site (log in, a multi-step form, a hover menu, dialogs, a
popup, a cross-site iframe, shadow DOM, drag to reorder, upload, download,
a mobile layout, a console error, a failed request, …), three on a public
sandbox (`--offline` skips them), a bug to find and fix on a local dev
server, four "fix the bug behind this button" tasks on the fixture apps of
`testdata/apps-src` (React 18, React 19, Vue, Svelte) running on Vite's
dev server (they need Node and `cargo xtask apps`), and plan 11's E2E tasks on `examples/e2e-shop`: write scenarios
and mocks for it, fix an app change that broke checkout, and make the
mocked suite also pass against the real backend.

Each run is Claude Code headless (`claude -p`) in a fresh copy of the
task's working dir, with the built-in tools Bash, Read, Edit, Write,
Glob, Grep and Skill, no user settings (`--setting-sources project`: no
CLAUDE.md, skills, hooks or MCP servers of the user's), and chrome-cdp:
`chrome-cdp skill --install` in the project and the binary on `PATH`;
nothing else about chrome-cdp.

Permissions are skipped (`--dangerously-skip-permissions`) so nothing
waits for a human: run it on a machine or VM you are happy to let an agent
use. A run that takes longer than the task's limit (10 minutes, 15-20 for
the E2E tasks) is stopped and fails.

A verifier decides success, never the agent's own report: what the page
posted to the fixture server (`/bench/record`), a `chrome-cdp batch`
script against the fixed app, `chrome-cdp test` in the working dir, files
that must not change (the tests of a "fix the app" task), or, for
questions, the agent's final `ANSWER:` line. Tokens are the sum over every
model Claude Code called (input, output, cache reads and writes), turns
its `num_turns`, files read the Read, Grep and Glob calls in the
transcript (how much code the agent looked through to find the place),
shell reads the commands of its Bash calls that start with a reading
tool (`cat`, `grep`, `rg`, `sed`, `find`, `ls`, …; a pipeline counts
once), and wall time the whole `claude` process. Each task runs
5 times (`--reps`); the tables show the success rate and medians.

`--check` runs no agent. For each task it checks that the verifier fails
on the untouched task and passes after the task's known solution (a
`batch` script, file edits or the expected answer), so a broken fixture or
verifier shows up before a paid run. A task whose fixture app has no
`node_modules` (no Node, or no `cargo xtask apps`) is skipped and says
so. It needs no API access and is safe to
run anywhere.

In CI, the `Agent benchmark` workflow (`.github/workflows/bench-agent.yml`)
runs it by hand only (`workflow_dispatch`, with the `ANTHROPIC_API_KEY`
secret): it checks the tasks, runs the benchmark, and uploads the logs and
the updated page.

Each run's log is kept under `target/bench/agent/<time>/`: Claude Code's
transcript (`stream-json`, the result last), its stderr, and `episodes.jsonl` with every check's outcome.
Only a full run (every task) is published here.

<!-- bench:agent -->
Not run yet: the agent benchmark runs once all plans are implemented.
<!-- /bench:agent -->
