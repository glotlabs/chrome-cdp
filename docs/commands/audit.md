<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp audit`

Audit the page in one report: Chrome's issues, accessibility, layout, errors, failed requests, security and metadata

```text
chrome-cdp audit [OPTIONS] [LOCATOR]
```

Arguments:

- `<LOCATOR>`: Check inside this element what can be scoped (a11y, layout, the forms of security): a ref (e12) or a locator that matches one

Options:

- `--category <CATEGORY>`: The categories to check (comma-separated or repeated); all by default, or with a LOCATOR those it can scope (one of `issues`, `a11y`, `layout`, `console`, `network`, `security`, `meta`)
- `--baseline <FILE|NAME>`: The accepted findings: a .json file, or NAME for &lt;dir&gt;/audit/NAME.json
- `--update`: Write the findings into the baseline (keeping its notes; dropping stale entries)
- `--dir <DIR>`: Where a baseline NAME is (default: .chrome-cdp in the project; e2e under `chrome-cdp test`)
- `--min-severity <SEVERITY>`: The least severe findings shown (one of `error`, `warning`, `info`; default `warning`)
- `--show-accepted`: Also list the findings the baseline accepts, marked (accepted)
- `--all-tabs`: Every tab of the session (or context)
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Runs every check chrome-cdp has over the active tab (or --tab) and prints
one report, most severe first, then by category:
  issues    Chrome's own findings (see `issues`) since the page loaded
  a11y      the accessibility rules of expect --a11y, level AA (A is an
            error, AA a warning)
  layout    `layout`'s findings at the tab's viewport
  console   uncaught exceptions (error) and console errors (warning)
  network   failed requests and 4xx/5xx responses (error from the page's
            origin, else warning)
  security  http: pages (not localhost), password fields and forms on
            http:, no Content-Security-Policy, no HSTS on https:, cookies
            without Secure, HttpOnly (session-like names) or SameSite
  meta      no <title>, no lang, no or zoom-blocking viewport meta,
            no description (info), noindex (info)
A LOCATOR checks inside that element what can be scoped: a11y, layout and
the forms of security (--category adds the others, page-wide). It waits
for the page's load event first.

Each finding has a key (`a11y/image-alt/#logo`, `security/no-csp//cart`)
that stays the same across runs, ports and reloads. --baseline accepts
findings by key: a JSON file (NAME is <dir>/audit/NAME.json, <dir> being
--dir or the project's .chrome-cdp; `chrome-cdp schema audit-baseline`
has its format). Accepted findings are hidden (--show-accepted lists
them), entries that no longer match are listed as stale, and --update
writes the current findings into the file, keeping notes. It exits 0
whatever it finds; expect --audit fails on findings the baseline does
not accept. -q prints the number of findings shown. docs/audits.md has
the details, and docs/commands/audit.md every rule with its severity.

Examples:
  chrome-cdp audit
  chrome-cdp audit --min-severity error
  chrome-cdp audit --category security,meta --min-severity info
  chrome-cdp audit 'role=main' --category a11y
  chrome-cdp audit --baseline checkout --update     # accept what it finds
  chrome-cdp audit --baseline e2e/audit/checkout.json --show-accepted
```


## Rules and severities

Every finding's key is `<category>/<rule>/<where>`. The `issues`
category's rules are Chrome's issues, with their severities (see
[`issues`](issues.md#rules-and-severities)); the `a11y` category's are
the rules of `expect --a11y` ([docs/accessibility.md](../accessibility.md)),
level A an error and level AA a warning. The other categories' rules:

| Category | Rule | Severity | What |
|---|---|---|---|
| `layout` | `covered` | warning | an interactive element whose centre something else covers |
| `layout` | `overlap` | warning | two interactive elements whose boxes overlap |
| `layout` | `clipped` | warning | text cut off by overflow: hidden or clip |
| `layout` | `truncated` | info | text cut on purpose, by an ellipsis or a line clamp |
| `layout` | `overflow-x` | warning | the page scrolls sideways |
| `layout` | `offscreen` | warning | an interactive element no scroll can reach |
| `layout` | `small-target` | warning | an interactive element under 24x24 px |
| `layout` | `contrast` | warning | text below 4.5:1 (3:1 when large) against the pixels behind it |
| `console` | `uncaught-exception` | error | an exception no code caught, since the page loaded |
| `console` | `console-error` | warning | a console.error call, since the page loaded |
| `network` | `request-failed` | error | a request that got no answer (net::ERR_…); a warning when not to the page's origin |
| `network` | `http-error` | error | a response with a 4xx or 5xx status; a warning when not from the page's origin |
| `security` | `http-password` | error | a password field on an http: page (not localhost) |
| `security` | `form-http-action` | error | a form that sends its data to an http: URL (not localhost) |
| `security` | `http-page` | warning | the page is served over http: (not localhost) |
| `security` | `no-csp` | warning | the document has no Content-Security-Policy (header or meta tag) |
| `security` | `cookie-no-secure` | warning | on an https: page, a cookie of its site without Secure |
| `security` | `cookie-no-httponly` | warning | a cookie named like a session (sid, session, token, auth) that scripts can read |
| `security` | `cookie-no-samesite` | warning | a cookie of the page's site without SameSite |
| `security` | `no-hsts` | info | an https: page without Strict-Transport-Security (not localhost) |
| `meta` | `no-title` | warning | the document has no &lt;title&gt; |
| `meta` | `no-lang` | warning | &lt;html&gt; has no lang attribute |
| `meta` | `no-viewport` | warning | no &lt;meta name=viewport&gt;: phones show the desktop layout, zoomed out |
| `meta` | `viewport-blocks-zoom` | warning | the viewport meta tag blocks zooming (user-scalable=no, maximum-scale under 2) |
| `meta` | `no-description` | info | no &lt;meta name=description&gt; |
| `meta` | `noindex` | info | a robots meta tag keeps search engines out (noindex) |
