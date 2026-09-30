# Page audits: `audit` and `expect --audit`

`chrome-cdp audit` runs every check chrome-cdp has over the page and
prints one report, most severe first, then by category:

```sh
chrome-cdp audit                                 # every category
chrome-cdp audit --min-severity error            # errors only
chrome-cdp audit --category security,meta --min-severity info
chrome-cdp audit 'role=main' --category a11y     # inside one element
```

```
audit: http://localhost:5173/checkout — 2 errors, 5 warnings
errors
  a11y/image-alt                     img [e1] has no alt text  (WCAG 1.1.1 A)
  network/http-error                 GET /api/tax 404
warnings
  issues/deprecation-unload-handler  unload event listeners are deprecated; use pagehide  (/src/cart.js:2)
  layout/clipped                     paragraph "Continue to payment" [e2]: 60px of 206px shown (overflow: hidden on it)
  console/console-error              Cart failed to load  (/src/cart.js:3)
  security/no-csp                    the document has no Content-Security-Policy
  meta/no-viewport                   there is no <meta name=viewport>
hint: chrome-cdp audit --baseline checkout --update   (accepts these)
```

It waits for the page's `load` event first, then looks once. It exits 0
whatever it finds; [`expect --audit`](#expect---audit) is the check that
fails. `-q` prints the number of findings shown; `--json` has every
finding with its target (ref, stable locator, request id, cookie, frame)
and its key (`chrome-cdp schema audit` has the shape).

## Categories

| Category | What it checks |
|---|---|
| `issues` | Chrome's own findings since the page loaded (DevTools' Issues panel; see [`issues`](commands/issues.md)): mixed content, CORS and CSP blocks, cookie problems, deprecations, form problems |
| `a11y` | the built-in accessibility rules of `expect --a11y` at level AA ([accessibility.md](accessibility.md)), in the page and its iframes |
| `layout` | [`layout`](commands/layout.md)'s findings at the tab's viewport: covered, overlapping, clipped and too small elements, sideways scrolling, low contrast |
| `console` | uncaught exceptions and `console.error` calls since the page loaded |
| `network` | requests that failed and responses of 400 or more since the page loaded (not the browser's own `/favicon.ico`) |
| `security` | the page's scheme, its Content-Security-Policy and HSTS headers, its cookies, password fields and forms that post to `http:` |
| `meta` | `<title>`, `<html lang>`, the viewport meta tag, the description, robots `noindex` |

Findings of several categories about the same thing stay separate: an
image without alt text is `a11y/image-alt`, and Chrome may raise an issue
about it too.

The security checks treat this machine as secure, as browsers do:
`http://localhost`, `*.localhost`, `127.x.x.x` and `[::1]` are not
flagged as `http:` pages, and get no HSTS finding. The CSP and HSTS
checks read the document's response headers from the network log; on a
page chrome-cdp attached to after it loaded (`connect`), they are
skipped with a warning (reload the page for them). A CSP in a
`<meta http-equiv>` tag counts.

`--category` picks categories (comma-separated or repeated). A LOCATOR
checks inside that element what can be scoped: `a11y`, `layout`, and the
forms and password fields of `security`; the page-wide categories run
with it only when `--category` names them.

## Severities

Each rule has a fixed severity:

- **error**: breaks for some users or leaks data. Chrome issues that
  blocked something (CORS, a CSP block, mixed content that was blocked),
  WCAG level A violations, uncaught exceptions, failed requests to the
  page's own origin, a password field or a form posting to `http:`.
- **warning**: will break or degrade. Deprecations, cookie problems,
  WCAG level AA violations, `layout` findings, console errors, failed
  requests to other origins, a missing CSP, cookies without `Secure` on
  `https:`, without `HttpOnly` (for names like `sid`, `session`, `token`,
  `auth`) or without `SameSite`, a missing title, `lang` or viewport.
- **info**: worth knowing. Quirks mode, lazy-loaded images in view, a
  missing description, `noindex`, no HSTS.

`--min-severity error|warning|info` (default `warning`) sets the least
severe finding shown; the header counts the hidden ones. The command
reference has [every rule and its severity](commands/audit.md#rules-and-severities).

## Keys

Every finding has a key, `<category>/<rule>/<where>`, that stays the same
across runs, ports and reloads: it holds no refs, request ids, timings or
ports. `<where>` is:

- for an element: its stable locator (the one `history --as-scenario`
  writes, e.g. `role=button[name="Pay"]`), else its role and name
  (`img`);
- for a request: its method and URL without the query, the page's own
  origin as `self` (`GET self/api/tax`);
- for a script's line: the script's URL, the same way, and the line
  (`self/src/cart.js:3`, then a console error's message);
- for a cookie: `cookie <name>@<domain>`;
- for the page as a whole: the path of its URL (`/checkout`).

The same key twice is one finding with a count (`×3`).

## Baselines

A baseline is a JSON file of accepted findings, committed with the
project:

```json
{
  "version": 1,
  "accepted": [
    { "key": "security/no-csp//checkout", "note": "CSP comes with the CDN move" },
    { "key": "console/console-error/self/src/cart.js:3 Cart failed to load", "count": 1 }
  ]
}
```

`--baseline NAME` is `.chrome-cdp/audit/NAME.json` in the project
(`e2e/audit/NAME.json` under `chrome-cdp test`; `--dir` sets the folder
`audit/` is in); a name ending in `.json` is a file path.

- Findings the baseline accepts are hidden and counted (`7 accepted`);
  `--show-accepted` lists them, marked `(accepted)`.
- An entry without `count` accepts any number of its finding; with one,
  more than that is a new finding.
- Entries that match nothing any more, in the categories checked, are
  listed as `stale`, so the file shrinks as problems get fixed.
- `--update` writes the findings at `--min-severity` or above into the
  file: it adds new ones, drops stale ones, keeps notes, and raises a
  count to what it found. Entries of categories not checked stay.

`chrome-cdp schema audit-baseline` prints the file's JSON Schema (for
editors), and `chrome-cdp lint` checks the baselines of `e2e/audit/` and
`.chrome-cdp/audit/`: the JSON, its fields, the version, and keys of a
known category, each once.

## `expect --audit`

```sh
chrome-cdp expect --audit                              # no findings at all
chrome-cdp expect --audit --baseline checkout          # none the baseline does not accept
chrome-cdp expect --audit --min-severity error --category a11y,security
```

It fails when a finding at `--min-severity` (default `warning`) or above
is not accepted by the baseline, or, without a baseline, when there is
any. Like `audit`, it waits for `load` (failing if the page does not
load within `--timeout`), then looks once: issues and errors are
recorded, not polled. The failure is the report of the new findings:

```
error: expect --audit --baseline checkout failed: 1 finding the baseline does not accept
  page: http://localhost:5173/checkout
  found: 1 error (6 accepted)
  baseline: /home/me/shop/e2e/audit/checkout.json
  new:
    errors
      a11y/image-alt  img [e1] has no alt text  (WCAG 1.1.1 A)
  hint: chrome-cdp expect --audit --baseline checkout --update   (accepts them, if they are fine), or fix them
```

A stale entry is a warning, not a failure. A missing baseline behaves
like `expect-snapshot`'s: a local run writes it (and passes), and in CI
(`CI` set) it fails with `there is no baseline yet`. `--update` (or
`chrome-cdp test --update-snapshots`) rewrites it.

## In CI

Put one smoke scenario per page in the E2E suite, each ending in
`expect --audit`:

```
# e2e/scenarios/audit.cdp
@test "checkout has no new findings"
  goto /checkout
  wait --settle
  expect --audit --baseline checkout

@test "the account page has no new findings"
  goto /account
  expect --audit --baseline account --min-severity error
```

Run it once locally: each baseline is written to `e2e/audit/`, with what
the page has today. Review the files, add a `note` to what you accept
for now, and commit them. From then on, CI fails on anything new, and
the report says what. When a finding is fixed, its entry goes stale;
`chrome-cdp test --update-snapshots` (or `expect --audit … --update`)
drops it. [testing.md](testing.md) has more on `chrome-cdp test`, and
[ci.md](ci.md) on running it in a workflow.
