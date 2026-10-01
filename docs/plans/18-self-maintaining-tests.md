# Plan 18 — Tests that maintain themselves

Status: IN PROGRESS (18.1-18.5 done)
Depends on: 11 (the test runner, `history --as-scenario`, `mock record`), 13 (the target graph, for `record` in iframes)
Roadmap: track 7 of [plan 12](12-after-v1.md)

## Goal

Plan 11 makes E2E tests cheap to write. What costs over time is keeping
them working:

- a redesign renames a button or moves it, and ten tests fail on a
  locator that finds nothing;
- a test fails one run in twenty, and nobody knows why;
- nobody knows what the suite never touches;
- a bug found in an interactive session cannot be handed to someone
  else to replay.

This plan adds the tools for each: fingerprints and `test --heal`, flake
diagnosis, UI coverage, repro bundles, and a recorder. It also finishes
the runner's backlog notes and tunes `convert` on real suites.

Rules that hold for every milestone:

- **Nothing edits a committed file unless asked.** `--heal` writes a
  patch; only `--heal --write` applies it.
- **Every guess says how sure it is**, with the evidence, in text an
  agent reads (report.md, the text reporter) and in `--json`.
- The CLI and `--json` schemas only grow (docs/output.md). New fields
  are optional.

### Why this beats Playwright

| | Playwright | chrome-cdp |
|---|---|---|
| Broken locator after a redesign | The test fails; a person rewrites the locator | The report names the element it most likely is now; `--heal` writes the patch |
| Flaky test | Marked flaky after a retry | `--repeat` names the likely cause with evidence from the runs |
| What the suite misses | Code coverage at best | UI coverage: per page, the controls no test acted on, and mocks no test used |
| Handing over a bug | A trace file for a GUI | A folder with a scenario, mocks and state that `test` replays offline |
| Recording | `codegen` window (TypeScript) | `record` writes `.cdp` lines with the same stable locators as `history` |

## Design

### Fingerprints

A fingerprint describes the element a step's locator found, so it can be
found again when the locator no longer works.

- **Which steps.** Steps whose locator must find one element: actions
  (`click`, `fill`, `select`, `check`, `hover`, `drag`, `upload`, …),
  `wait <loc>` for `visible`/`attached`/`enabled`, and `expect <loc>`
  with a matcher that needs the element (`visible`, `enabled`, `text`,
  `value`, `attr`, …). Not: `count`, `hidden`, `--not`, and `expect`
  steps that expect nothing to be found. A step whose locator is a
  ref (`e12`) has no fingerprint; scenarios should not hold refs.
- **How it is taken.** The runner asks the daemon for it with the step
  (an optional field on the RPC request, not a CLI flag). The engine
  already resolves the element for the action; a new injected module,
  `injected/src/fingerprint.ts`, reads it from the same element before
  the action runs. The fingerprint comes back in an optional RPC
  response field that the CLI's renderers ignore.
- **What it holds** (version 1):
  - `role`, `name` (accessible name), `tag`, `type` attribute;
  - `testid` (with the project's `testid_attribute`), `id`, `label`,
    `placeholder`, `text` (the first 80 characters, normalized);
  - `classes`: up to 5 class names, generated-looking ones left out
    (hashes such as `css-1x2y3z`, `sc-…`);
  - `container`: the nearest named container (`dialog`, `form`,
    `listitem`, `row`, `region`, landmark) as role and name;
  - `index`: its place among elements of the same role in that
    container;
  - `page`: the URL's path as a template (numbers, UUIDs and long hex
    runs become `:id`);
  - `candidates`: the stable locators `stable.ts` finds for it (plan 11,
    11.8), best first.

  No boxes, positions or times: a fingerprint must not change between
  two runs of an unchanged app.
- **Where it is kept.** `e2e/fingerprints/<scenario path>.json`, next to
  `e2e/snapshots`, meant to be committed. One object per test name,
  then per step key `<line>:<locator as written>`, where the line is the
  step's line in the file that holds it (a step file for an `@include`d
  step, with the include's parameters in the key). Keys are sorted and
  the JSON is pretty-printed, so diffs are small.
- **When it is written.** After a test passes, its fingerprints are
  merged into the file, and the file is rewritten only if something
  differs. Parallel jobs send theirs to the run, which writes each file
  once at the end. Config: `[e2e] fingerprints = "auto" | "write" |
  "read" | "off"`; `auto` (the default) writes, except when the `CI`
  environment variable is set, where it only reads. Fingerprints of
  steps that no longer exist are dropped when their test passes.

### Healing

When a fingerprinted step fails because its locator found nothing (or,
under strictness, found more than one element), the runner looks for
the element the fingerprint best matches. This happens with or without
`--heal`; only what follows differs.

- **Matching.** A new injected function scores the page's elements
  (of the frames the step could reach) against the fingerprint. Signals
  and weights (tune them on the fixture suite, and record the final
  numbers in the Decisions log):

  | Signal | Weight |
  |---|---|
  | testid equal | 3 |
  | role equal | 3 |
  | name equal / similar (token and edit similarity ≥ 0.7) | 3 / 2 |
  | label, placeholder, text equal or similar | 2 each |
  | container role and name equal | 2 |
  | id equal | 1 |
  | classes (Jaccard) | 1 |
  | tag and type equal | 1 |
  | index in container equal | 0.5 |

  An element with another role than the fingerprint's gets at most half
  marks, unless both roles are in one family (`button` and `link`,
  `textbox` and `searchbox` and `combobox`). **Confidence** is the best
  score over the most it could have had, lowered when the runner-up is
  close: `confidence = score × min(1, (best − second) / 0.2 + 0.5)`,
  capped at 1. `high` is ≥ 0.8, `medium` ≥ 0.6, and below that there is
  no match.
- **Without `--heal`.** The test fails as today. `report.md` and the
  error's hint add the match: `likely the same element now:
  role=button[name="Add to basket"] (confidence 0.86: same testid,
  role and container; name differs)`, and suggest `--heal`.
- **With `--heal`.** A `high` match is used: the runner takes its best
  stable locator (`stable.ts`, as `history --as-scenario` does), runs
  the step with it, and goes on. A `medium` match is used only with
  `--heal=medium`. The step is marked `healed` in `steps.jsonl`, the
  text reporter and `--json` (`healed: [{file, line, from, to,
  confidence, reasons}]` on the test result).
- **The patch.** When a healed test passes, its changes go into
  `<artifacts>/heal.patch`: a unified diff of the scenario and step
  files, one hunk per changed locator. A test that fails later in the
  run, after a heal, contributes nothing and its report says the heal
  is suspect. A locator the file does not spell out (it comes from
  `${var}` or an `@include` parameter) cannot be patched: the report
  says where the value comes from and what it should be. Two tests that
  heal one step-file line differently are a conflict: neither goes into
  the patch, and both are reported.
- **`--write`.** `test --heal --write` applies the patch to the files
  (after the run, only if every hunk still applies) and prints the
  files it changed. Fingerprints of healed steps are rewritten under
  their new keys.

### Flake diagnosis

A test is flaky when its runs disagree: a retry passed (`--retries`),
or `--repeat N` had both outcomes. Today the runner keeps the first
failed run's artifacts and nothing else.

- **Keep every run.** With `--repeat` or a retry, each run gets its own
  folder, `run-01/`, `run-02/`, … under the test's artifacts folder.
  The request journal records when each request was sent and answered,
  relative to the test's start, and `steps.jsonl` already has each
  step's start and duration.
- **Compare.** For a test with passing and failing runs, find the
  failing step and compare the runs up to it. Each cause below has a
  rule and the evidence the report shows:

  | Cause | Evidence |
  |---|---|
  | **Acted before the page settled** | In failing runs, a request the step's element depends on was still open when the step started, or answered after it; in passing runs it was done. Names the request. |
  | **Requests raced** | Responses came back in a different order in failing and passing runs. Names the two requests. |
  | **Moving target** | The failing step's error is an actionability error (not stable, obscured, detached). Names the animating or covering element. |
  | **Different requests** | Failing runs sent a request passing runs did not (or the other way), or had an unmatched request. |
  | **Time or randomness** | Determinism is off for the test, and the failing assertion's actual value differs between failing runs (dates, ids). |
  | **Shared state** | Failures only with `--jobs` > 1, or only in runs after the first, or only after one other test. Real backends only, since mock state is per test. |
  | **Unknown** | None of the above. Shows the first step whose output differs between a passing and a failing run. |

  More than one cause can match; they are ranked by how many failing
  runs they explain.
- **Output.** `report.md` of a flaky test gets a "Flaky" section: `3 of
  20 runs failed at checkout.cdp:14 (click "Pay")`, the likely cause,
  its evidence, and a fix hint (`wait --response /api/cart` before the
  step, `@mocks` for the extra request, and so on). The text reporter
  prints the cause after the flaky line. `--json` adds `flake: {cause,
  confidence, evidence[]}` to the test result.

### UI coverage

`test --ui-coverage` records what the suite reached and what it touched.

- **Pages.** Keyed by the URL path template (as in fingerprints), or by
  `[e2e] pages = ["/products/:id", …]` when the project lists them.
- **Elements seen.** The runner reads `snapshot -i --json` (the
  interactive elements) when a step changed the URL, when a step's
  action report shows new nodes (a dialog, a menu), and at the end of
  each test; at most once per step. An element's key is its page, role
  and name, plus its container when that is not unique.
- **Elements touched.** Those a step acted on, and separately those an
  `expect` checked, by the same key (from the step's fingerprint).
- **Requests.** The mocks of the test's sets and whether any request
  matched each one; in real-backend mode, the method and path template
  of each API request.
- **Output.** `<artifacts>/ui-coverage.json` and a summary printed after
  the run, bounded like every output:

  ```
  ui coverage: 7 pages, 64 controls, 38 acted on (59%), 12 more only checked
  /checkout     3 of 11 acted on; never: button "Apply coupon", link "Terms", checkbox "Gift wrap", …
  /products/:id 4 of 6
  mocks never used: base.toml:31 "cart-empty", errors.toml:8 "rate-limited"
  ```

  With `--jobs`, the runs merge. Pages the suite never visited are not
  known; the summary says so (plan 16's `explore` can list them).

### Repro bundles

A bundle is a small project that replays what a session did, offline.

- **Commands.** `repro start [--scope GLOB]…` marks the start: it saves
  the session's cookies and storage (`state save`), notes the history
  position and the time, and starts keeping response bodies in scope
  (default: every request of the session's tabs, documents and scripts
  included). `repro save <dir> [--name NAME]` writes the bundle. Without
  `repro start`, `repro save` uses the whole history, an empty state and
  the network log as it is, and warns.
- **Layout.**

  ```
  <dir>/
    chrome-cdp.toml        # [e2e]: tests = ["repro.cdp"], mocks = "mocks",
                           #   unmatched = "fail", no web server, the session's viewport
    repro.cdp              # @test NAME: the history as a scenario (history --as-scenario)
    mocks/repro.toml       # every recorded request (mock record's conversion)
    mocks/data/…           # response bodies
    state.json             # the state at repro start
    evidence/
      snapshot.txt         # the last snapshot
      screenshot.png
      console.txt, errors.txt, requests.txt
    README.md              # what happened, the last error, versions, how to replay
  ```

- **The scenario.** `@before` loads `state.json`, freezes time at the
  moment of `repro start` and applies the session's emulation. The steps
  come from `history --as-scenario`, with one change: the last failed
  command, if any, is kept as the last step with a comment giving its
  original error. So the bundle's test fails the way the session did,
  and passes once the bug is fixed; then it is a regression test.
- **Size.** Bodies over 5 MB, and images and fonts over 1 MB, are
  replaced by a placeholder with a warning (`--max-body` changes it).
  The README lists what was left out.
- **Replay.** `chrome-cdp test <dir>` runs a folder that has its own
  `chrome-cdp.toml` as that project (check how
  `e2e::config::find_project_file` picks a project today, and record the
  rule in the Decisions log). Every request is answered from the
  bundle's mocks; nothing reaches the network.
- **Secrets.** `${secret:…}` values in the history stay placeholders,
  and the README lists the secrets the replay needs. Password fields'
  values are never written. `Authorization` and `Cookie` headers are
  left out of the mocks (`mock record` already drops volatile headers).

### Recorder

`record` turns what happens in the page into `.cdp` lines, whoever does
it: a person in a headed window, or anything else that sends input.
Because it listens to the page, not to chrome-cdp's commands, it can be
tested headless with input sent through `cdp Input.dispatchMouseEvent`
and friends.

- **Commands.** `record start [--name NAME]`, `record stop [-o FILE]`
  (prints or writes the scenario, `@test "NAME"` and its steps),
  `record status` (how many steps so far).
- **How.** An init script in the isolated world of every frame (the
  target graph of plan 13) listens in the capture phase and reports
  through a `Runtime.addBinding` binding. The locator is computed at
  event time, before the page reacts, with `stable.ts` and the project's
  `testid_attribute`.
- **Events to lines.**

  | Page event | Line |
  |---|---|
  | click on a checkbox or radio | `check LOC` / `uncheck LOC` |
  | other click, double click | `click LOC` / `dblclick LOC` |
  | typing into a field | one `fill LOC "value"`, when the field loses focus or another event comes |
  | password field | `fill LOC "${secret:PASSWORD}"` and a comment; never the value |
  | select change | `select LOC value…` |
  | file chosen | `upload LOC <name>` and a `# TODO: path` comment |
  | Enter, Escape, Tab, shortcuts that type nothing | `press KEY` |
  | a typed URL, back, forward, reload (headed) | `goto URL`, `back`, `forward`, `reload` |
  | a dialog closed | `dialog accept` / `dialog dismiss` |
  | a new tab or a switch to one | `tab new URL` / `tab N` |

  Lines are made relative to `base_url` when the project has one.
  `stop` ends with a `# add expect steps` comment: the recorder does not
  guess assertions.

## Milestone 18.1 — Runner backlog

Do the backlog notes filed under track 7, except `editor-support-cdp`
and `convert-real-world-specs` (18.7):

- `e2e-runner-baselines`: `run file.cdp` inside a project uses
  `e2e/snapshots` like `test`.
- `runner-context-leftovers`: relative output paths of steps (`state
  save`, `screenshot -o`, `pdf -o`, `har stop -o`, …) resolve in the
  test's artifacts folder.
- `secrets-in-screenshots`: mask fields whose value is a secret before
  `failure.png` and `--trace` screenshots, and say so in report.md.
- `watch-data-files`: `--watch` also watches `${data:…}` files and
  `state load` files.
- `saved-list-indexes`: `${saved.order.items.0.sku}` (grammar,
  docs/scenario-syntax.md and the resolver).
- `lint-after-load-errors`: `lint` reports step problems in files that
  also have directive errors.
- `history-testid-attribute`: the config's `testid_attribute` reaches
  the daemon's stable locators (the recorder and heal need it too).

## Milestone 18.2 — Fingerprints and `test --heal`

- `fingerprint.ts`, the RPC field, and storage in `e2e/fingerprints`
  with the `fingerprints` config key, as in "Fingerprints".
- Matching, the report.md hint without `--heal`, `--heal`,
  `--heal=medium`, `heal.patch`, and `--write`, as in "Healing".
- Docs: a "Healing broken locators" section in docs/e2e/, the `test`
  page, and a line in `chrome-cdp skill`'s E2E section.
- The redesign fixture: a scripted change of the e2e-shop app
  (committed as a patch or a script under `testdata/heal/`, applied to
  a temporary copy of what the shop serves) that renames CSS classes,
  changes some button and link texts slightly ("Add to cart" → "Add to
  basket"), moves buttons into other containers, and changes a few test
  ids but not all. It must also include one change heal must *not*
  accept (an element removed, with a similar one elsewhere) so the test
  checks that it fails with a low-confidence hint instead.

## Milestone 18.3 — Flake diagnosis

- Per-run folders and request timings in the journal.
- The causes, their rules and evidence, ranking, and the output, as in
  "Flake diagnosis".
- Planted flakes in `testdata/e2e`: one per cause (a mock with seeded
  random latency that sometimes answers after the step; two requests
  whose order varies; an animating button with animations not disabled;
  a request sent only on some runs; a date shown with determinism off).
  Each must be diagnosed as its cause with `--repeat 20` and a fixed
  seed, so the test is deterministic.

## Milestone 18.4 — UI coverage

- `test --ui-coverage`, the `pages` config key, the collection points,
  the keys, `ui-coverage.json`, and the summary, as in "UI coverage".
- Check what the extra snapshots cost in steps: build a runner-overhead
  comparison with and without the flag (an ignored benchmark; do not
  run it).

## Milestone 18.5 — Repro bundles

- `repro start` and `repro save`, the layout, the scenario rules, size
  limits, secrets, and `test <dir>` replay, as in "Repro bundles".
- Test: a session on the fixture site that ends in a failed `expect`;
  `repro save`; then `test <dir>` with the fixture server stopped fails
  at the same step with the same error; after changing the bundle's
  mock to the fixed answer it passes.

## Milestone 18.6 — `record`

- `record start|stop|status`, the init script and binding, and every
  row of the events table, as in "Recorder".
- Tests drive a fixture form headless through raw `cdp Input.*` calls
  (mouse, keys, `Input.insertText`), in the main frame and an iframe,
  and compare `record stop`'s output with an expected file. The
  recorded scenario must then pass with `test` (the round trip).
- Headed checks (a typed URL, a real window) cannot run on a machine
  without a display: file a `.looper/tasks/` note.

## Milestone 18.7 — `convert` on real suites

- `convert-real-world-specs`: vendor three to five small, permissively
  licensed Playwright suites (for example Playwright's own TodoMVC demo
  tests) into `testdata/convert/` with their license files; count the
  TODO reasons in `convert --json`; add translations for the most
  common ones; keep the rule that anything unclear becomes a TODO line,
  not a wrong command.
- A golden test per vendored suite, and a count of remaining TODOs in
  the Decisions log.

## Acceptance criteria

- [x] Two runs of an unchanged e2e-shop suite write byte-identical
      fingerprint files.
- [x] Against the redesigned shop (18.2's fixture), `test --heal` passes
      every test whose element still exists, `heal.patch` fixes each
      broken locator, and after `--write` the suite passes without
      `--heal`.
- [x] The removed element is not healed: its test fails, with the
      closest candidate and its low confidence in report.md.
- [x] Without `--heal`, every broken locator's report.md names the
      element it likely is now.
- [x] Each planted flake of 18.3 is diagnosed as its own cause, first
      in rank, with evidence naming the right request or element.
- [x] `test --ui-coverage` on e2e-shop lists every page the suite
      visits, the controls it never acted on, and the mocks it never
      used; the numbers match a hand count on two pages.
- [x] A repro bundle replays offline and fails at the same step with
      the same error; with the fix in its mocks it passes.
- [ ] `record` round trip: recorded fixture flows pass with `test`.
- [ ] No password or `${secret:…}` value appears in fingerprints,
      patches, bundles or recordings (a test searches for it).
- [ ] The track 7 backlog notes of 18.1 and 18.7 are done and their
      files deleted.
- [ ] `cargo xtask release check` passes: the CLI and schemas only grew.

## Tests

- Unit: fingerprint path templates and class filtering; the matcher's
  scores and confidence on hand-made fingerprints; patch building,
  conflicts and variables; flake rules on hand-made run records; UI
  coverage keys and merging; bundle size limits; recorder event
  coalescing (typing, then blur).
- Integration (real Chrome): fingerprints on e2e-shop; heal against the
  redesign fixture; the planted flakes; UI coverage on e2e-shop; repro
  save and offline replay; record round trips in a frame and an iframe.
- Golden: `test --heal` text output and report.md, the flaky section of
  report.md, the UI coverage summary, `record stop` output, the new
  commands' help and `--json` schemas.

## Decisions log

_(append here)_
- 18.1 `history-testid-attribute`: the attribute travels as
  `Request::testid_attribute` beside `root`, and both are now
  process-wide rather than thread-local, since `test` runs its jobs on
  threads of their own. `stable.ts` keeps writing an attribute other than
  `data-testid` as `css=[data-test="…"]`: that locator replays whatever
  the config says, and the candidate is found only with the attribute.
- 18.1 `runner-context-leftovers`: outputs go straight into the test's
  (or hook's, or try's) artifacts folder, not a subfolder, so their paths
  are the ones report.md lists. A relative `state load` looks for a file
  an earlier step wrote (the step's folder, the test's folder and its
  `try-N`/`run-N` folders, newest first, then the file's `@before-all`
  folder) before the current directory: that keeps `@before-all` →
  `@before` hand-overs and an `@after` that leaves a file for the retry
  working. Input paths (`upload`, a `state load` no step wrote) stay
  relative to the current directory.
- 18.1 `watch-data-files`: the data files are read off each prepared
  step (its `${data:…}` variables, relative to the step's file, and a
  literal `state load FILE`) when the watcher computes dependencies,
  rather than recorded by `e2e::scenario::Vars` as it resolves: the
  runner resolves `data:` at run time, after the watcher has its list.
- 18.1 `secrets-in-screenshots`: before `failure.png` and each `--trace`
  screenshot an isolated-world script marks inputs and text areas whose
  whole value is a secret, compared by length and FNV-1a hash so the
  secret is never in the script, and the screenshot masks
  `css=[data-chrome-cdp-secret]`. Partial values and iframes are not
  covered (a note).
- 18.1 `lint-after-load-errors`: `e2e::scenario::read` returns the
  scenario with its errors; a `@test` with problems keeps what was right.
  `no-expect` is left out for a file with load errors, whose missing
  steps may hold the `expect`. A syntax error still stops its file.
- 18.2 RPC: `Request::fingerprint` carries the step's locator (as the
  runner expanded it) and `Response::fingerprint` the answer; the daemon
  runs the call in `Engine::fingerprinting`, a task-local that
  `locate_one` fills the first time the call resolves that locator to one
  element (before an action acts). `expect` and `wait` resolve elements
  another way, so a passed call without one gets it read after. The
  client keeps the ask and the answer in thread-locals, like the history
  line. The JSON field `type` is `kind` in Rust.
- 18.2: the bundle reads features (`fingerprint.ts`: `fingerprint`,
  `healCandidates` of the elements of the fingerprint's role or family,
  tag, test id or id, `healStable`); the engine filters class names, makes
  path templates and scores (`engine::fingerprint::score`), so those rules
  have unit tests.
- 18.2 weights, tuned on the redesign fixture: test id 2 (not 3: with 3,
  a renamed test id with everything else equal scored 0.74, medium);
  role 3, of one family 1.5; name 3 / similar 2; label, placeholder and
  text 2 / similar 1.5, and not counted when equal to the name (a
  button's text is its name); container 2; id 1; classes 1 × Jaccard;
  tag and type 1 when both equal; index 0.5; another role capped at half
  marks. Similar is not one 0.7 score (`Add to cart`/`Add to basket` is
  0.69 by edit distance, 0.67 by words): edit similarity ≥ 0.7, or two
  thirds of the longer's words in the shorter (a word counts at edit
  similarity ≥ 0.8), or the shorter (3+ characters) as whole words of the
  longer. On the fixture: 0.84 for a new text with the test id kept,
  0.81 for a new test id, 0.24 for the removed link's closest.
- 18.2 files: `e2e/fingerprints/<scenario below its tests glob's
  folder>.json` (like the artifacts: `cart.cdp` is `cart.json`). Keys
  drop the quotes around a word; a step of another file starts with that
  file's path from the project root; the `@include` parameters in a key
  are those the locator uses. Heal looks a key up in the test's object,
  then in the file's other tests (a renamed test).
- 18.2: only runs against the mocks write fingerprints (a real backend's
  data changes), as baselines come from mock runs. `@before-all` and
  `@after-all` steps get none (they belong to no test); `@before` and
  `@after` count as each test's. A step has one fingerprint: `drag`'s is
  its FROM's.
- 18.2: matching looks in the frame the locator's `frame=` steps name,
  else the main frame, and a match in an iframe gets its `frame=` step.
  No iframe test yet (a note).
- 18.2 patch: `similar`'s unified diff with 3 lines of context, paths
  from the project root with `a/` and `b/` (`git apply` there). Changes
  closer than that share a hunk: separate hunks would overlap, which
  `patch` rejects. `--write` writes only when every change still applies
  to every file. Without `--write`, a healed test's new fingerprint stays
  under the old key, so the next run matches the new element at 1.0 until
  the file changes.
- 18.2: `--heal` with `fingerprints = "off"`, or with `--backend all`
  (which reports several runs), is a usage error.
- 18.2 `history-testid-locator-form`: heal and fingerprint locators run in
  the project's `test`, where its `testid_attribute` holds, so they write
  `testid=…` for any attribute; `history --as-scenario` keeps its form (the
  note stays for it and 18.6's recorder).
- 18.2 fixture: the shop's buttons got classes and test ids (a redesign
  needs something to rename), and its fingerprints are committed. The
  redesign is `testdata/heal/redesign.toml`: substitutions in a copy of the
  shop's minified `dist/`, so its names are the committed build's; the
  test names the substitution that stops applying after a rebuild.
  `testdata/e2e/scenarios/heal` and `testdata/site/e2e/heal.html` back the
  golden output (`crates/chrome-cdp/tests/golden/heal/`).
- 18.3 folders: runs keep the `run-1/`, `run-2/` names (a retry's
  `try-2/`) that docs/e2e/debugging.md and the tests already used, not
  `run-01/`: they were per run before this milestone, and a rename would
  move documented paths.
- 18.3 journal: each `requests.txt` line ends with `(sent +120ms, answered
  +132ms)`, from the test's start as `steps.jsonl` times are (from the
  network log's start time and duration). The mock server's journal has
  no answer times, so the rules read the browser's requests only.
- 18.3 rules: a step "acts" when it ends, since an action auto-waits for
  its element and so starts before the data in passing runs too.
  `unsettled` takes a request sent before the step started (a request the
  step sent is not one it depends on), open when it acted in failing runs
  and answered by then in every passing run; it looks at the actions up
  to the failing step and that step (a passed `wait`/`expect` cannot act
  early). `race` compares `fetch`/`xhr` requests only. Requests are keyed
  by method and URL with ids made `:id` (`engine::fingerprint_score::
  page_template`, also for query values), and Chrome's favicon fetch is
  left out (it comes in some runs only). `shared-state` is the "only in
  runs after the first" rule (passed until one run, failed from it on);
  "only with --jobs > 1" and "only after one other test" need a second run
  or other tests' timelines (a note). Confidence is a weight per cause
  times the share of failing runs it explains: moving target 0.95,
  unsettled 0.9, race and different requests 0.85, time 0.8, shared
  state 0.5, unknown 0; ties rank in the table's order.
- 18.3 planted flakes: the mocks' random latency and the pages' seeded
  random values are the same in every run (that is the determinism the
  runner promises), so they cannot make runs differ. The fixture page
  (`testdata/site/e2e/flaky.html`) instead seeds a PRNG with the run's
  number from `${test.id}`, and in a bad run sends `x-flake: slow`, which
  a header-matched mock answers late: same URLs in every run, the same
  runs fail every time. `goto` waits for the network to go quiet (up to
  5s), so the settle flake is a click whose request outlives the click's
  1.5s wait. The test turns `freeze_time` and `disable_animations` off
  for the clock and the sliding button.
- 18.3 output: `--json`'s test result has `flake: {cause, summary,
  confidence, evidence, hint, runs, failed_runs, at, step, others}`; the
  section goes into the report.md of the failure the test shows (a
  retried test's first try's).
- 18.4 reads: `snapshot -i --all --text-limit 0 --max-chars 0` after a
  step whose URL changed (a `Navigated` event, or the URL `goto`, `back`,
  `forward` or `reload` gives, unlike the last read's) and at the end of
  each test unless the last step was read; after other actions
  `snapshot -i --diff`, or the action's own `--snap` diff, whose added
  nodes count. A diff costs the daemon a full read; it is the "action
  report's new nodes" and keeps the JSON small. Two side effects: a
  scenario's own `snapshot --diff` compares with the runner's last read,
  and the refs in step outputs grow.
- 18.4 keys: the container is the nearest landmark, form, region or
  dialog in the `-i` tree (the only containers it keeps), not the
  fingerprint's, which can be a list item or row that `-i` drops. A
  touched element whose fingerprint container is no landmark, on a page
  with several alike controls, counts on the first. Names compare on 80
  characters, containers on 40 (what fingerprints keep). A touched
  element of an interactive role that no read saw is a control; one of
  another role (the text an `expect` checked) is not.
- 18.4 touches come from fingerprints, so `--ui-coverage` takes them
  with `fingerprints = "off"` too (and writes none). Failed tests'
  reads and touches count: they show what was reached.
- 18.4 mocks: `mock list` of the test's context after `@after`, plus the
  mock server's journal; hits add up over every test, run and retry.
  Requests (real backend only): fetch and XHR of `requests.txt`'s
  journal, by method and path template, query left out.
- 18.4 `pages` patterns: a `:name` or `*` segment matches one segment, a
  last `**` the rest; a path no pattern matches keeps its template.
- 18.4 summary: every page line says "acted on" (the plan's example
  drops it after the first); its never-acted controls are ranked by how
  few pages show them, so the header every page shares does not fill
  every line. `--backend all` refuses `--ui-coverage`, like
  `--record-mocks`. `ui-coverage.json` is masked like stdout.
- 18.4 benchmark: `fingerprints_and_ui_coverage_cost_per_step` (also the
  `fingerprint-overhead` note's fingerprints off/on comparison); its
  bounds, 50ms per step for `--ui-coverage` and 10ms for fingerprints,
  are its own, as the plan names none. Deferred: benchmarks run after
  all plans.
- 18.5 bodies: the daemon asks Chrome for a body when its request
  finishes loading (`Network.loadingFinished`) while a repro runs, and
  keeps it by session and request id (`observe::kept`, at most 512MB;
  more are counted, not kept); `har export` reads the store first. The
  bodies of the documents open at `repro start` are asked for right away,
  and its `repro` flag exports each tab's requests from its document open
  at the start (tabs opened later from their first), so the start page is
  in the mocks. The start state (cookies and storage) stays in the
  daemon's memory until `repro save` writes it; `repro save` does not end
  the recording.
- 18.5 scenario: the frozen time, locale, time zone and viewport go into
  the bundle's `chrome-cdp.toml` (`[e2e.determinism]`, `viewport`), not
  `@before` commands: the runner applies determinism before `@before`, so a
  `time freeze` there would freeze twice. `@before` only loads
  `state.json`. The config also sets `allow_console_errors = true` (the
  session's page was not checked for them) and `fingerprints = "off"`, and
  `base_url` is the session's, else the start page's origin.
  `unmatched = "fail"` is a mock set's setting, so it is in
  `mocks/repro.toml`'s `[settings]` with `scope = ["/**"]`, not in `[e2e]`.
- 18.5 the failed step: the last failed command that would be a step and
  did not fail with a usage error (`BadArgs`, `BadLocator`); commands after
  it are left out. The history now keeps a failed command's message with
  its notes (`HistoryEntry::message`: `expected: …`, `actual: …`), so the
  comment and README quote it. `mock` commands are left out of a bundle's
  test: its own mocks answer.
- 18.5 passwords: after a `fill` or `type` the daemon asks whether the
  focused element (through open shadow roots, in the isolated world) is a
  password input (`HistoryEntry::password`); the bundle's step reads
  `${secret:PASSWORD}`. A field in an iframe is not recognized (a note).
- 18.5 size: `--max-body` (5MB) is the limit; images, fonts and media get
  a fifth of it (1MB). A left-out body's mock keeps its status and says
  why (`Exchange::left_out`).
- 18.5 replay: `e2e::find_project_file` walks up from the current
  directory only; a path argument never chose the project. Now `test DIR`,
  with one path that is a folder holding a project file other than the
  current directory's project, changes into DIR before the config is read
  and runs with no paths, as `cd DIR; chrome-cdp test` would; relative
  inputs (`state load state.json`) then resolve there. Any other path list
  keeps the old rule.
