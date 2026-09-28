# e2e-shop: E2E tests with chrome-cdp

A small shop (Vite + React) with a complete `e2e/` folder: the scenarios,
the mocks that stand in for its API, and the vars that let the same
scenarios run against the real API. It is the acceptance suite of the
E2E runner, and a starting point to copy.

```
chrome-cdp test                      # against the mocks: no backend, no Node
chrome-cdp test --backend local      # the same scenarios against the real API
chrome-cdp test --repeat 20          # check for flaky tests before a commit
```

The built app (`dist/`) is committed, and `chrome-cdp test` serves it
itself (`static = "dist"` in `chrome-cdp.toml`), so a fresh clone needs
nothing but chrome-cdp. After changing `src/`, run `npm install` once
and `npm run build`.

## What it covers

| Scenario | What the mocks do |
|---|---|
| `products.cdp` | a list from a data file; a product page per id (`data/products/{{params.id}}.json`, a missing file is 404) |
| `search.cdp` | answers chosen by the query string (`query = { q = "key*" }`) |
| `login.cdp` | sign in and out with named state (`set_state`, `when`) |
| `cart.cdp` | an in-memory collection: `POST` shows up in the next `GET`; arranged with `http` |
| `checkout.cdp` | an order that fails with 503 once, then succeeds (a `sequence`, in set `checkout`) |
| `errors.cdp` | the API down: set `errors/api-down` layered on top for one test |
| `live-prices.cdp` | a WebSocket mock; the test pushes prices with `mock ws send` |
| `e2e/fixtures/unmatched.cdp` | fails on purpose: a request no mock answers, and the diagnosis (not in `tests`; run it by path) |

## Layout

```
chrome-cdp.toml          [e2e]: the mocks, the static server, the real backend
e2e/scenarios/*.cdp      the tests
e2e/steps/sign-in.cdp    a step file (@include … email=…)
e2e/mocks/base.toml      the whole API (default_mocks)
e2e/mocks/checkout.toml  layered by @mocks checkout
e2e/mocks/errors/        layered by @mocks errors/api-down
e2e/mocks/data/          response bodies
e2e/vars/mock.toml       ${var:…} for every backend; local.toml overrides
src/                     the app
dist/                    the built app (committed)
```

## Against the real API

`--backend local` starts the repository's fixture server
(`cargo run -p fixtures -- --shop dist --port 4174`, built on the first
run), which serves `dist/` and a real, stateful `/api`. The mocks are off
and requests go to it. Tests marked `@backend mock` (a 503 on purpose, a
pushed price, mock state) are skipped with the reason. The real API has a
fourth product, so the scenarios read counts from `${var:products}`.

`chrome-cdp test --backend local --verify-mocks` also compares every real
answer with the answer the mocks would have given, and fails on drift in
status or JSON shape.

To work on the app against the real API, run the fixture server and
`npm run dev`; Vite sends `/api` to port 4174.
