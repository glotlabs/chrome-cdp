# UI coverage

Code coverage says which lines ran. What a suite of E2E tests misses is
easier to see from the other side: the buttons no test clicks, the fields
no test fills, and the mocks no request needs. `test --ui-coverage`
records that.

```
chrome-cdp test --ui-coverage
```

```
ui coverage: 7 pages, 57 controls, 13 acted on (23%), 3 more only checked
/products/:id 3 of 9 acted on; never: button "Sign out", link "Cart (0)", link "Sign in", …
/cart         1 of 9 acted on; never: button "Remove Wireless mouse", link "Check out", link "Cart (1)", …
/checkout     3 of 9 acted on; never: link "Cart (1)", link "Sign in" in banner, link "Sign in" in main, …
pages no test visited are not known: list them in [e2e] pages, or find them with `chrome-cdp explore`
mocks never used: base.toml:85 "gift cards"
ui-coverage.json: /home/me/shop/e2e/.results/ui-coverage.json
```

## Pages

A page is the URL's path as a template: numbers, UUIDs and long hex runs
become `:id`, so `/products/1` and `/products/2` are `/products/:id`.
When paths name things another way (`/products/red-shoes`), list the
app's pages in `[e2e]`:

```toml
[e2e]
pages = ["/", "/products/:slug", "/checkout", "/account/**"]
```

A `:name` or `*` segment matches any one segment, and a last `**` the
rest of the path. A path that matches no pattern keeps its template. With
the list, the summary also names the listed pages no test visited
(`never visited: /account/**`); without it, the run cannot know them,
and [`explore`](../commands/explore.md) can find them.

## Controls

The controls are the elements `snapshot -i` gives refs: buttons, links,
fields, checkboxes, tabs, menu items and the like. The runner reads them:

- after a step that changed the URL (`goto`, a link, a form, a
  `pushState`): all of the page's;
- after the other actions (`click`, `fill`, `press`, …): the ones they
  added, from `snapshot -i --diff` (a dialog, a menu), or from the
  action's own `--snap` diff when the config turns that on;
- at the end of each test, unless the last step already read the page.

That is at most one snapshot per step, in the test and its `@before` and
`@after`. A control's key is its page, role and name: `button "Pay"`.
When another control of the page has that role and name, the key adds
its container, the nearest landmark, form, region or dialog: `button
"Buy" in navigation`.

A control is **acted on** when a step acted on it (`click`, `fill`,
`select`, `check`, `hover`, …), and **only checked** when an `expect` or
`wait` read it but no step acted on it. Which element a step touched
comes from its [fingerprint](healing.md#fingerprints), so it works with
any locator; refs (`e12`) get none and do not count.

A name that changes with the app's state is another control:
`link "Cart (0)"` and `link "Cart (1)"` are two.

Each page's line names the controls no step acted on, the page's own
first (those few other pages show), so the header every page shares
does not fill every line. `ui-coverage.json` lists them all.

## Mocks and requests

Against the mocks, the run lists each mock of the sets the tests used,
with how many requests it answered over the whole run (the mock server's
too). `mocks never used` names those that answered none: a mock for a
state no test reaches, or one that no longer matches what the app sends.

Against a real backend (`--backend NAME`), the run lists the API
requests the pages sent (fetch and XHR), by method and path template:
`GET /api/products/:id ×8`.

## `ui-coverage.json`

The file in the artifacts folder holds everything; `--json` has the same
data as `ui_coverage`:

```json
{
  "controls": 57,
  "acted": 13,
  "checked": 3,
  "pages": [
    {
      "page": "/checkout",
      "tests": 3,
      "acted": 3,
      "checked": 0,
      "controls": [
        { "key": "button \"Place order\"", "role": "button", "name": "Place order", "acted": 2, "checked": 0 },
        { "key": "link \"Sign in\" in main", "role": "link", "name": "Sign in", "container": "main", "acted": 0, "checked": 0 }
      ]
    }
  ],
  "mocks": [
    { "set": "base", "name": "gift cards", "file": "/home/me/shop/e2e/mocks/base.toml", "line": 85, "at": "base.toml:85", "hits": 0 }
  ]
}
```

`acted` and `checked` count steps over every test, run and retry. With
`--jobs`, each test's results merge in the order of the tests, so the
file is the same whatever ran at once. `--backend all` takes one backend
at a time: run `--ui-coverage` with `--backend NAME`.

## What it costs

A snapshot after each action and navigation: steps take longer with the
flag, so it is off by default. The `ui_coverage_cost_per_step` benchmark
(`crates/chrome-cdp/tests/bench.rs`, run by `cargo xtask bench micro`)
compares the shop's steps with and without it.
