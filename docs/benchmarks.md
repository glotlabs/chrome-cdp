# Benchmarks

Four suites measure how fast chrome-cdp is, how few tokens its output
costs, how well an agent does with it (plan 10, milestone 10.4), and how
fast its test runner gets through a large suite (plan 22).
`cargo xtask bench` runs them; this page says what each measures, how to run it, and what it
found last time. The results sections are rewritten by the suites
themselves (between the `<!-- bench:… -->` markers); edit the rest by hand.

| Suite | What it measures | Needs | Cost |
|---|---|---|---|
| `micro` | Per-command latency | Chrome | a few minutes; timing-sensitive |
| `tokens` | Snapshot size in tokens | Chrome, Node | a minute; deterministic |
| `agent` | Claude Code's success, tokens, turns, files read and time on ~30 tasks with chrome-cdp | Chrome, Node, `claude` logged in | hours and API spend |
| `scale` | `chrome-cdp test` on 1,000 generated tests at several `--jobs` | Chrome | about two hours |

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
sandbox with its own daemon and Chrome. For each run, the table has:

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
Last run: 2026-10-18 on macos-aarch64 (16 CPUs), Google Chrome for Testing 155.0.8059.84. 1000 tests (`cargo xtask scale-suite --tests 1000`).

| `--jobs` | Wall time | Per test | Test p50 | Overhead | setup | check | teardown | other | Chrome RSS | Daemon RSS | Failed |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 560 s | 560 ms | 260 ms | 17.5 ms | 10.8 ms | 0.00 ms | 0 ms | 6.75 ms | 1.9 GB | 27 MB | 0 |
| 2 | 308 s | 308 ms | 318 ms | 19.6 ms | 13.2 ms | 0.00 ms | 0 ms | 6.40 ms | 2.6 GB | 29 MB | 0 |
| 4 | 199 s | 199 ms | 441 ms | 44.1 ms | 37.5 ms | 0.01 ms | 0 ms | 6.56 ms | 3.9 GB | 31 MB | 0 |
| 8 | 162 s | 162 ms | 769 ms | 157 ms | 150 ms | 0.02 ms | 0.00 ms | 6.98 ms | 6.5 GB | 35 MB | 0 |
| 16 | 166 s | 166 ms | 1802 ms | 787 ms | 779 ms | 0.05 ms | 0.00 ms | 7.44 ms | 9.3 GB | 42 MB | 0 |
| 32 | 181 s | 181 ms | 4576 ms | 3175 ms | 3167 ms | 0.04 ms | 0.00 ms | 8.41 ms | 15.2 GB | 54 MB | 0 |
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
