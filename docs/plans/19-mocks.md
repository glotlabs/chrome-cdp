# Plan 19 — Mocks

Status: DONE (19.1-19.4)
Depends on: 11 (the mock format, `mock record`/`import`/`check`/`serve`, WebSocket and SSE mocks), 13 §13.2 (mocks in every target), 14 §14.7 (WebSocket frames in the network log)
Roadmap: track 8 of [plan 12](12-after-v1.md)

## Goal

Plan 11 gave chrome-cdp its mocks. This plan fills in their edges, so a real app's traffic
can be mocked without falling back to hand-written workarounds:

- **Matching and data:** multipart form bodies, nested REST resources,
  and recordings that tell requests apart by their bodies.
- **Streaming:** WebSocket and SSE traffic can be recorded, pushed from a
  scenario, linted, and served by `mock serve` to clients that are not a
  browser page.
- **Upstream:** `mock serve` can forward to a real server, so hybrid
  setups (some endpoints mocked, the rest real) work for server-side
  fetches too; the e2e-shop example shows hybrid mode.
- **Mocks from a spec:** `mock gen --openapi` writes a starting mock set
  from an OpenAPI spec, and `test` warns about spec operations no mock
  covers.

Read plan 11 (§11.2–§11.3, §11.8, §11.9, §11.11) and
[docs/e2e/mocks.md](../e2e/mocks.md) first. Code: `crates/mock/`
(format, matching, templates, collections, record, check, openapi),
`crates/e2e/src/mock_server.rs` (`mock serve`),
`crates/engine/src/route/` (mocking in the browser, the socket table,
`shim.js`), `crates/chrome-cdp/src/mock.rs` (the `mock` commands).

## Rules for every milestone

- **The mock format only grows.** New keys are optional and default to
  today's behavior. A file that loads today loads the same way after
  this plan. The format is the `schemars` types in the mock crate;
  `docs/e2e/mocks.schema.json` and `docs/e2e/mocks.md` are generated
  from them (`UPDATE_GOLDEN=1 cargo test -p chrome-cdp-mock reference`)
  and may only gain entries.
- **The command line only grows** (docs/output.md): new flags and
  subcommands, no renamed or removed ones, `--json` schemas only add.
- Every new command follows plan 08 §2–§4: compact text, `--json`,
  bounded output, errors with a hint, and an example in its help.
- Every change is covered by unit tests in the mock crate and, where the
  browser or a server is involved, an integration test against the
  fixture server.
- Update docs/e2e/ (mocks.md is generated; beyond-fetch.md, authoring.md,
  backends.md and recipes.md are hand-written), docs/commands/ (`cargo
  xtask docs`) and the golden tests.

## Milestone 19.1 — Matching and data

Backlog notes: `mock-multipart-forms`, `mock-collection-nested-paths`,
`mock-record-request-bodies`.

### Multipart form bodies

`match.form` parses `application/x-www-form-urlencoded` only
(`crates/mock/src/matcher.rs`, `Req::form`), so a `FormData` POST never
matches and the reason says "the body is not a form".

- When the `Content-Type` is `multipart/form-data`, parse the body with
  the `boundary` parameter. Each part with a `Content-Disposition:
  form-data; name="…"`:
  - a text part (no `filename`) gives `name = <its text>`;
  - a file part gives `name = <its filename>`, so `form = { avatar =
    "*.png" }` matches an upload by its file name.
  Repeated names keep every value; `match.form` holds if any value
  matches (as it does today for repeated urlencoded fields).
- Parse with a small hand-written parser in the mock crate (no new
  dependency unless one is already in the tree). A malformed body (no
  boundary, no closing delimiter) is "not a form", with that reason in
  `mock explain`.
- Request bodies reach the matcher as text (`from_utf8_lossy`); file
  contents may be binary. Only the part headers and text parts are
  needed, so lossy text is fine. Do not keep file contents: a part's
  value is its file name.
- Templates: `{{form.x}}` already exists for urlencoded bodies; it gets
  multipart fields the same way. Add `{{form_files.x}}` with
  `{ filename, content_type, size }` of a file part (size in bytes of the
  part as received).
- `mock record` and `mock import`: a multipart POST's recorded match
  uses `form` with its text fields (not `body`), leaving file fields out.

### Nested collections by path parameter

A `[[collection]]` path may have parameters (`/api/users/:user/todos`)
but `Collection::op` (`crates/mock/src/collection.rs`) ignores them:
every user shares one list.

- A collection's path parameters **scope its items**: each parameter
  names a field of the same name. `GET /api/users/7/todos` lists the
  items whose `user` field equals `"7"` (compare as text, so a number
  `7` in the seed matches); `GET /api/users/7/todos/3` finds item 3 only
  if its `user` is 7, else 404; `POST` sets `user = "7"` on the new item
  (a number if the seed's values of that field are numbers); `PUT` and
  `PATCH` keep it (a body that changes it is refused with 400 and a
  reason); `DELETE` removes only a matching item.
- The field is named after the parameter unless the collection maps it:
  a new optional key `params = { user = "userId" }` (parameter → field).
- Ids stay unique across the whole collection, not per parent, so
  `new_id = "increment"` counts over all items.
- `list.filter_query`, paging and `wrap` apply after the scoping.
- `mock state <collection>` still shows every item; `mock seed` is
  unchanged.
- `mock check`: a `params` entry naming a parameter the path does not
  have is a `load` error.

### Recording bodies: `--match-bodies`

Recorded mocks match method, path, query and GraphQL operation, so two
POSTs to one path with different bodies become one `sequence` in call
order.

- `mock record --match-bodies` and `mock import --match-bodies`: for
  requests with a JSON body, the recorded mock's match gains `json =
  { … }` with the body's **stable** fields: drop fields whose value
  differs between the recorded requests of that method and path *and*
  looks generated (a UUID, a number or date close to the recording
  time, a string over 64 characters). Keep the rest. Requests whose
  stable subsets are equal still form a `sequence`.
- For form bodies (urlencoded or multipart), the same with `form`.
- Without the flag, nothing changes.
- The summary line says how many requests were told apart by body.

## Milestone 19.2 — Streaming

Backlog notes: `mock-record-websockets`, `mock-sse-send`,
`mock-check-websockets`, `mock-serve-websockets`.

### Recording WebSocket and SSE traffic

- `mock record` and `mock import` also write `[[websocket]]` entries for
  sockets in scope (a scope glob matches a socket's URL as it matches
  requests; the default scope — the page's fetch and XHR — also takes
  the page's WebSockets). Sources: the network log's frames (plan 14,
  §14.7) for `record`, and Chrome's `_webSocketMessages` in HAR files
  for `import`.
- Conversion of one socket: `url` is its path glob (full URL when it is
  on another origin than the page, as for requests); the server's frames
  before the page's first message become `on_open` sends (each with the
  `delay` since the one before, rounded to 10 ms, dropped when under
  50 ms); each page message and the server frames after it, up to the
  page's next message, become an `on_message` rule matching that message
  (a JSON subset of its stable fields, as `--match-bodies` picks them;
  text messages exactly) that sends the first frame. Frames beyond the
  first answer to one message become extra rules only if they can be
  keyed; otherwise they are listed in `skipped` with the reason. Binary
  frames are skipped with a reason. Several sockets to one URL are
  merged: identical rules once, conflicting ones from the first socket
  only, the rest in `skipped`.
- A `text/event-stream` response becomes a `respond.sse` with its events
  (`mock::parse_sse` exists) and their delays, instead of a body.

### `mock sse send`

```
chrome-cdp mock sse send <URL-GLOB> <DATA> [--event TYPE] [--id ID] [--retry MS]
```

- Pushes one event to every open mocked `EventSource` of the context
  whose URL the glob matches (the globs of `mock ws send`), after the
  mock's own `sse` events. `DATA` is sent as it is (`-` reads stdin;
  newlines become several `data:` lines).
- The engine keeps the open mocked EventSources in the route's socket
  table next to the WebSockets (`crates/engine/src/route/sockets.rs`),
  with the same cap and removal on close; `shim.js` reports open and
  close through the same binding.
- Waits up to `--timeout` for a matching stream to open, like `mock ws
  send`. Output: `sent to 2 streams (/api/prices)`.
- Scenario syntax: it is an ordinary command, so it works in `.cdp`
  files unchanged.

### Lint rules for `[[websocket]]`

In `crates/mock/src/check.rs`, next to the `[[mock]]` rules:

- `ws-unreachable` (warning): a `[[websocket]]` whose `url` an earlier
  one of the same file always covers (the glob-cover logic of
  `unreachable`).
- `ws-rule-unreachable` (warning): an `on_message` rule after a rule
  with no `match` (or a match that takes every message), in the same
  mock.
- `undeclared-state` also covers state names used in websocket
  templates, when websocket mocks can read state (today they cannot; if
  so, skip this part and say so in the decisions log).
- `ws-close-code` (error): a `close` code outside 1000 and 3000–4999
  (if `load` does not already refuse it).

### `mock serve` answers WebSocket upgrades

- A request with `Upgrade: websocket` to `mock serve` whose URL a
  `[[websocket]]` mock matches is upgraded (hyper's upgrade plus
  `tokio-tungstenite`, already a workspace dependency) and served by
  `Mocks::ws_open` and `ws_message`, with delays and `close` as in the
  browser. No match: 404 as today. Per-test state partitions (the
  `x-chrome-cdp-test` header) apply to sockets too.
- **Pushing to served sockets and streams.** `mock serve` reserves the
  path prefix `/__chrome-cdp/` for control requests, served only from
  loopback peers: `POST /__chrome-cdp/ws/send` and
  `POST /__chrome-cdp/sse/send` with a JSON body
  `{ "url": glob, "data": …, "event"?, "id"?, "test"? }`.
  `mock ws send` and `mock sse send` get `--serve <URL>` to post there
  instead of to the browser. In a `test` run with a `mock_server`, a
  `mock ws send` or `mock sse send` step goes to both the browser and
  the runner's mock server (for the test's partition) and reports
  counts for each.
- `mock serve`'s printed log shows `WS /ws/prices open`, each message
  in and out (cut to one line), and the close.

## Milestone 19.3 — Upstream

Backlog notes: `mock-serve-upstream`, `e2e-shop-hybrid-backend`.

### `mock serve --upstream`

`passthrough = true` mocks and `unmatched = "passthrough"` answer 502
from `mock serve` today.

- `mock serve --upstream <URL>`, and `mock_server = { port = 4010,
  upstream = "http://localhost:8080" }` in `[e2e.web_server]` (and in a
  backend's `web_server`), forward those requests to the upstream:
  the same method, path and query, joined onto the upstream's URL
  (its path is a prefix); the request's headers minus hop-by-hop ones
  (`Connection`, `Keep-Alive`, `Transfer-Encoding`, `Upgrade`, …) and
  `Host`, which becomes the upstream's; the body as received.
- `patch` applies to the upstream's JSON answer, as in the browser.
- A request no set owns still answers 404; only owned requests that
  pass through are forwarded. `--upstream` does not change that.
- Timeouts and errors: 30 s per request (configurable with
  `--upstream-timeout`); a connection error or timeout answers 502 with
  a JSON body naming the upstream and the error, and the log line says
  so.
- The journal (`mock log` for a test, `report.md`) marks these
  `passthrough → upstream 200`.
- WebSocket passthrough is out of scope: a socket no mock matches is
  404 as before.
- Use the HTTP client already in the tree (`reqwest` with rustls).

### e2e-shop in hybrid mode

The shop has no third-party service to mock. Give it one:

- Checkout calls a fake **payment provider** on another origin from the
  browser (`POST http://127.0.0.1:4175/v1/charges`, like a Stripe.js
  call), before posting the order to the shop's API. The URL comes from
  the app's config so mock and real runs can point it anywhere.
- The fixture server (`-p fixtures`, used by `backends.local`) serves
  the payment provider on its own port too, so `--backend local` stays
  fully real.
- `e2e/mocks/third-party/payments.toml`: the provider's mocks (success,
  and a declined card named for a scenario to use).
- `[e2e.backends.hybrid]`: the real shop API plus
  `mocks = ["third-party/payments"]`, as in docs/e2e/backends.md.
- A checkout scenario that pays with the declined card, `@backend mock
  hybrid` (it needs the mock).
- Rebuild `dist/` (it is committed) and update the README and
  `crates/chrome-cdp/tests/e2e_shop.rs` with a hybrid case.

## Milestone 19.4 — Mocks from a spec

### `mock gen --openapi`

```
chrome-cdp mock gen [--openapi FILE] [-o FILE] [--force] [--dir DIR]
                    [--seed N] [--scope GLOB] [--collections]
```

Writes one mock set with a `[[mock]]` for each operation of the spec
(OpenAPI 3.x or Swagger 2.0, JSON or YAML; the loader in
`crates/mock/src/openapi.rs`), for a project that has a spec but no
mocks yet.

- `--openapi` defaults to `[e2e] openapi`. `-o` defaults to
  `generated.toml` in the mocks directory; it refuses to overwrite
  without `--force`, like `mock record`. `--scope` limits operations by
  path glob. `[settings] scope` is written from the spec's server paths
  (`/v1/**`), else the union of its paths' first segments.
- **One mock per operation**, for its success answer: the lowest 2xx
  status the spec lists (`default` when there is none; 200 then). The
  match is the method and the path with `{param}` turned into `:param`.
  The mock's `name` is the `operationId`, else `METHOD path`.
- **Example data, in this order**, for the chosen response's JSON body:
  1. the media type's `example`;
  2. the first of the media type's `examples` (its `value`);
  3. the schema's own `example`, recursively: a property or item schema
     with an `example` uses it;
  4. generated from the schema, below.

  Large bodies (over 2 KB as JSON) go to a data file,
  `data/<operation>.json`, as `mock record` does.
- **Generated values** are deterministic: the same spec and `--seed`
  (default 1) give byte-identical output. Generation follows `$ref`s
  (local only, as the loader does) and stops at a depth of 8 or on a
  cycle, leaving the property out (or `null` if it is required and
  nullable, else `{}`/`[]`).
  - `default`, then `enum[0]`, then `const`.
  - `object`: required properties always, optional ones too (a mock
    shows the full shape); `additionalProperties` adds none.
  - `array`: `max(minItems, 2)` items, capped by `maxItems`.
  - `string` by `format`: `date-time` → a fixed instant derived from the
    seed (`2026-01-01T00:00:00Z` plus the seed in hours), `date`, `uuid`
    (seeded), `email` (`user1@example.com`), `uri`/`url`
    (`https://example.com/…`), `ipv4`, `byte` (base64 of a short text);
    otherwise the property name plus a counter (`name-1`), cut or padded
    to `minLength`/`maxLength`. `pattern` is not honored; such values are
    marked with a `# TODO: pattern …` comment.
  - `integer`/`number`: a seeded value in `[minimum, maximum]` (default
    1–100), a multiple of `multipleOf`.
  - `boolean`: `true`.
  - `oneOf`/`anyOf`: the first branch; `allOf`: the merge of all.
- **Collections** (`--collections`): a path pair `/things` (GET list,
  POST) and `/things/{id}` (GET, PUT/PATCH, DELETE) becomes one
  `[[collection]]` named `things` with three generated items (the item
  schema: the list's array items, else the item GET's body) instead of
  mocks, using `id` or the path parameter's name as its `id` key. Nested
  paths get `params` (§19.1).
- Operations with no JSON success body (204, a binary, a text body) get
  a mock with that status and an empty or placeholder body.
- The file starts with the `#:schema` line and a comment naming the
  spec and seed. Output: `wrote e2e/mocks/generated.toml: 14 mocks, 2
  collections, 3 data files (4 TODOs)`; `--json` lists them.
- **The set must pass `mock check` against the same spec.** A value the
  generator cannot make fit is a TODO comment, not a silent misfit.

### The spec in `test`

`chrome-cdp test` already runs `mock check` on the sets it uses, with
`[e2e] openapi` (`crate::mock::check_for_run`): errors stop the run and
warnings are printed. Add:

- A new rule, `openapi-uncovered` (a warning in `mock check`, only with
  a spec): an operation of the spec that no mock and no collection of
  the checked sets matches. `mock check` lists them (at most 20, then a
  count); in `test` they are summed up in one warning line
  (`12 spec operations have no mock (mock check --openapi to list)`)
  so a run's output stays short.
- `[e2e] mock_check = "errors" | "warnings" | "off"` (default
  `"errors"`, today's behavior): `"warnings"` fails the run on warnings
  too (for CI that wants mocks to track the spec), `"off"` skips the
  check.

### Test corpus

- Vendor the OpenAPI Petstore spec (3.0, Apache-2.0; keep its license
  header) into `testdata/openapi/`. Golden test: `mock gen` over it with
  seed 1, and with `--collections`.
- Write `examples/e2e-shop/api/openapi.yaml` for the shop's real API
  (products, cart, orders, the payment provider's charge call is not
  in it), set `openapi` in its config, and fix any mock the check then
  flags.

## Acceptance criteria

- [x] A `FormData` POST with text and file fields matches `form = {
      email = "*@example.com", avatar = "*.png" }`, and `{{form.email}}`
      and `{{form_files.avatar.filename}}` render.
- [x] `/api/users/:user/todos` keeps each user's todos apart for list,
      get, post, put, patch and delete, with and without `params`.
- [x] `mock record --match-bodies` on two logins as different users
      writes two mocks keyed by the email, not a sequence.
- [x] Recording a page that uses a WebSocket and an EventSource, then
      replaying the recorded set, gives the page the same messages and
      events (fixture pages on the fixture server).
- [x] `mock sse send` pushes an event a page's `EventSource` receives,
      in a session and in a `.cdp` scenario.
- [x] `mock check` reports each new websocket rule on a fixture file.
- [x] A WebSocket client that is not a browser (a test using
      `tokio-tungstenite`) talks to `mock serve`'s `[[websocket]]`
      mock, and `mock ws send --serve` reaches it.
- [x] `mock serve --upstream` forwards passthrough mocks and unmatched
      owned requests, applies `patch`, and answers 502 with a reason
      when the upstream is down.
- [x] examples/e2e-shop passes with `--backend mock`, `--backend local`
      and `--backend hybrid`.
- [x] `mock gen` over the Petstore spec is deterministic and the set it
      writes passes `mock check --openapi` with no errors.
- [x] `test` in e2e-shop checks the mocks against its spec and prints
      the `openapi-uncovered` summary when a mock is removed.
- [x] `docs/e2e/mocks.schema.json` only gained entries (the release
      check's schema comparison passes).

## Tests

- Unit (mock crate): multipart parsing (boundaries with quotes, CRLF and
  LF, repeated names, files, a malformed body); nested collection ops
  for each method, `params` mapping, number vs text parent ids; stable
  field picking for `--match-bodies`; websocket and SSE recording from
  hand-made exchanges and a HAR with `_webSocketMessages`; each new
  check rule, positive and negative; example selection order and each
  generated type and format; the Petstore golden.
- Integration (real Chrome, fixture server): a multipart form fixture;
  a nested todos fixture; record → replay for a WebSocket page and an
  SSE page; `mock sse send` in a session and in a scenario.
- Integration (no browser): `mock serve` upgrades and pushes; upstream
  forwarding against an in-test server, including patch, headers,
  timeout and a dead upstream.
- `crates/chrome-cdp/tests/e2e_shop.rs`: the hybrid case.
- Golden: help and `--json` schemas of the new commands and flags;
  docs/e2e/mocks.md and mocks.schema.json regenerated.

## Decisions log

_(append here)_
- 19.1: a multipart POST's recorded `form` match (text fields, files left
  out) is written under `--match-bodies` only. Recordings match no body
  today, and "without the flag, nothing changes" wins over the
  multipart bullet.
- 19.1: `--match-bodies` adds a body match only where it splits a mock
  (requests of one method, path, query and operation with different
  stable fields). A lone request keeps no `json`: its generated fields
  cannot be told from chosen ones without a second request, and a
  stricter match would buy nothing. Mocks of one path are ordered by how
  many body fields they match, most first, so a subset never shadows a
  superset. Names get the telling fields (`POST /api/login
  (email=bob@example.com)`).
- 19.1: GraphQL requests key on `graphql.variables` (not `json` over the
  whole body), and "close to the recording time" is within two days of
  the request's own time (the HAR's `startedDateTime`, else now).
  Numbers count as seconds or milliseconds.
- 19.1: a POST to a nested collection whose body names another parent
  (`user = 8` under `/users/7/todos`) is refused with 400, as PUT and
  PATCH are, rather than moved silently.
- 19.1: `match.form` holds if any value of a repeated field matches. The
  plan says this is today's behavior, but the code checked the first
  value only.
- 19.2: no `ws-close-code` rule: `load` already refuses a close code
  outside 1000 and 3000–4999, so `mock check` reports it as `load`.
  Websocket templates do read state (`{{state.x}}`), so
  `undeclared-state` scans their messages and close reasons; it scans
  `[[mock]]` answers (bodies, headers, `.tmpl.` data files) the same way.
- 19.2: recorded `[[websocket]]`s match the socket's path, never its full
  URL: recorded `[[mock]]`s match paths only today too (a repro bundle's
  other origins are a backlog note), and a path glob takes the socket on
  any host.
- 19.2: a page message's answers after the first are always left out
  (and listed): a rule sends one message, and a second rule for the same
  message would never be reached, so no extra rule can be keyed.
- 19.2: the default record scope takes the page's EventSources too, not
  only its WebSockets: an `sse` answer is the point of recording them.
- 19.2: the network log keeps each frame's payload (up to 64 KB) and an
  EventSource's messages (`Network.eventSourceMessageReceived`), and the
  HAR export writes them: `_webSocketMessages` gets the whole payload, and
  `_eventSourceMessages` (our extension) the messages. Without them a
  still-open stream has no body to record.
- 19.2: an upgrade no `[[websocket]]` takes is answered by the HTTP mocks
  as before (404 when no set owns it, the set's `unmatched` answer in a
  scope). A WebSocket open is journalled (resource type `websocket`) but
  not counted in `mock serve`'s requests; the socket log has it.
- 19.2: control requests also take `base64` (a binary message) and
  `timeout_ms` (the server waits for a socket), and without `test` reach
  every partition. In a `test` run, a push step polls the browser and the
  mock server with no wait until either has a match, so a step whose
  socket is only in the browser is as fast as before.
- 19.3: a real backend starts its web server's `mock_server` only when it
  has an `upstream` (without one it would answer 404 for the real API),
  and it serves the backend's `mocks`, as the browser does in hybrid
  mode; mock mode is unchanged. `upstream_timeout` is the config's
  `--upstream-timeout`.
- 19.3: a forwarded answer is streamed as it comes (an upstream event
  stream stays one) unless a `patch` reads it; then the request goes
  without `Accept-Encoding`, so the body comes back plain. Redirects are
  passed on, not followed, as a browser's fetch would see them.
- 19.3: the shop reads the provider's URL from `public/config.js` (a
  script the static server and the fixture server serve as is), not a
  build-time variable: tests move it to a free port without a rebuild.
  The mock set's scope is the provider's origin on any port
  (`http://127.0.0.1:*/v1/**`), and the set is in `default_mocks` too.
- 19.3: the fixture provider accepts every card, so the declined card is
  the mock's alone and its scenario is `@backend mock hybrid`.
- 19.4: data files go to `data/<set>/<operation>.json` (`data/generated/`
  by default), as `mock record`'s go to `data/<set>/`: two generated sets
  cannot overwrite each other's.
- 19.4: a body is written inline only up to 100 characters of JSON, not
  2 KB: TOML's inline tables are one line, and a 2 KB line is no file to
  start from. Bodies TOML cannot say (a `null`) go to data files too, and
  a collection's items to a seed file when one is over 160 characters.
- 19.4: each operation's values come from a generator seeded by `--seed`
  and the operation's name, with counters (`name-1`) starting over per
  mock, so adding an operation leaves the others' values alone. All
  `date-time` values are the one seeded instant. A `number` whose range
  holds no whole number gets a fraction to two places, not a TODO;
  `writeOnly` properties are left out unless required.
- 19.4: `--collections` needs a list GET (an array, or an object with an
  array property, which becomes `list.wrap`) and an item GET; the
  Petstore has no list GET, so its `--collections` golden equals the
  plain one, and `testdata/openapi/todos.yaml` covers collections (nested,
  wrapped) and every type, format and example source.
- 19.4: `openapi-uncovered` counts a mock for an operation when its path
  and method land on it in the spec (`/pets/:id`, `/pets/7`), not when a
  catch-all (`/api/**`, a regex, no path) merely matches it: the shop's
  `errors/api-down` would otherwise cover every operation of every run.
  Its problems name the spec file at the operation's line.
- 19.4: a set scoped only to full URLs that no server of the spec is
  under (a third party's, like the shop's payment provider) is not
  checked against the spec, and a collection's paths need to be in the
  spec with any method rather than GET (the real shop has no GET of one
  cart item, nor a list of orders). Without both, the shop's mocks could
  not pass its own spec.
- 19.4: the spec check now lets `null` through a `nullable` schema whose
  type comes from `allOf: [$ref]`, the 3.0 way of a nullable reference
  (it checked the `allOf` first and refused it).
- 19.4: the Petstore is swagger-api/swagger-petstore's OpenAPI 3.0 file,
  with a comment naming its source and its Apache-2.0 license (the spec's
  own `info.license` says so too); it has no separate license header to
  keep.
- 19.4: "the first of `examples`" is the first by name, and generated
  objects list their properties by name: the spec loader reads into
  `serde_json` maps, which sort their keys (the workspace does not turn
  on `preserve_order`). The output is still deterministic; document order
  is a backlog note (`mock-gen-spec-key-order`).
