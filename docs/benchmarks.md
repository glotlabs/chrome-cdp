# Benchmarks

chrome-cdp claims to be a better browser tool for agents than Playwright.
Three suites test that claim (plan 10, milestone 10.4). `cargo xtask bench`
runs them; this page says what each measures, how to run it, and what it
found last time. The results sections are rewritten by the suites
themselves (between the `<!-- bench:… -->` markers); edit the rest by hand.

| Suite | What it measures | Needs | Cost |
|---|---|---|---|
| `micro` | Per-command latency, next to the Playwright library | Chrome, Node (for the comparison) | a few minutes; timing-sensitive |
| `tokens` | Snapshot size in tokens, next to Playwright MCP | Chrome, Node | a minute; deterministic |
| `agent` | Claude Code's success, tokens, turns, files read and time on ~30 tasks, with chrome-cdp vs Playwright MCP | Chrome, Node, `claude` logged in | hours and API spend |

## Setup

All suites drive the same Chrome for both tools. Point `CHROME_PATH` at it
(`cargo xtask chrome stable` installs Chrome for Testing and prints its
path). The Playwright side lives in `bench/playwright/`, a Node package
used only by the benchmarks:

```sh
export CHROME_PATH=$(cargo xtask chrome stable)
(cd bench/playwright && npm ci)
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

The Playwright library does the same on the same pages and Chrome
(`bench/playwright/micro.mjs`): its numbers are calls inside one Node
process, as the library has no CLI, except the cold start, which times a
fresh `node` that launches Chrome and opens `about:blank`. Its snapshot is
`page._snapshotForAI()` (what Playwright MCP sends) when the installed
Playwright has it, else `ariaSnapshot()`; the table's note says which.

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
counts the tokens of `chrome-cdp snapshot`, `snapshot -i` and Playwright
MCP's `browser_snapshot` (`bench/playwright/snapshot-tokens.mjs`). It
also audits the trees: every element Playwright gives a ref to that an
agent acts on (buttons, links, fields, …) should have a chrome-cdp ref of
the same name; the last column counts the ones that do not.

The tokenizer is the Anthropic token counting API (`claude-opus-5-5`'s
tokenizer) when `ANTHROPIC_API_KEY` or `ANTHROPIC_AUTH_TOKEN` is set,
otherwise the offline `o200k_base` BPE as an approximation; the results
say which. Both tools' outputs go through the same one, so the ratio holds
either way. `--tokenizer anthropic|o200k` forces one.

Counts are deterministic, so `--check` is exact: it fails when a page's
`snapshot` or `snapshot -i` grows more than 15% over the baseline for its
tokenizer (`bench/baselines/tokens-<tokenizer>.json`).

<!-- bench:tokens -->
Last run: 2026-09-29 with Google Chrome for Testing 154.0.8037.57. Tokenizer: `o200k_base` (gpt-tokenizer, offline: an approximation; set ANTHROPIC_API_KEY for Claude's own counts).

| Page | `snapshot` | `snapshot -i` | Playwright MCP `browser_snapshot` | `snapshot` vs MCP | Actionable (MCP refs) | Without a chrome-cdp ref |
|---|---:|---:|---:|---:|---:|---:|
| simple | 65 | 40 | 128 | 51% | 1 | 0 |
| login | 221 | 192 | 444 | 50% | 10 | 0 |
| forms | 525 | 519 | 1014 | 52% | 31 | 0 |
| hidden | 116 | 79 | 262 | 44% | 4 | 1 |
| clickable | 229 | 194 | 379 | 60% | 3 | 0 |
| lists-tables | 665 | 131 | 3813 | 17% | 8 | 0 |
| widgets | 352 | 253 | 514 | 68% | 15 | 0 |
| modal | 93 | 83 | 353 | 26% | 8 | 4 |
| shadow | 68 | 47 | 115 | 59% | 1 | 0 |
| frames | 132 | 100 | 236 | 56% | 4 | 0 |
| article | 226 | 43 | 415 | 54% | 1 | 0 |
| big-app | 4181 | 2782 | 52898 | 8% | 1245 | 0 |
| huge | 7752 | 7130 | 415654 | 2% | 6000 | 0 |
| **total** | **14625** | **11593** | **476225** | **3%** | | |
<!-- /bench:tokens -->

## Agent task benchmark

This is the headline number for v1 (overview §7): an agent must succeed at
least as often with chrome-cdp as with Playwright MCP, with at most half
its tokens, and faster.

```sh
cargo xtask bench agent -- --check      # no agent: test the tasks themselves
cargo xtask bench agent                 # every task × both arms × 5 runs
cargo xtask bench agent -- --task login --arm chrome-cdp --reps 1
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
task's working dir, with the same built-in tools in both arms (Bash, Read,
Edit, Write, Glob, Grep, Skill), no user settings (`--setting-sources
project`: no CLAUDE.md, skills, hooks or MCP servers of the user's), and
one browser tool:

- **chrome-cdp**: `chrome-cdp skill --install` in the project and the
  binary on `PATH`; nothing else about chrome-cdp;
- **playwright-mcp**: Playwright MCP (`--headless --isolated`) as the
  only MCP server, on the same Chrome, and no `chrome-cdp` on `PATH`.

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
5 times per arm (`--reps`); the tables show the success rate and medians.

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
Only a full run (every task, both arms) is published here.

<!-- bench:agent -->
Not run yet: the agent benchmark runs once all plans are implemented.
<!-- /bench:agent -->
