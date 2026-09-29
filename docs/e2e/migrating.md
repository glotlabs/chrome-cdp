# Migrating Playwright tests and `page.route` mocks

This page moves a `@playwright/test` suite, and the `page.route` mocks in
it, to `chrome-cdp test`. For single commands (locators, actions,
assertions), [../migrating-from-playwright.md](../migrating-from-playwright.md)
has the full tables, and `chrome-cdp convert spec.ts` translates a spec
file into a scenario.

## The shape of a suite

| `@playwright/test` | `chrome-cdp test` |
|---|---|
| `tests/checkout.spec.ts` | `e2e/scenarios/checkout.cdp` |
| `test('name', async ({ page }) => …)` | `@test "name"` and its indented steps |
| `test.describe('group', …)` | a file (tests can't nest) |
| `test.beforeEach` / `afterEach` | `@before` / `@after` |
| `test.beforeAll` / `afterAll` | `@before-all` / `@after-all` (in a context of their own; hand state over with `state save` and `state load`) |
| fixtures, helper functions | step files: `@include ../steps/sign-in.cdp email=ada@example.com` |
| `test.skip`, `test.fixme`, `test.only` | `@skip "why"`, `@only` |
| tags, `--grep` | `@tags smoke`, `--tags smoke,!slow`, `--grep` |
| `test.use({ viewport })`, `devices[…]` | `@viewport 390x844`, `@device "iPhone 15"` |
| `test.use({ storageState })` | `state load ../state/user.json` in `@before` |
| `test.setTimeout`, `retries` | `@timeout 60s`, `@retries 2`, `--retries` |
| `workers`, `--shard` | `jobs` in `[e2e]`, `--jobs`, `--shard 2/4` |
| `--repeat-each` | `--repeat 20` (and flaky tests are always reported) |
| `webServer` | `[e2e.web_server]` (`command`, or `static = "dist"` for a built app) |
| `baseURL` | `base_url` in `[e2e]`; `goto /path` |
| `page.clock` | frozen by default in every test; `time advance 5m` |
| reporters (`list`, `junit`, `json`, `html`) | `--reporter text,junit,json,html` |
| the trace viewer | `report.md` per failed test (for agents), `--reporter html`, `--debug` |
| `toHaveScreenshot`, `--update-snapshots` | `expect-screenshot`, `--update-snapshots` |
| `projects` over environments | backends: `--backend staging` ([backends.md](backends.md)) |
| Firefox, WebKit | Chromium only |

A test is a list of chrome-cdp command lines; there are no variables to
assign, loops or conditions. A test that computes values in TypeScript
becomes a scenario with `${var:…}`, `${data:file.json#/path}` or
`http … --save` values, or a mock that returns the right data.

## `page.route` handlers become mock files

In Playwright, mocks are callbacks registered in each test:

```ts
test.beforeEach(async ({ page }) => {
  await page.route('**/api/products', (route) =>
    route.fulfill({ json: products }));
  await page.route('**/api/products/*', (route) => {
    const id = route.request().url().split('/').pop();
    return route.fulfill({ path: `fixtures/products/${id}.json` });
  });
});

test('retries when the shop is busy', async ({ page }) => {
  let calls = 0;
  await page.route('**/api/orders', (route) =>
    ++calls === 1
      ? route.fulfill({ status: 503, json: { error: 'busy' } })
      : route.fulfill({ status: 201, json: { id: 1 } }));
  await page.goto('/checkout');
  await page.getByRole('button', { name: 'Place order' }).click();
  await expect(page.getByRole('alert')).toContainText('busy');
  await page.getByRole('button', { name: 'Try again' }).click();
  await expect(page).toHaveURL(/orders\/1/);
});
```

In chrome-cdp they are data: a mock set every test gets, and one layered
on for the test that needs it. The fixtures folder becomes the mocks'
`data/` folder, and the `:id` in a path picks the file.

```toml
# e2e/mocks/base.toml
[settings]
scope = ["/api/**"]

[[mock]]
name = "list products"
match = { method = "GET", path = "/api/products" }
respond = { json_file = "data/products.json" }

[[mock]]
name = "product by id"
match = { method = "GET", path = "/api/products/:id" }
respond = { json_file = "data/products/{{params.id}}.json" }

[[mock]]
name = "place order"
match = { method = "POST", path = "/api/orders" }
respond = { status = 201, json = { id = 1 } }
```

```toml
# e2e/mocks/busy.toml
[[mock]]
name = "orders: busy once"
match = { method = "POST", path = "/api/orders" }
sequence = [{ status = 503, json = { error = "busy" } }]
after_sequence = "unmatched"
```

```cdp
# e2e/scenarios/checkout.cdp
@test "retries when the shop is busy"
  @mocks busy
  @allow-console-errors  # the 503 is a failed load
  goto /checkout
  click 'role=button[name="Place order"]'
  expect role=alert text busy
  click 'role=button[name="Try again"]'
  expect --url **/orders/1
  expect request POST /api/orders --count 2
```

With `default_mocks = ["base"]` in `[e2e]`, every test gets `base`. The
`sequence` answers the first order with 503, then steps aside, and
`base`'s mock takes the retry.

| In a route handler | In a mock file |
|---|---|
| `route.fulfill({ json })` | `respond = { json = … }` or `json_file` |
| `route.fulfill({ status, headers, body, path })` | `status`, `headers`, `body`, `body_file`, `file` |
| `route.abort('failed')` | `respond = { abort = "failed" }` |
| a delay before `fulfill` | `delay = "2s"` (or `latency` for the whole set) |
| `route.fetch()`, change the JSON, `fulfill` | `passthrough = true` with `patch = { … }` |
| `route.continue()` | no mock (outside `scope`), or `passthrough = true` |
| reading `route.request()` (params, query, body) | `:id` paths, `query`, `json`, `form`, `graphql` conditions; `{{params.id}}`, `{{query.q}}`, `{{json.field}}` in answers |
| a counter in the closure | `sequence` with `after_sequence` |
| an array the handler pushes to and lists | a `[[collection]]` (`GET`, `POST`, `PUT`, `PATCH`, `DELETE` work) |
| a variable for "logged in" | `[state]`, `set_state`, `when` |
| `page.routeWebSocket` | `[[websocket]]` and `mock ws send` ([beyond-fetch.md](beyond-fetch.md)) |
| `routeFromHAR(file)` | `chrome-cdp mock import file.har` writes readable mock files |
| `page.waitForRequest`, checking `request.postDataJSON()` | `expect request POST /api/orders --json-body '{"qty":1}'` |
| a request no route handles goes to the network | it fails the test, naming the closest mock (`unmatched = "fail"`) |

While moving a suite, `unmatched = "passthrough"` in `[settings]` lets
unmocked requests through as Playwright did; switch it back to `fail` once
the mocks are complete, so a missing mock can never hide.

## Step by step

1. `chrome-cdp init e2e` in the app (it writes the config, a mock set and
   a CI workflow).
2. Move `webServer` and `baseURL` into `chrome-cdp.toml`.
3. Turn the route handlers into mock sets, as above; the files under
   your fixtures folder go to `e2e/mocks/data/`. `chrome-cdp mock
   explain GET /api/products/2` tells which mock answers a request.
4. `chrome-cdp convert tests/checkout.spec.ts -o e2e/scenarios/checkout.cdp`
   writes the scenario: a `@test` per test, the hooks as `@before` and
   `@after`, and `test.use` as settings. Replace its `route` lines with
   `@mocks` sets, and rewrite the lines it marks `# TODO`.
5. `chrome-cdp lint` checks the scenarios and mocks, `chrome-cdp test`
   runs them, and each failure's `report.md` says what to fix.
6. `chrome-cdp test --repeat 20` before deleting the Playwright suite.
