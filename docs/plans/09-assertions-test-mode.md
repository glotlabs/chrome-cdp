# Plan 09 — Assertions & test mode

Status: TODO
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

- [ ] Every failed expect includes actual vs expected and a hint.
- [ ] `docs/migrating-from-playwright.md` covers every API in the parity
      matrix.

## Decisions log

_(append here)_
