# Plan 09 — Assertions & test mode

Status: DONE (9.1, 9.2 and 9.4; 9.3 moved to plan 11)
Depends on: 05, 08 (batch/run)
Note: the test runner (formerly 9.3) is in plan 11.

## Goal

Agents often use Playwright to *verify* their work ("does the page still
render the form after my change?"). Give them first-class assertions that
auto-retry, clear failure output, and a lightweight script runner that can
live in a repo and run in CI. This is not a full test framework (overview §8).

## Milestone 9.1 — `expect`

`chrome-cdp expect <loc> <matcher> [args] [--timeout 5s] [--not]`

Matchers (retrying until the timeout, like Playwright web-first assertions):

| Matcher | Example |
|---|---|
| `visible` / `hidden` / `attached` / `detached` | `expect e6 visible` |
| `enabled` / `disabled` / `editable` / `checked` / `unchecked` / `focused` | |
| `text <str\|/re/>` (substring; `--exact`) | `expect 'role=alert' text "Saved"` |
| `value <str>` / `values <a> <b>` | |
| `attr <name> [value]`, `class <name>`, `css <prop> <value>` | |
| `count <n>` (`--min`/`--max`) | `expect '.row' count 3` |
| `in-viewport` | |

Page-level:
`expect --url <glob|/re/>`, `expect --title <str>`,
`expect --no-errors [--since mark]` (no uncaught exceptions or console
errors), `expect --no-failed-requests [--origin self]`,
`expect --a11y [--level A|AA]` (runs an embedded axe-core in the isolated
world; MPL-2.0, so check the license and record the decision; otherwise
implement a smaller built-in rule set).

`mark <name>` sets a named point in the console/network cursors, so
`--since mark` works.

Failure output (exit 1, `ExpectFailed`):

```
expect failed: role=alert text "Saved"
  expected: text containing "Saved"
  actual:   "Error: email is required" (after 5.0s, 12 retries)
  element:  alert [e31]
  hint: chrome-cdp errors   (1 console error since last action)
```

## Milestone 9.2 — Visual & snapshot assertions (stretch)

- `expect-snapshot <name> [--scope loc]`: compare the `snapshot -i` text with
  a stored baseline in `.chrome-cdp/snapshots/<name>.txt`. The output is a
  unified diff. `--update` rewrites the baseline. Text baselines are robust
  and cheap, so they suit agents better than pixel diffs.
- `expect-screenshot <name> [--threshold 0.1]`: a pixel diff baseline
  (`image-compare` crate). It is marked experimental because fonts render
  differently on macOS and Linux; baselines are per platform.

## Milestone 9.3 — Test runner → moved to plan 11

The test runner, scenario files, browser-context isolation, reporting and
mocked/real backends are specified in
[plan 11](11-e2e-testing-mocked-backend.md). Do not implement a runner here.

## Milestone 9.4 — Playwright migration aid (stretch)

`chrome-cdp convert <spec.ts>` is a best-effort translator for simple
Playwright tests (`page.goto`, `getByRole().click()`, `fill`, `expect(...)
.toHaveText`, …) into `.cdp` files. It uses tree-sitter to parse, and leaves
`# TODO` lines for anything it doesn't understand. Include it only if cheap;
otherwise document the mapping table in `docs/migrating-from-playwright.md`,
which is required either way.

## Tests

- A matcher × (pass, fail, eventually-pass) matrix against the fixtures, plus
  golden failure messages.

## Acceptance criteria

- [x] Every failed expect includes actual vs expected and a hint.
- [x] `docs/migrating-from-playwright.md` covers every API in the parity
      matrix.

## Decisions log

_(append here)_
- 9.1: `--a11y` is a built-in rule set (`injected/src/a11y.ts`), not axe-core. axe's minified bundle is about 550 KB (a quarter of the binary), the project's own license is not chosen yet (MPL-2.0 is file-level copyleft, so shipping it needs that decision and a NOTICE), and it could not be fetched and reviewed offline. The rule set has 11 rules with axe's ids so results read the same: `document-title`, `html-has-lang`, `image-alt`, `input-image-alt`, `button-name`, `link-name`, `label`, `frame-title`, `aria-hidden-focus` (level A), and `meta-viewport`, `color-contrast` (AA). Contrast is computed from styles only (text over its ancestors' background colours); text over a background image is skipped. `--level AA` is the default.
- 9.1: `expect <locator> --a11y` checks only that element's subtree (the page rules are skipped); not in the plan, and cheap.
- 9.1: Element matchers and `--title` reuse the wait engine's probe: a `Condition::Expect` runs `__ccdp.expectElements`/`expectPage` on every animation frame, so navigation, frames, refs re-resolved by role+name, dialogs and crashes behave as in `wait`. `--url` watches the frame tree like `wait --url`. The default timeout is 5s (Playwright's), not `wait`'s 10s.
- 9.1: `--no-errors`, `--no-failed-requests` and `--a11y` look once instead of retrying: errors and failed requests do not go away by waiting, and retrying a failing audit only delays the failure. `wait --idle` first when requests are still running. `--not` goes only with element matchers, `--url` and `--title` (a usage error otherwise).
- 9.1: Without `--since`, `--no-errors` and `--no-failed-requests` look at what happened since the main frame's current document committed (recorded on `Page.frameNavigated`), so an earlier page's errors do not count. The document's own request comes before that point and is not counted (`goto` already reports its status). Chrome's automatic `/favicon.ico` request never counts: a missing one fails nearly every page.
- 9.1: `mark <name>` is per tab (the active one, or `--tab`), and setting a name again moves it. It is not a page command (it needs no script and does not move `wait --request`'s "previous command" mark). It records the console's, the network tracker's and the network log's positions, so `console`, `errors` and `network --since <name>` read from a mark too (`Since::Mark`, resolved in the engine). A name starts with a letter, then letters, digits, `-`, `_` or `.`, and is not `last` or a duration, which `--since` reads as such.
- 9.1: Matchers that look at one element (all but `attached`, `detached` and `count`) fail when several match, as Playwright's strict mode does, but as `ExpectFailed` naming the first matches with refs, and the hint is the same `expect` on the first ref. `--not` does not pass them either, nor a matcher that cannot apply (`value` on a `<p>`, `checked` on a non-checkbox, no element for `text`). No element passes `hidden`, `detached` and `count 0`, and fails `visible`.
- 9.1: `text` compares the element's rendered text (`innerText`, as `chrome-cdp text` reads it; `textContent` for an element that is not rendered), spaces collapsed and case counting. A value written `/re/flags` is a JavaScript regex; a bad one is `BadArgs` at once. `value`, `attr` and `css` compare the whole value; `css` colours compare as colours (`red` is `rgb(255, 0, 0)`). `values` compares a multiple select's values in any order. `class` checks that each space-separated class is present. `count` takes `N`, `--min` and `--max`. `--title` follows `text`'s rules.
- 9.1: The failure text follows plan 08's error form, so it is `error: expect failed: …` with `expected:`, `actual:` and `element(s):` notes (not column-aligned), and details carry the raw values. `actual` ends with `(after 5s, 12 retries)`. For a retried assertion, unread console errors come first in the hint (`chrome-cdp errors   (1 console error not shown yet; …)`); otherwise the hint is the read command that shows what was compared (`text e31`, `value e31`, `attr e31 class`), the first match, a snapshot when nothing matched, or the same `expect` with a longer timeout.
- 9.1: A passed `expect` prints `passed: <assertion> (12ms)`; `-q` prints the value compared (the text, a count), so `$(chrome-cdp expect .row count --min 1 -q)` is the count. The golden files mask the retry counts (`<n>`), which vary from run to run.
- 9.2: The pixel diff is our own (`engine/src/expect/pixels.rs`: pixelmatch's YIQ colour distance, pixels blended over white by their alpha) on the `png` crate already in the tree, not `image-compare`, which brings the `image` crate and its codecs for one comparison. It has no anti-aliasing detection: a baseline is per platform and taken with the same Chrome, which renders the same pixels each time. `--threshold` defaults to 0.1 (pixelmatch's, as written above; Playwright's is 0.2), and `--max-diff-pixels N` (default 0) lets N changed pixels pass.
- 9.2: A snapshot baseline is the `snapshot -i` lines without the header (its tab name and URL, with the test server's port, vary) and without refs (they vary per session); long lists are not collapsed, so a change past the cut shows. `--full` stores the whole tree (`snapshot --all`), `--scope` a subtree. Line ends and spaces at line ends in the file do not count.
- 9.2: Both commands read the page again every 100ms until it matches or `--timeout` (5s, like `expect`) passes. `--update`, and a first run with no baseline, store the page once two reads in a row are alike, or the last read with a warning when the page never settles. Reading counts as the tab's last snapshot for `snapshot --diff`.
- 9.2: Baselines live in `.chrome-cdp/snapshots/<name>.txt` and `.chrome-cdp/screenshots/<name>-<os>.png` (one per platform, like Playwright's suffix) of the nearest directory up with `.chrome-cdp.toml`, `.chrome-cdp/` or `.git`, else the current one; `--dir` puts them elsewhere (plan 11 can keep them next to scenario files). A name is words of letters, digits, `-`, `_` and `.` with `/` for folders, and a typed `.txt`/`.png` is not doubled. The CLI resolves the path; the daemon reads and writes the file, as with `screenshot -o`.
- 9.2: A missing baseline is stored and the command passes (`created`), as jest and insta do, so an agent's first run is not a failure. With the `CI` environment variable set (not empty, `0` or `false`) it fails instead (`ExpectFailed`, hint `--update`): in CI it would pass without checking anything.
- 9.2: A snapshot failure prints the unified diff (`- baseline`, `+ page`) as a block note: `ErrorInfo` notes may now hold several lines, printed indented under their label. It is cut at `--max-chars`; `details.diff` has all of it. A screenshot failure saves the screenshot and a diff image (the page faded, changes in red) where `screenshot` saves its files, so the baselines' directory only ever holds baselines to commit.
- 9.2: `expect-screenshot` captures at CSS scale with no `--max-size`, so a baseline does not depend on the device pixel ratio. It stills the page for each capture as Playwright does (a style hiding the caret in every frame, finite animations and transitions finished, endless ones cancelled and played again afterwards); `--mask` covers the rest (added at close-out). `--update` over a page whose changes are all under `--threshold` leaves the file alone (`unchanged`).
- 9.2: The pass line is `passed: expect-snapshot login: 7 lines as in the baseline (12ms)`; `-q` prints the baseline's path. `expect-screenshot` counts as fixed-size output (no `--max-chars`): it prints one line, and a failure's notes are paths and counts.
- 9.4: `convert` is included, with its own small JavaScript reader (`crates/chrome-cdp/src/convert/`: a lexer, a forgiving parser of statements, member-chain calls, literals and arrow functions, and the translator) instead of tree-sitter. The TypeScript grammar is a large generated C parser that needs a C toolchain and adds megabytes to the binary, and a test's calls need none of it. What it cannot read becomes a TODO line, so nothing is rejected.
- 9.4: `convert <spec> [-o file]` writes one script: each test is a section after a `# test: describe > title` comment, with the `test.use` options (as emulation commands), then the `beforeEach` steps, the test's, and the `afterEach` steps; `beforeAll`/`afterAll` go once around them, and skipped tests are a `# skipped:` comment. The tests run one after another in one session, so a route added in `beforeEach` is added again for each test (harmless: the first rule answers). Plan 11 can emit its `@test` directives here instead.
- 9.4: A statement it cannot translate becomes `# TODO line N (why, often with the command to use): code`, the source on one line; `--json` lists them in `todos`, and a warning counts them. It also follows locators kept in variables, `process.env.X` (as `${env:X}`), string constants, `const p = page.waitForResponse(…)` then `await p`, and `Promise.all([wait, action])` (the action, then the wait, since `wait --response` looks back to the previous command).
- 9.4: The migration guide maps every row plan 10's parity matrix names (and the rest of Playwright's page, locator, context and expect APIs) with the same ✅/🟡/❌/➖ marks; `docs/playwright-parity.md` itself stays plan 10's.
- 9.1 (close-out): the retry count sums the probe's tries over every document it ran in, so a navigation mid-assertion no longer restarts it from zero. A run whose document went away cannot report its count, so it counts one try per 100ms it ran (the probe's slowest pace), a lower bound.
