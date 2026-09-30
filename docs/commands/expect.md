<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp expect`

Assert something about the page, retrying until it holds: an element's state, text, value, count, …

```text
chrome-cdp expect [OPTIONS] <LOCATOR|--url <PATTERN>|--title <TEXT>|--no-errors|--no-issues|--no-failed-requests|--a11y|--perf <BUDGETS>|--layout> [MATCHER] [VALUE]...
```

Arguments:

- `<LOCATOR>`: The element(s) to check (any locator: e12, #id, text=…, role=…); with --a11y, where to check
- `<MATCHER>`: What to check: visible, hidden, attached, detached, enabled, disabled, editable, checked, unchecked, focused, text, value, values, attr, class, css, count or in-viewport
- `<VALUE>`: The matcher's values: text, value(s), attribute name and value, class, CSS property and value, or count; `-` reads one from stdin

Options:

- `--not`: The assertion must not hold (element matchers, --url, --title)
- `--exact`: `text` and --title: all of the text, not a part of it
- `--min <N>`: `count`: at least this many
- `--max <N>`: `count`: at most this many
- `--url <PATTERN>`: The page URL matches: a part of it, a glob (\* and \*\*) or /regex/
- `--title <TEXT>`: The page title contains this (all of it with --exact), or matches /regex/
- `--no-errors`: No uncaught exception or console error since the page loaded (or --since)
- `--no-issues`: No issue of Chrome's (see `issues`) of --min-severity or above since the page loaded (or --since)
- `--min-severity <SEVERITY>`: --no-issues: the least severe issues that fail it (one of `error`, `warning`, `info`)
- `--all-tabs`: --no-errors, --no-issues and --no-failed-requests: in every tab of the session (or --context), each since it opened, whatever it loaded since
- `--no-failed-requests`: No request failed or got a status of 400 or more since the page loaded (or --since)
- `--since <MARK>`: --no-errors, --no-issues, --no-failed-requests and `request`: only after this mark (see `mark`), not since the page loaded
- `--count <N>`: `request`: exactly this many such requests
- `--query <NAME=VALUE>`: `request`: the query has this parameter, `name=value` (a glob; repeatable)
- `--header <HEADER>`: `request`: it has this header, 'Name: value' (a glob; repeatable)
- `--json-body <JSON>`: `request`: its JSON body contains this, as a mock's match.json ("\*" any value, strings with \* are globs); `-` reads stdin
- `--origin <ORIGIN>`: --no-failed-requests: only requests to this origin; `self` is the page's
- `--a11y`: Check the page and its iframes (or LOCATOR's element) against built-in accessibility rules
- `--level <LEVEL>`: --a11y: the WCAG level to check (one of `A`, `AA`)
- `--perf <BUDGETS>`: Check `perf`'s numbers against budgets, all of which must hold: 'lcp&lt;2.5s cls&lt;0.1 tbt&lt;200', or good (the Web Vitals thresholds)
- `--perf-missing <WHAT>`: --perf: what a metric not measured yet (INP before an interaction) does (one of `fail`, `ok`)
- `--layout`: Check the page's layout (see `layout`) or LOCATOR's element: fail on error findings the --baseline does not list
- `--baseline <NAME>`: --layout: the baseline, &lt;dir&gt;/layout/&lt;NAME&gt;.txt (one finding key per line)
- `--update`: --layout: write the findings to the baseline instead of comparing
- `--dir <DIR>`: --layout: the baselines' directory (default: .chrome-cdp in the project)
- `--viewports <LIST>`: Check at each of these viewports: mobile, tablet, desktop or WxH, comma-separated
- `--only <KINDS>`: Check only these kinds, comma-separated: covered, overlap, clipped, truncated, overflow-x, offscreen, small-target, contrast
- `--full`: Look for covered elements and low contrast down the whole page, a viewport at a time, not only in view
- `--min-target <PX>`: The least width and height of a target, in CSS pixels (default 24)
- `--stretch-text <PCT>`: Check again with every text this many percent longer, and report what only that finds
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Checks the active tab (or --tab) again on every animation frame until the
assertion holds or --timeout (default 5s) passes, like Playwright's
expect(). A failure exits 1 (ExpectFailed) and says what it expected, what
it saw, the element (with a ref) and what to run next. --not inverts
element matchers, --url and --title. Needs a running session.

Element matchers (expect <LOCATOR> <MATCHER> [VALUE…]):
  visible, hidden, attached, detached, enabled, disabled, editable,
  checked, unchecked, focused, in-viewport
  text <TEXT>          its text contains TEXT (all of it with --exact);
                       case counts, spaces are collapsed
  value <V>, values <V…>   a field's value; a multiple select's values
  attr <NAME> [V], class <NAME>, css <PROPERTY> <V>
  count <N> (or --min/--max)   how many elements match
  A TEXT or V written /regex/flags is a regex. Matchers that look at one
  element fail when several match (add >> nth=0, or use a ref).

Page checks:
  --url, --title       retried like the matchers
  --no-errors          no uncaught exception or console.error since the
                       page loaded (or --since <mark>; see `mark`; or
                       --all-tabs: in any tab since it opened)
  --no-issues          no issue of Chrome's (`issues`) of severity warning
                       or error since then (--min-severity error|info;
                       --since, --all-tabs as for --no-errors)
  --no-failed-requests no request failed or got a 4xx/5xx since then
                       (--origin self: only to the page's origin; the
                       browser's own /favicon.ico does not count;
                       --all-tabs: in any tab since it opened)
  --a11y [--level A]   built-in accessibility rules (WCAG A and AA), in
                       the page and its iframes, or the element LOCATOR
                       names
  --perf BUDGETS       `perf`'s numbers within every budget, e.g.
                       'lcp<2.5s cls<0.1 tbt<200': <metric><op><value>,
                       op <, <=, > or >=; ttfb, fcp, dcl, load, lcp, inp,
                       tbt in ms (or 2.5s), cls a score, requests and
                       elements counts, transfer and heap in bytes (or kb,
                       mb, gb); `good` is lcp<=2500 cls<=0.1 inp<=200. Waits
                       for load, then reads once; the failure lists every
                       budget and the throttling. A metric not measured
                       yet (inp before an interaction) fails, unless
                       --perf-missing ok
From --no-errors to --perf, they look once: what they find does not go
away by waiting.
  --layout --baseline NAME   no error finding of `layout` (in the page, or
                       LOCATOR's element) that .chrome-cdp/layout/NAME.txt
                       does not list; retried like the matchers. --update
                       writes the findings to it; layout's --viewports,
                       --only, --full, --min-target and --stretch-text apply

Mock state (expect mock-state <NAME> <CHECK>), retried like the matchers:
  count <N> (or --min/--max)   items of a collection (or a list value)
  value <V>            the named value is V (JSON, else text)
  contains <V>         some item has V's fields (`{"title":"Milk"}`)

Requests sent (expect request <METHOD> <PATH>), from the network logs of
every tab of the session (or --context; or --tab), mocked or not:
  PATH                 a path glob (/api/orders/*, **/orders), a full-URL
                       glob, or *; METHOD may be * (any)
  --query k=v, --header 'Name: value', --json-body '{"qty":2}'
                       conditions, as a mock's match (values are globs)
  --count N | --min N | --max N   how many (default: at least 1)
  --since <mark>       only those sent after the mark
Retried until enough were sent; too many fails at once. `expect
no-request <METHOD> <PATH>` checks none was sent. A failure lists the
matching requests and the closest others, with the condition each fails.

Examples:
  chrome-cdp expect e6 visible
  chrome-cdp expect 'role=alert' text Saved
  chrome-cdp expect '#email' value alice@example.com
  chrome-cdp expect '.row' count 3
  chrome-cdp expect '.spinner' visible --not
  chrome-cdp expect 'role=button[name="Save"]' enabled --timeout 10s
  chrome-cdp expect css=h1 text '/^Welcome, \w+$/'
  chrome-cdp expect --url '**/dashboard'
  chrome-cdp expect --no-errors --since before-save
  chrome-cdp expect --no-failed-requests --all-tabs
  chrome-cdp expect --no-issues --min-severity error
  chrome-cdp expect --a11y --level A
  chrome-cdp expect --perf 'lcp<2.5s cls<0.1 tbt<200'
  chrome-cdp expect --layout --baseline cart --viewports mobile,desktop
  chrome-cdp expect mock-state todos count 4
  chrome-cdp expect request POST /api/orders --json-body '{"qty":2}' --count 1
  chrome-cdp expect no-request DELETE '/api/cart/**'
```

