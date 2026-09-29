# Plan 17 — Page audits

Status: NOT STARTED
Depends on: 13 (the target graph: issues come from every target), 16
§16.2 (`layout`, one of `audit`'s categories), 09 §9.1 (`expect`,
`--a11y`), 06 (`perf`, `console`, `network`)

## Goal

Track 6 of [plan 12](12-after-v1.md). Chrome already checks a lot of
things on every page: mixed content, CSP violations, cookie problems,
deprecated APIs, CORS failures, form problems. DevTools shows them in
its Issues panel. Nothing shows them to an agent. And the checks
chrome-cdp has (`--a11y`, `perf`, `expect --no-errors`) run one by one,
so nobody runs all of them.

This plan adds:

- `issues`: Chrome's own findings, per tab, like `console`.
- `expect --perf`: performance budgets on `perf`'s numbers.
- More built-in accessibility rules.
- `audit`: one report over a page, from all of the above plus security
  and metadata checks, and `expect --audit`, which fails only on
  findings that are not in a committed baseline.

Read plan 08 §2–§4 before any command: output is compact text by
default, bounded by `--max-chars`, with `--json`; errors teach.

## Design

### Where findings come from

| Source | What | Milestone |
|---|---|---|
| `Audits.issueAdded` | Chrome's issues | 17.1 |
| `perf` (`PerfData`) | budgets | 17.2 |
| `injected/src/a11y.ts` | accessibility rules | 17.3 |
| plan 16's `layout` | layout findings | used by 17.4 |
| console and network logs | errors, failed requests | used by 17.4 |
| the document's response and cookies | security | 17.4 |
| the DOM | metadata | 17.4 |

### A finding

Every source is turned into one shape, so `audit`, baselines and
`--json` treat them alike:

```
Finding {
  category: "issues" | "a11y" | "layout" | "console" | "network" | "security" | "meta",
  rule:     string,   // stable id, e.g. "mixed-content", "image-alt", "cookie-no-samesite"
  severity: "error" | "warning" | "info",
  message:  string,   // one line, plain words
  target:   { ref?, locator?, request_id?, url?, frame? },
  key:      string,   // the identity used by baselines; see below
  count:    u32,      // duplicates folded into this finding
  docs?:    string,   // a link, for Chrome issues and WCAG rules
}
```

`key` is what makes a finding "the same one" across runs. It must not
contain refs, request ids, timings or ports. It is
`category/rule/<where>`, where `<where>` is:

- for an element: its stable locator (the code `history --as-scenario`
  uses), else its role and name;
- for a request or a cookie: the URL without query, with the origin
  replaced by `self` when it is the page's own, or the cookie's name
  and domain;
- for a page-wide finding: the path of the page's URL.

### Severities

Fixed per rule, in a table in the code that the docs page is generated
from. The default mapping:

- `error`: things that break for some users or leak data. Chrome issues
  that block something (`BlockedByResponse`, CORS, CSP violations that
  blocked a load, mixed content that was blocked), a11y level A
  violations, uncaught exceptions, failed requests to the page's own
  origin, a password or login form on `http:`.
- `warning`: things that will break or degrade. Mixed content that was
  only upgraded or warned, deprecations, cookie problems, a11y level AA
  violations, `layout` findings, console errors, missing CSP on the
  document, cookies without `Secure` on `https:`.
- `info`: `QuirksMode`, `LazyLoadImage`, `Performance` issues, missing
  metadata that does not affect use (description).

`--min-severity error|warning|info` (default `warning`) filters output.

## Milestone 17.1 — `issues`

- Add `Audits` to `crates/cdp/protocol/domains.txt` (`Audits: enable,
  disable, checkFormsIssues, issueAdded`) and run `cargo xtask gen-cdp`.
- Register `Audits.enable` with plan 13's target graph for pages,
  OOPIFs and workers. Chrome replays issues raised before `enable`, so
  nothing is lost on attach; check that on a fixture that raises an issue
  during load.
- Keep issues per tab in a bounded ring buffer, like `console`
  (`crates/engine/src/observe/`), with the same `--since
  last|N|mark`, `--grep`, `--limit` and `-f/--follow` flags and the same
  reset on navigation rules as `console`. Reuse that code; do not copy it.
- Deduplicate: Chrome raises one issue per occurrence. Fold issues with
  the same code and the same `key` into one with a count.
- Map every `InspectorIssueCode` to a rule id (kebab-case of the code, and
  of the sub-reason where the details have one, e.g.
  `cookie-exclude-same-site-unspecified-treated-as-lax`,
  `mixed-content-blocked`), a severity (§Severities) and a one-line
  message built from the details. Codes not in the table (newer Chrome)
  become `issues/unknown-<code>` with severity `info` and the raw code in
  the message.
- Resolve targets: `backendNodeId`s in the details (e.g.
  `ElementAccessibilityIssue`, `GenericIssue`, form issues) get refs;
  `requestId`s get the network log's id and URL; cookie issues name the
  cookie.
- Call `Audits.checkFormsIssues` when `issues` runs, so form issues
  (autocomplete, labels) are current.
- Command: `chrome-cdp issues [--since …] [--grep RE] [--limit N]
  [-f] [--min-severity S] [--all-tabs]`.

Output:

```
3 issues (1 error, 2 warnings) since the page loaded
E mixed-content-blocked  http://cdn.example.com/app.js blocked on https page  ×1
W cookie-no-samesite     cookie "sid" (example.com) has no SameSite  ×4
W deprecation            unload event listeners are deprecated  (app.js:12)  ×1
```

- `expect --no-issues [--min-severity S] [--since MARK] [--all-tabs]`:
  fails with the list. Same shape as `--no-errors`.
- The action report gets a line when an action raised a new issue of
  severity `error`: `! issue: mixed-content-blocked …`.

## Milestone 17.2 — Performance budgets

`expect --perf '<budget> …'`, where a budget is `<metric><op><value>`,
`<op>` is `<`, `<=`, `>` or `>=`, and the value may have a unit (`ms`,
`s`, `kb`, `mb`; none means the metric's own unit).

Metrics, named after `PerfData`'s fields:

| Name | Field | Unit |
|---|---|---|
| `ttfb` | `navigation.ttfb_ms` | ms |
| `fcp` | `fcp_ms` | ms |
| `dcl` | `navigation.dom_content_loaded_ms` | ms |
| `load` | `navigation.load_ms` | ms |
| `lcp` | `lcp.ms` | ms |
| `cls` | `cls.value` | — |
| `inp` | `inp.ms` | ms |
| `tbt` | `tbt_ms` | ms |
| `requests` | `requests` | count |
| `transfer` | `transfer_bytes` | bytes |
| `elements` | `elements` | count |
| `heap` | the JS heap in `metrics` | bytes |

- Several budgets in one string, space-separated, all must hold:
  `expect --perf 'lcp<2.5s cls<0.1 tbt<200 transfer<1mb'`.
- `--perf good` is a shortcut for the Web Vitals "good" thresholds
  (`lcp<=2500 cls<=0.1 inp<=200`).
- A metric that is not measured yet (INP before an interaction, LCP
  before it fires) fails with a hint (`interact first`, `wait --load`),
  not as a pass. `--perf-missing ok` turns that into a pass.
- It waits for `load` first (within `--timeout`), then reads once. No
  retrying: numbers do not converge like a DOM does.
- The budget's grammar is parsed at CLI time and in `lint`, so a typo
  (`lpc<2500`) fails before the browser starts, with "did you mean".
- The failure lists every budget, held or not, with the measured value,
  and names the current throttling (`throttle`, `cpu-throttle`), since
  budgets only mean something under a known one.

```
expect failed: --perf 'lcp<2.5s tbt<200'
  lcp  3120ms  > 2500ms  ✗  (element: img.hero [e4])
  tbt   140ms  < 200ms   ✓
  throttling: network slow-4g, cpu 4x
```

In `.cdp` scenarios this is the same step; `@perf` is not a directive.

## Milestone 17.3 — More accessibility rules

`crates/engine/src/injected/src/a11y.ts` has 11 rules (plan 09's
decisions log explains why there is no axe-core: size, and the license
is not chosen). Add rules, built in, each with its WCAG criterion and
level, in the same `Rule` shape:

| Rule id | WCAG | Level | Checks |
|---|---|---|---|
| `aria-valid-role` | 4.1.2 | A | `role` values that are not ARIA 1.2 roles |
| `aria-valid-attr` | 4.1.2 | A | `aria-*` attributes that do not exist |
| `aria-valid-attr-value` | 4.1.2 | A | wrong values: bad tokens, ids that do not exist in `aria-labelledby`/`describedby`/`controls` |
| `aria-required-attr` | 4.1.2 | A | a role without its required attributes (`checkbox` without `aria-checked`, `slider` without value) |
| `aria-required-children` / `-parent` | 1.3.1 | A | `list`/`listitem`, `tablist`/`tab`, `menu`/`menuitem`, … |
| `duplicate-id-referenced` | 4.1.2 | A | an id used twice that a label, `for` or `aria-*` points to |
| `list` | 1.3.1 | A | `<ul>`/`<ol>` with children other than `<li>`, `<script>`, `<template>` |
| `table-headers` | 1.3.1 | A | a data table (not a layout table, by Chrome's AX role) with no header cells |
| `heading-order` | 1.3.1 | A | a heading level skipped going down (h2 → h4) |
| `empty-heading` | 1.3.1 | A | a heading with no text |
| `landmark-one-main` | 1.3.1 | A | no `main`, or more than one visible |
| `autocomplete-valid` | 1.3.5 | AA | bad `autocomplete` tokens |
| `target-size` | 2.5.8 | AA | a pointer target under 24×24 CSS px without enough spacing |
| `focus-visible` | 2.4.7 | AA | an element that takes focus with no visible focus change (compare outline, box-shadow and border before and after focus, in the isolated world) |

- Use Chrome's computed role and name (`Accessibility.getPartialAXTree`
  or what `snapshot` uses) where a rule needs them, not a re-implemented
  name computation.
- Each rule gets a fixture in `testdata/site/a11y/` with passing and
  failing elements, and a test that it flags exactly the failing ones.
- Also run the rules in every frame, from plan 13 (expect-a11y-iframes
  should already be done there; if not, do it here).
- Axe-core stays a later decision. Record in the decisions log that it
  waits on `.looper/tasks/choose-license.md`, and keep the `Rule` table
  shaped so an axe runner could be added behind `--a11y-engine axe`.

## Milestone 17.4 — `audit` and `expect --audit`

`chrome-cdp audit [LOCATOR] [--category C,…] [--min-severity S]
[--baseline FILE] [--all-tabs]` runs every category over the page (or a
part of it, where a category can be scoped) and prints one report,
sorted by severity, then category.

Categories:

- `issues`: 17.1, since the page loaded.
- `a11y`: 17.3, level AA.
- `layout`: plan 16's `layout` at the current viewport (its findings
  map to `Finding` one to one).
- `console`: uncaught exceptions (error) and console errors (warning)
  since the page loaded.
- `network`: failed requests and 4xx/5xx; the page's own origin is an
  error, others a warning.
- `security`: from the main document's response and the cookie jar:
  the page is `http:` and not localhost (warning; error with a password
  field); no `Content-Security-Policy` (warning); `https:` without
  `Strict-Transport-Security` (info; localhost skipped); cookies of the
  page's site without `Secure` on `https:`, without `HttpOnly` when their
  name looks like a session (`sid`, `session`, `token`, `auth`), or
  without `SameSite` (warning); forms that post to `http:` (error).
- `meta`: no `<title>`, no `lang`, no viewport meta, a viewport that
  blocks zoom, no `<meta name=description>` (info), `noindex` on a page
  (info).

Findings from several categories about the same thing (Chrome's
`ElementAccessibilityIssue` and an a11y rule on one element) are kept
separate; the report may group them under the element.

Output:

```
audit: https://shop.test/checkout — 2 errors, 3 warnings (1 info hidden)
errors
  a11y/label          textbox [e12] "Card number" has no label  (WCAG 4.1.2 A)
  issues/cors         GET self/api/tax blocked: no Access-Control-Allow-Origin
warnings
  layout/clipped      button [e20] "Continue to pay…" text is cut
  security/no-csp     the document has no Content-Security-Policy
  meta/no-lang        <html> has no lang attribute
hint: chrome-cdp audit --baseline e2e/audit/checkout.json --update  (accept these)
```

### Baselines

A baseline is a JSON file of accepted findings, committed with the
project (default directory `e2e/audit/` under `chrome-cdp test`, else
`.chrome-cdp/audit/`, like `expect-snapshot`'s baselines):

```json
{
  "version": 1,
  "accepted": [
    { "key": "security/no-csp/checkout", "note": "CSP comes with the CDN move" },
    { "key": "issues/deprecation/self/app.js", "count": 1 }
  ]
}
```

- `audit --baseline FILE` marks accepted findings `(accepted)` and
  hides them unless `--show-accepted`; `--update` writes the current
  findings into the file (keeping existing notes).
- A baseline entry without `count` accepts any number of that finding;
  with `count`, more than that is a new finding.
- Entries that no longer match anything are listed as `stale` in the
  output, so a baseline shrinks as issues get fixed. `--update` drops
  them.
- A JSON schema for the file (`schema audit-baseline`) goes next to the
  mock schema, and `lint` checks baseline files.

### `expect --audit`

`expect --audit [--baseline NAME|FILE] [--category …] [--min-severity
S]` fails when there is any finding at or above the severity that the
baseline does not accept. With no baseline, any finding fails. It does
not retry (issues are recorded, not polled); it waits for `load` first.
The failure output is the `audit` report of the new findings only.
In the test runner, `--baseline NAME` resolves under the project's
audit directory, and a missing baseline behaves like
`expect-snapshot`'s: it is written on a local run, and fails under CI.

### Docs

A `docs/audits.md` guide: what each category checks, the rule table
with severities, baselines, and a CI recipe (`expect --audit` in a
smoke scenario per page). Link it from `docs/README.md` and mention
`audit` in `chrome-cdp skill` in one line.

## Acceptance criteria

- [ ] `issues` shows Chrome's issues for the page, its iframes (same-site
      and OOPIF) and its workers, deduplicated, with refs for elements
      and ids for requests.
- [ ] Every `InspectorIssueCode` in the vendored protocol has a rule id,
      severity and message; an unknown code is shown, not dropped.
- [ ] `expect --perf` parses every metric in the table with units, fails
      with every budget's measured value and the throttling, and treats a
      metric not measured yet as a failure with a hint.
- [ ] The 14 new accessibility rules each flag exactly the failing
      elements of their fixture, and none on a clean page.
- [ ] Fixture pages with one known finding per `audit` category: `audit`
      finds each, with the right severity, and finds nothing on a clean
      page.
- [ ] `expect --audit` fails on a new finding, passes with it in the
      baseline, and reports stale baseline entries; baseline keys are
      stable across runs, ports and reloads.
- [ ] Every new command and flag has help with an example, a
      `docs/commands/` page, golden tests, and additive `--json` schemas.

## Tests

- `crates/engine/tests/issues.rs`: a fixture per issue family the
  fixture server can trigger offline: mixed content (an `https` page
  loading `http`; use the fixture server's TLS origin with
  `--ignore-https-errors`, or `Security`-domain overrides), a CSP
  violation (a `Content-Security-Policy` header and an inline script), a
  CORS failure (the second origin without the header), a cookie without
  `SameSite`, a deprecation (`unload` listener), a quirks-mode page, a
  form issue (`checkFormsIssues`: an input with a bad `autocomplete`).
  Each in the main frame, in a cross-site iframe and, where it applies,
  in a worker.
- Dedupe: 50 identical cookie issues show as one with ×50.
- Issues raised before attach (during a `connect`) are replayed.
- `expect --perf`: parser unit tests (units, operators, `good`, typos);
  integration tests on a fixture with a slow LCP image (served with a
  fixture delay) and one with long tasks. Check pass and fail, not
  timings: budgets in these tests are far from the measured values.
- A11y: one fixture per new rule in `testdata/site/a11y/`, a test per
  rule, and the existing clean-page test extended to all rules.
- `audit`: one fixture per category with one known finding, and a clean
  page with none; the golden text output for one page; key stability
  (two runs on different ports give identical keys).
- Baselines: accept, count limits, stale entries, `--update` keeping
  notes, the schema, `lint` on a bad file, and `expect --audit` in a
  `chrome-cdp test` scenario with a local run writing the baseline and
  a CI run failing without one.
- Golden: `issues`, `issues --json`, `audit`, `audit --json`,
  `expect --perf` failure, `expect --audit` failure.

## Decisions log

_(append here)_
