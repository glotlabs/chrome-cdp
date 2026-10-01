# Plan 12 — After v1.0: roadmap

Status: IN PROGRESS (tracks 1 and 3-8 done in plans 13-19; tracks 0, 2
and 9 open)
Depends on: v1.0 released (plan 10's release checklist)

## Goal

Plans 01–11 built v1: an agent can open, inspect, operate and verify a
web page from a shell, and commit the result as a test. This plan says
what comes next and why. It has two sources:

- **The backlog.** The 92 LOW notes in `.looper/tasks/` that were found
  while building v1. Each one was filed under a track in §6, so none is
  lost; §6 now lists those still open.
- **First principles.** What an agent that changes web code still
  cannot do with chrome-cdp, or with any other tool. Items marked ★ go
  beyond Playwright (library, CLI and MCP).

This is a roadmap, not a plan to execute. Each track picked up gets its
own plan file, written like plans 01–11: milestones sized for one
`.looper` task, acceptance criteria and a test list. Tracks 1 and 3–8
have theirs (plans 13–19, linked in §3); the sketches below say *whether*
and *in what order*, the plans say *how*. Where a plan and this sketch
disagree, the plan wins.

## 1. Where v1 leaves an agent

An agent's loop with a browser has five steps. v1 is strong on the
first three and weak on the last two.

| Step | v1 | What is missing |
|---|---|---|
| **See** the page | `snapshot` with refs, `--diff`, `markdown`, `screenshot --annotate` | Iframes and workers are half-seen (§3 Track 1). Nothing says *why* an element can't be used until an action times out. Layout bugs (overlap, clipping, overflow) are invisible to the AX tree. |
| **Act** | Every Playwright action, auto-wait, the action report | Forms are filled field by field. Some parity gaps (locator `has=`/`or`, paused timers, rich clipboard). |
| **Verify** | `expect`, `expect-snapshot`, `expect-screenshot`, `--a11y`, `test` | Checks are one element at a time. No audit of the whole page: Chrome's own Issues, security, performance budgets, layout. |
| **Debug** | `errors` and `console` with source maps, `network`, `trace`, `perf` | The agent's real question is "**which line of my code** did this?". The page gives a DOM node or a request; the agent edits source files. Nothing connects the two. |
| **Hand off** | `history --as-scenario`, `report.md` | A human is needed for CAPTCHA, 2FA and SSO, and there is no clean way to hand them the browser and take it back. A failure cannot be packed up and replayed by another agent. |

The biggest lever is **Debug**: a coding agent spends most of its time
in its editor, and chrome-cdp stops at the browser's edge. Track 4
closes that gap and is the one this plan would do first after the
foundations.

## 2. Principles for after v1

The v1 principles (overview §2) still hold. Three more:

1. **Every frame, every worker, one model.** A feature that works in the
   main frame but not in an iframe or a worker is a bug, not a gap. New
   features declare which targets they apply to, and the engine applies
   them to all targets of that kind, including ones that attach later.
2. **Close the loop to the source.** Where the browser knows where
   something came from (a stack, a source map, a framework's debug
   info), show the agent the file and line, not the generated bundle.
3. **Beyond Playwright only where an agent gains.** Human-only features
   (a GUI trace viewer, a codegen window) come last. A feature an agent
   can read in text wins over one a human watches.

## 3. Tracks

Each track has: why, the items (new ones and backlog notes), and what
"done" looks like. Backlog notes are named by their `.looper/tasks/`
file, without `.md`.

### Track 0 — Finish v1's open boxes (1.0.x)

**Why.** v1 ships with its measurements and its GitHub and macOS
checks still open (plan 10's close-out). Until they run, the numbers in
the README are claims.

- The HIGH and MEDIUM notes: `choose-license`,
  `run-benchmarks-after-all-plans`, `verify-release-workflow`, the CI,
  nightly, Docker and macOS checks, signing, and
  `skill-validate-with-claude-code`.
- Benchmarks that run with them: `batch-benchmark-run`,
  `route-overhead-benchmark`, `latency-benchmark-fixed-host`,
  `locator-benchmark`, `soak-budgets-tune`,
  `report-brief-settle-busy-pages`, `runner-unmatched-check-cost`,
  `tokens-real-world-corpus`, `readme-playwright-cli-column`.
- Checks that need a display, macOS or a clean machine:
  `doctor-on-macos`, `extensions-headed-and-macos`,
  `headed-highlight-inspect-verify`, `tabs-headed-activate`,
  `headed-viewport-window`, `docker-apparmor-unconfined-sandbox`,
  `with-deps-end-to-end-test`.
- Release chores: `mock-schema-versioned-url`, `doctor-newer-release`.

Done: every v1 checkbox in the overview §7 is ticked or has a decisions
log entry saying why not.

### Track 1 — Every frame, every worker

Plan: [13-every-frame-every-worker](13-every-frame-every-worker.md).
Status: DONE (plan 13, 13.1-13.5).

**Why.** Seventeen backlog notes are the same bug: a feature attached
to the page's own session and missed out-of-process iframes (OOPIFs),
dedicated workers, shared workers or service workers. Fixing them one
by one would take seventeen patches and leave the eighteenth. Fix the
model once.

- **A target graph in the engine.** Every attached target (page, OOPIF,
  dedicated, shared and service worker) is a node with its tab (or none)
  and its context. Features register what they need per target kind
  (`Network.enable`, `Fetch.enable`, emulation, init scripts, console,
  coverage, determinism), and the graph applies them on attach and when
  a feature is turned on later. Shared and service workers belong to
  every tab of their origin in the same context.
- Then, as users of the graph: `network-worker-requests`,
  `route-worker-requests`, `emulate-workers`,
  `console-shared-service-workers`, `mock-shim-workers-binary`,
  `determinism-workers-shadow-dom`, `coverage-cross-site-iframes`,
  `cdp-listen-child-sessions`, `tabs-other-workers`,
  `context-permissions`.
- Reading and acting across frames: `read-page-text-iframes`,
  `wait-text-in-iframes`, `report-focus-in-iframes`,
  `expect-a11y-iframes`, `drag-scroll-edge-cases`,
  `history-frame-locators`, `frame-order-on-connect`.
- A `frames` command (`frames-listing-command`) that prints the whole
  tree, OOPIFs and workers included, with the names `--frame` takes.

Done: one fixture page with a same-site iframe, a cross-site iframe,
and a dedicated, a shared and a service worker. A table test runs every
observe and control feature (network, route, mock, console, errors,
emulate, time, determinism, coverage, text, wait) against each target
and passes. New features must add a row.

### Track 2 — Hardening and upkeep

**Why.** v1 trusts the page and the local user more than it should,
and some parts hold too much in memory. None of this is visible when
it works; all of it is a bad day when it does not.

- **Hostile pages.** `page-world-reports` (a page can fake what `goto`
  reports), `mock-shim-binding-trust`, `download-quota`,
  `connect-autoattach-scope`. ★ A "hostile page" fixture set that tries
  each trick (spoofed title and timings, binding floods, a download
  bomb, a 1 GB console log, `window.close` loops) with one test each.
- **Local surface.** `rpc-line-limits`, `mock-serve-cors-host`.
- **Memory.** `har-stream-large-files`, the streaming half of
  `pdf-more-options`, `snapshot-baseline-memory`,
  `daemon-runtime-threads`.
- **State that should survive or be undoable.**
  `route-rules-daemon-upgrade`, `session-options-off`.
- **Engine correctness.** `check-first-click-miss`,
  `obscured-ref-always`, `actionability-emulated-viewports`,
  `dom-walker-layout-tables`, `snapshot-dom-engine-keys`.
- **Keeping up with Chrome.** `cdp-protocol-vs-installed-chrome`,
  `cdp-enum-unknown-raw-value`, `keys-table-regenerate-check`,
  `shrink-release-binary`. ★ A weekly job that runs the suite against
  Chrome Beta and Canary, regenerates the protocol from the newest
  stable and opens a pull request with the diff, so a Chrome change is
  found before users find it.

Done: the hostile-page suite passes; the daemon's RSS stays within
budget with a 500 MB HAR and a 200-page PDF; the Beta job has been
green for a month.

### Track 3 — Parity gaps and small commands

Plan: [14-parity-gaps](14-parity-gaps.md).
Status: DONE (plan 14, 14.1-14.7).

**Why.** The last ❌ rows of `docs/playwright-parity.md`, the 🟡 rows
agents or converted suites hit, and small commands the backlog asked
for. Each is small; together they remove "chrome-cdp can't do X" from
migrations.

- Locators: `has=`, `has-not=`, `has-not-text=`, `or`/`and`
  (`locator-has-filter`); a command that prints a ref's stable locator
  (`locator-for-ref`).
- Time: paused timers, `time run-for`, `time pause-at`
  (`clock-fake-timers`).
- `clipboard-rich-content`, `upload-directory-inputs`,
  `extensions-command`, `downloads-dir-option`, `open-init-script`,
  `eval-in-auto-wait`, `wait-for-binding-call`,
  `no-failed-requests-all-tabs`, `har-pages-per-navigation`, the
  header and footer half of `pdf-more-options`,
  `screenshot-annotate-top-layer`, `cdp-method-did-you-mean`.
- The 🟡 rows worth closing: `expose` functions that return a value;
  WebSocket frames in `network` (not only through `cdp --listen`);
  `keyboard down`/`up`; `expect-snapshot` templates with wildcards and
  regexes (Playwright's `toMatchAriaSnapshot`).

Done: `docs/playwright-parity.md` has no ❌, and the 🟡 rows above are ✅.

### Track 4 ★ — From page to source

Plan: [15-page-to-source](15-page-to-source.md).
Status: DONE (plan 15, 15.1-15.7; the agent benchmark's run is deferred).

**Why.** §1: a coding agent's next step after "the button is wrong" is
"which file do I edit?". Today it greps the codebase for the button's
text. The browser often knows the answer, and Playwright does not ask
it.

- **`source <loc>`: element → file and line.** In order of preference:
  1. The framework's own debug info in dev builds: React's fiber
     (`_debugSource`, or the owner stack in React 19), Vue's
     `__vueParentComponent.type.__file`, Svelte's `__svelte_meta`,
     Angular's `ng.getComponent`. Each adapter is a small injected
     script in the page's main world (the framework's properties on DOM
     nodes are not visible from an isolated world), and returns the
     component chain.
  2. The stack that created the node: `DOM.setNodeStackTracesEnabled`
     before load, then `DOM.getNodeStackTraces`, resolved through
     source maps. It works for any framework, and for none.

  Output: `e12 button "Add to cart" ← <CartItem> src/components/CartItem.tsx:42 ← <Cart> src/pages/cart.tsx:88`.
  `snapshot --source` adds the component to each ref'd node, compactly.
- **`components [loc]`**: the component tree with props and state
  (React, Vue, Svelte adapters), bounded like every output. For "why
  does this show 3 items?".
- **Logpoints: printf debugging without editing code.**
  `debug log src/cart.ts:42 'total, items.length'` sets a
  `Debugger.setBreakpointByUrl` whose condition logs and returns
  `false`, mapped through the source map. The values land in `console`
  and in the next action's report. `debug clear`. The agent learns the
  runtime value on the next click, without an edit, rebuild and reload.
- **Locals at an uncaught exception.** `errors --locals` (or a session
  option) pauses on uncaught exceptions, reads the top frames' scopes
  (`Runtime.getProperties`, bounded), resumes, and stores them with the
  error. `errors` shows `total = undefined, items = [3 items]` next to
  the source-mapped line.
- **Why a request was sent.** `network show <id>` prints the
  initiator's stack, source-mapped: `fetchCart() src/api.ts:12 ←
  onClick src/components/CartItem.tsx:57`.
- **Source-mapped action reports** (`report-sourcemapped-console-errors`).
- **Dev-server awareness.** Detect Vite's and webpack's HMR clients;
  `wait --hmr` waits for the next update to apply, and the action
  report says `! hot update applied (src/cart.ts)` or
  `! hot update failed: <error>`. After an edit the agent knows whether
  the page it sees has its change.

Done: `examples/` gets small Vite apps in React, Vue and Svelte. On
each, `source` names the right file and line for 10 chosen elements,
`debug log` shows a value set at a line, and `errors --locals` shows a
local of the throwing function. The agent benchmark gains tasks of the
form "fix the bug behind this broken button"; with Track 4 the agent
reads fewer files to get there (measured).

### Track 5 ★ — Seeing more of the page

Plan: [16-seeing-more](16-seeing-more.md).
Status: DONE (plan 16, 16.1-16.6).

**Why.** The AX tree tells an agent what is there. It does not say
what is *wrong* with how it looks, why an action would fail, or what
changed beyond one tab.

- **`why <loc>`: explain actionability now, without acting.** Visible?
  In the viewport? Covered, and by what (with its ref)? Disabled, and
  by what (`disabled`, a disabled `<fieldset>`, `aria-disabled`,
  `inert`, `pointer-events: none`)? Stable, or animating? Playwright only
  says this in a timeout's log. The pipeline exists (plan 05); this
  prints its verdict for every check.
- **`layout`: visual lint.** From `DOMSnapshot` boxes and computed
  styles, list: elements that overlap interactive ones; text clipped by
  `overflow: hidden` or cut by `text-overflow`; horizontal scroll at
  the viewport; interactive elements off-screen or under 24×24 px;
  low contrast measured on the rendered pixels, not on declared colors.
  `--viewports mobile,tablet,desktop` runs it at each size. `expect
  --layout` fails on new findings (with a baseline of accepted ones).
  Also `--stretch-text 40%`: temporarily lengthen text nodes, as a
  translation would, and report what breaks.
- **`snapshot` for big pages.** `--budget <tokens>` folds regions into
  stubs with ids (`[region r4: 38 nodes]`) and `snapshot --expand r4`
  opens one. Repeated structures fold into one example and a count
  (`… 47 more cards like e40`), with refs still resolvable.
- **`forms`**: each form as a unit: fields with labels, types, required,
  current value and validation message, and its submit button.
  `fill-form <loc> --json '{"Email": "a@b.c", "Country": "Norway"}'`
  fills by label, picking `fill`, `select`, `check` or `upload` by
  field type, and reports fields it could not match.
- ★ **`compare`**: two pages, one diff. `compare <url-a> <url-b>` (or
  two sessions, or a page and a saved snapshot) prints the semantic
  diff of their snapshots and, with `--visual`, the regions whose
  pixels differ. For "did my change alter anything besides what I
  meant?" and for "local vs production".
- ★ **`explore`**: map an app. Follow links and buttons on one origin,
  to a depth and a page budget, without submitting forms unless told.
  Output: the pages and states found, how to reach each (a `.cdp`
  path), and per page its console errors, failed requests and `layout`
  findings. For an agent new to an app, and as a smoke test.

Done: each command has fixture pages with known answers (a covered
button, a clipped label, a 60-card grid, a three-step form). `explore`
maps the e2e-shop example fully within its budget.

### Track 6 ★ — Page audits

Plan: [17-page-audits](17-page-audits.md).
Status: DONE (plan 17, 17.1-17.4).

**Why.** Chrome already checks a lot (its DevTools Issues panel:
mixed content, CSP violations, cookie problems, deprecated APIs, CORS,
form problems), and nothing shows it to an agent. Checks that exist
one by one (`--a11y`, `perf`, `expect --no-errors`) should also run
together.

- **`issues`**: Chrome's own findings from `Audits.issueAdded`, grouped
  and deduplicated, with the element's ref or the request's id. Kept
  per tab like `console`. The `Audits` domain needs generating.
- **`audit`**: one report over a page, sorted by severity: `issues`,
  accessibility, `layout`, console and network hygiene, security
  (mixed content, cookies without `Secure`/`SameSite`, missing CSP or
  HSTS on the document), and basic metadata (title, language,
  viewport). `--json` for tools; `expect --audit` with a baseline file
  of accepted findings, so a test fails only on new ones.
- **Performance budgets.** `expect --perf 'lcp<2500 cls<0.1 tbt<200'`,
  with `perf`'s numbers under the current throttling.
- **Accessibility.** `expect-a11y-more-rules`: more built-in rules, or
  axe-core once the license allows it.

Done: fixture pages with one known finding per category; `audit` finds
each and nothing on a clean page; `expect --audit` fails on a new
finding and passes with it in the baseline.

### Track 7 ★ — Tests that maintain themselves

Plan: [18-self-maintaining-tests](18-self-maintaining-tests.md).
Status: DONE (plan 18, 18.1-18.7; `editor-support-cdp` stays a note).

**Why.** v1 makes tests cheap to write. What costs over time is
keeping them working: broken locators after a redesign, flakes nobody
understands, and not knowing what the suite misses.

- **`test --heal`.** Each passing run stores a fingerprint per step's
  element (role, name, stable-locator candidates, place in the tree).
  When a locator later finds nothing, the runner looks for the element
  the fingerprint best matches, runs the rest of the test with it, and
  writes the fix as a patch to the `.cdp` file. It never edits files
  unless `--write`. The report says what was healed and how sure it is.
- **Flake diagnosis.** With `--repeat`, compare passing and failing
  runs of one test and name the likely cause: an action before the page
  settled, a request that raced, an animation, an unmatched mock, test
  order. Put it in `report.md`.
- **UI coverage.** `test --ui-coverage` records which refs' elements,
  routes and API requests the suite touched, and prints what it never
  touched per page (`/checkout: 3 of 11 buttons never clicked`). A
  suite's blind spots, in text.
- **Repro bundles.** `repro save <dir>` packs the session: history as a
  scenario, the traffic as recorded mocks, `state save`, the last
  snapshot and screenshots. `test <dir>` replays it offline. A bug an
  agent found can be handed to another agent, or committed as a
  regression test.
- **A recorder** (a v1 non-goal). `record` in a headed session writes
  what a human does as `.cdp` lines with stable locators, reusing
  `history --as-scenario`'s code.
- Runner backlog: `e2e-runner-baselines`, `runner-context-leftovers`,
  `secrets-in-screenshots`, `watch-data-files`, `saved-list-indexes`,
  `lint-after-load-errors`, `history-testid-attribute`,
  `convert-real-world-specs`, `editor-support-cdp`.

Done: the e2e-shop suite, run against a copy of the shop with renamed
classes and moved buttons, heals every broken locator with a correct
patch; a planted flake is diagnosed as its cause.

### Track 8 — Mocks

Plan: [19-mocks](19-mocks.md).
Status: DONE (plan 19, 19.1-19.4).

**Why.** Plan 11's mocks are the part of chrome-cdp furthest ahead of
Playwright. The backlog fills in its edges.

- Matching and data: `mock-multipart-forms`,
  `mock-collection-nested-paths`, `mock-record-request-bodies`.
- Streaming: `mock-record-websockets`, `mock-sse-send`,
  `mock-check-websockets`, `mock-serve-websockets`.
- `mock serve`: `mock-serve-upstream`.
- The example: `e2e-shop-hybrid-backend`.
- ★ **Mocks from the source of truth.** `mock gen --openapi spec.yaml`
  writes a mock set with example data for every operation, and
  `mock check` (which already checks mocks against a spec) runs in
  `test` so drift fails the build.

Done: every WebSocket and SSE feature has a record → edit → replay test,
and `mock gen` on the Petstore spec makes a set the example app runs
against.

### Track 9 ★ — Beyond one local browser

**Why.** Agents run in more places than a laptop with Chrome, and
sometimes need a human.

- **Human handoff.** `handoff [--until-url PATTERN] [--message TEXT]`
  moves the session into a headed window on the user's display (or
  opens DevTools' screencast for a remote one), waits until the human
  says done or the URL matches, and hands the session back to the agent
  with its cookies, storage and tabs. For CAPTCHA, 2FA and SSO, which an
  agent should never try to solve.
- **Live view.** `view` serves a read-only page on localhost that
  shows the session's tabs as a screencast with each command as it
  runs, for a human watching an agent.
- **Remote and cloud browsers.** `cdp-wss-support`, plus headers and
  tokens for `connect` to hosted browser services.
- **Electron apps.** `open --electron <app>` launches an Electron app
  with a debugging pipe; every command then works on its windows.
- **Android.** `connect --android [serial]` reaches Chrome and WebViews
  on a device or emulator through `adb forward`, with real touch
  input.
- **MCP server mode.** Plan 08 §8.4 has the design; the maintainer
  dropped it from v1. Revisit only if the agent benchmark shows agents
  that cannot use a shell losing out. The design keeps it behind a
  Cargo feature.
- **Many agents on one machine.** Sessions that share one browser, each
  in its own context, with a cap on browsers per user and cleanup of
  sessions whose agent went away.
- **Video, for humans.** `record-video start|stop -o demo.webm` from
  `Page.startScreencast`, for a pull request's "here it is working".
  ★ For agents: `filmstrip`, a single image of the keyframes where the
  page changed during the last steps, which an agent can look at once.
- **Windows** (a v1 non-goal). The code is kept portable; it needs a
  named-pipe transport for the daemon and a CI runner.

Done per item: its own acceptance test; Electron and Android each run
the e2e-shop suite (Android on an emulator in the nightly job).

## 4. Order

A suggested order. Each release is a minor version (semver, from 1.0).

| Release | Tracks | Why this order |
|---|---|---|
| 1.0.x | 0 | Measure and verify before adding. |
| 1.1 | 1, 2 (hostile pages, local surface), 3 | Track 1 is the base every later feature stands on. Security fixes should not wait. |
| 1.2 | 4, `why` from 5 | The biggest gain for coding agents (§1). |
| 1.3 | 5 (rest), 6 | Share one engine: `DOMSnapshot` boxes and styles. |
| 1.4 | 7, 8 | They build on 4 (fingerprints and sources) and 5 (`explore`). |
| later | 9, 2 (rest) | Each item stands alone; pick by demand. |

Tracks 2, 3 and 8 are made of independent small items and can fill
gaps in any release.

## 5. Not planned

- **Firefox and WebKit.** chrome-cdp is CDP. WebDriver BiDi would be a
  second engine with a smaller feature set; a different tool does that
  better.
- **A GUI trace viewer.** An agent cannot use it. `report.md`, the
  filmstrip and `trace` for DevTools cover humans.
- **Control flow in `.cdp` files.** Plan 08's rule stands.
- **Solving CAPTCHAs or evading bot detection.** Handoff (Track 9) is
  the answer.
- **Component testing.** Still out of scope (overview §8).

## 6. Backlog map

The LOW notes still open in `.looper/tasks/`, by track. Plans 13-19
did the rest (each plan names the notes it absorbed). The notes those
plans' tasks left are in `.looper/tasks/v1/`; they are not mapped here.

| Track | Notes |
|---|---|
| 0 | batch-benchmark-run, docker-apparmor-unconfined-sandbox, doctor-newer-release, doctor-on-macos, extensions-headed-and-macos, headed-highlight-inspect-verify, headed-viewport-window, latency-benchmark-fixed-host, locator-benchmark, mock-schema-versioned-url, readme-playwright-cli-column, report-brief-settle-busy-pages, route-overhead-benchmark, runner-unmatched-check-cost, soak-budgets-tune, tabs-headed-activate, tokens-real-world-corpus, with-deps-end-to-end-test |
| 2 | actionability-emulated-viewports, cdp-enum-unknown-raw-value, cdp-protocol-vs-installed-chrome, check-first-click-miss, connect-autoattach-scope, daemon-runtime-threads, dom-walker-layout-tables, download-quota, har-stream-large-files, keys-table-regenerate-check, mock-serve-cors-host, mock-shim-binding-trust, obscured-ref-always, page-world-reports, pdf-more-options (streaming), route-rules-daemon-upgrade, rpc-line-limits, session-options-off, shrink-release-binary, snapshot-baseline-memory, snapshot-dom-engine-keys |
| 7 | editor-support-cdp |
| 9 | cdp-wss-support |

## 7. Rules for every task

Plans 13 and later are run one milestone per task by an agent with no
earlier context. Each task names a plan and a milestone. Before writing
code, read docs/plans/00-overview.md, sections 2-4 of
[plan 08](08-agent-ux-output-scripting.md) (the output, error and command
conventions every command follows), the plan, and the code the milestone
touches. Build on that code instead of duplicating it.

- **Scope.** Do only the named milestone. Later milestones are separate
  tasks; leave clean extension points for them.
- **Plan upkeep.** If the plan is wrong or impossible, pick the best
  alternative and record it in the plan's Decisions log with a one-line
  reason. When the milestone is done, tick the checkboxes that are now
  true, update the plan's `Status:` line (e.g. `IN PROGRESS (13.1-13.2
  done)`), and commit the plan with the code.
- **Backlog notes.** §6 maps each note in `.looper/tasks/` to a track, and
  each plan names the notes its milestones absorb. Do the notes of your
  milestone and delete each file you finish. Also read the notes earlier
  tasks of the run left in `.looper/tasks/<run>/` and do the ones that
  belong to your milestone.
- **Stability.** v1's command line and `--json` output are stable
  ([docs/output.md](../output.md)). New commands and flags are fine; a
  schema may only grow, and no command or flag is removed or renamed.
  `cargo xtask release check` must pass.
- **Docs move with code.** A new or changed command updates its help text
  (with an example), its page in `docs/commands/`, the golden tests
  (`crates/chrome-cdp/tests/golden/`), `chrome-cdp skill` when an agent
  should know it, and `docs/playwright-parity.md` when a row changes.
- **CDP.** New domains or commands go in
  `crates/cdp/protocol/domains.txt`, then `cargo xtask gen-cdp`.
- **Tests.** Integration tests need a real Chrome: Chrome for Testing 154
  is at
  `~/.cache/chrome-cdp/browsers/stable-154.0.8037.57-linux64/chrome-linux64/chrome`;
  point `CHROME_PATH` at it. An AppArmor profile runs it with the sandbox
  on, so never pass `--no-sandbox`; "No usable sandbox" is a bug in the
  launch code. There is no sudo and no display: headed-only checks become
  a note. Add fixture pages to `testdata/site`. Tests run offline, so
  vendor what they need.
- **Benchmarks.** Do not run timing measurements, and never wait for the
  machine to go idle. Build a benchmark the plan asks for (an `#[ignore]`
  test or an `xtask bench` suite) without running it; leave its criterion
  unticked with "deferred: benchmarks run after all plans" in the
  Decisions log. Correctness checks and counts (such as output tokens)
  are fine.
- **Never** push, tag or touch GitHub.

**Closing out a plan.** Check every acceptance criterion and checkbox
against the code and by running the tests, including the checks the
plan's close-out names. Fix gaps that fit, tick what is true, set
`Status:`, and mark the track done in §3 of this plan.

## Decisions log

_(append here)_
- Track 4: the Vite apps are test fixtures in `testdata/apps-src` (built
  into `testdata/site/apps/dev`), not `examples/`: tests run offline
  from committed dev builds, and they are one cart app in five
  frameworks, not examples to copy. The agent benchmark's four
  find-the-code tasks record files read; running them (the "measured"
  part) is deferred: benchmarks run after all plans.
- Close-out of plans 13-19: every plan is DONE (plan 15 with its agent
  benchmark's run deferred). §6 keeps only the notes whose files are
  still in `.looper/tasks/`; `pdf-more-options` left Track 3, since its
  templates half was done in 14.6 and the note is now only the
  streaming half (Track 2).
