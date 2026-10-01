# Writing tests and mocks: history, record, import, check, fmt, lint

Tests and mocks are plain files, but nobody has to write them from
scratch. The core workflow is: explore the app with the usual commands,
then save what worked.

## A session becomes a test: `history --as-scenario`

Every session keeps the last 1,000 commands it ran (from any shell, and
the steps of `batch` and `run`), each with whether it succeeded.
`chrome-cdp history` lists them; `--as-scenario NAME` prints them as a
test:

```
$ chrome-cdp open http://localhost:5173/shop
$ chrome-cdp snapshot -i
- button "Add" [e1]
- button "Add" [e2]
- textbox "Quantity" [e4] value="1"
- button "Buy" [e5]
$ chrome-cdp click e2
$ chrome-cdp fill e4 2
$ chrome-cdp click e5
$ chrome-cdp expect role=status text 'Order o-17: ordered, 2 items'
$ chrome-cdp history --as-scenario "orders a mouse" >> e2e/scenarios/shop.cdp
warning: left out 1 command: 2: snapshot -i (only reads)
```

```
@test "orders a mouse"
  goto /shop
  click 'role=listitem >> has-text=Mouse >> role=button[name="Add"]'
  fill 'role=textbox[name="Quantity"]' 2
  click 'role=button[name="Buy"]'
  expect role=status text 'Order o-17: ordered, 2 items'
```

- **Refs become stable locators.** A ref (`e12`) lives only as long as its
  session, so just before each command that names one runs, the session
  works out the most robust locator that finds that element and nothing
  else: role and name, then label, test id, placeholder, text; if none of
  those is unique, one of them inside a named container (a list item, a
  row, a dialog: `role=listitem >> has-text=Mouse >> role=button[name="Add"]`);
  `>> nth=N` only as a last resort (with a warning: a test id would be
  sturdier). A ref with no locator (an element in an iframe, for now)
  stays, with a comment above it and a warning.
- **Read-only commands are left out** (`snapshot`, `console`, `text`,
  `network`, `screenshot`, …), and so are failed commands and tools that
  are not steps (`status`, `mock record`, …). `expect` commands stay.
- `open URL` becomes `goto URL`; a URL under `base_url` becomes a path.
- `mock use SETS` becomes `@mocks SETS` (the defaults apply anyway).
- `--from N` starts at command N; `--clear` starts the history over after
  printing it. With `--context`, only that context's commands count.

Read the test before committing it: add the `expect` lines that say what
should have happened, if the exploration did not.

## Mocks from a real backend: `mock record`

Run the app against its real API, click through the flows, then:

```
$ chrome-cdp mock record --scope '/api/**' -o e2e/mocks/recorded.toml
e2e/mocks/recorded.toml: 6 mocks from 9 requests, 3 data files in e2e/mocks/data/recorded
next: chrome-cdp mock use recorded
```

It reads the session's network log (every tab of the context, or
`--tab`'s), keeps the requests `--scope` matches (globs as a set's `scope`
takes them; without it, the page's fetch and XHR requests, WebSockets and
EventSources) and writes:

- one `[[mock]]` per distinct request: method, path, query (cache busters
  such as `_=1767261600000` left out), and the operation of a GraphQL
  request;
- a request sent more than once with different answers as a `sequence`
  (`after_sequence` is `repeat-last`, so later calls get the last answer);
- JSON bodies in `data/<set>/…` (small ones inline), text and binary
  bodies in files typed by their extension;
- headers without the volatile ones (`date`, `etag`, `server`, CORS
  headers, …); `set-cookie` only when a later request sent that cookie;
- comments where a template would do: ids in paths (`path =
  "/api/products/:id"` answers every id), and ids and timestamps in bodies
  (`{{uuid}}`, `{{now}}` in a `.tmpl.json` data file);
- `[settings] scope`: the `--scope` given, or the first path segment every
  recorded request shares.
- a `[[websocket]]` per socket path: the server's messages before the
  page's first as `on_open` (each with the delay since the one before,
  rounded to 10 ms; none under 50 ms), and for each page message the
  server answered an `on_message` rule that sends the first answer. A JSON
  message is matched by its stable fields (as `--match-bodies` picks
  them), any other text exactly. Sockets to one path are merged; binary
  messages, answers after the first (a rule sends one message) and
  conflicting answers are left out and listed;
- an EventSource's messages as an `sse` answer, with their delays (a
  `text/event-stream` body in a HAR file gives the events without them).

Requests are told apart by method, path, query and GraphQL operation, so
two POSTs to one path with different bodies (logging in as two users)
become one `sequence` in call order. `--match-bodies` (for `mock import`
too) keys them by their bodies instead: the mock matches `json = { … }`
(`form` for a form, a multipart form's text fields; `graphql.variables`
for GraphQL) with the body's stable fields. A field is left out when its
value differs between the recorded requests of that method and path and
looks generated: a UUID, a number or date close to when the request was
sent, a string over 64 characters. Bodies are only matched where they
tell requests of one mock apart; requests with equal stable fields still
form a `sequence`, and the summary says how many were told apart:

```
$ chrome-cdp mock record --scope '/api/**' --match-bodies
e2e/mocks/recorded.toml: 3 mocks from 4 requests (2 told apart by body)
```

Chrome keeps response bodies only while the page that loaded them is open,
so record before navigating away. The file is checked as `mock use` would
read it. `-o` refuses to overwrite a file without `--force`.

`chrome-cdp test --backend <real> --record-mocks` records a whole suite
the same way, a set per passed test ([backends](backends.md#recording-mocks-from-a-real-run---record-mocks)).

## Mocks from a HAR file: `mock import`

```
chrome-cdp mock import requests.har --scope '/api/**'
```

The same conversion, from a HAR file: `chrome-cdp har stop`, the DevTools
Network panel's "Save all as HAR", or Playwright's `recordHar`. The mock
file is `<mocks dir>/<HAR's name>.toml` unless `-o` says otherwise.

## Mocks from an OpenAPI spec: `mock gen`

```
$ chrome-cdp mock gen --openapi api/openapi.yaml --collections
wrote e2e/mocks/generated.toml: 7 mocks, 2 collections, 2 data files (2 TODOs)
next: chrome-cdp mock use generated
warning: TODO getAvatar: the answer is image/png, which the spec gives no example of: add a body (body, body_file or file)
warning: TODO collection todos: $[].code: pattern ^[A-Z]{3}$ is not honored
```

For a project that has a spec (OpenAPI 3.x or Swagger 2.0, JSON or YAML;
`openapi` in `[e2e]` when `--openapi` is not given) and no mocks yet. It
writes `generated.toml` in the mocks directory (`-o` names another file;
it does not overwrite one without `--force`):

- **one `[[mock]]` per operation**, named by its `operationId` (else
  `GET /pets/{id}`), matching its method and path (`{id}` is `:id`, under
  the first server's path) and answering its lowest 2xx status (200 when
  the spec lists only `default`);
- **the body** is the media type's `example`, else the first of its
  `examples`, else built from the schema, where a schema's own `example`
  wins at every level and the rest is generated: `default`, `enum[0]` or
  `const`; objects with all their properties; arrays of two items (or
  `minItems`); strings by `format` (`date-time` from 2026-01-01 plus the
  seed in hours, a seeded `uuid`, `user1@example.com`,
  `https://example.com/…`, `192.0.2.1`, base64 for `byte`), else the
  property's name and a counter (`name-1`) within `minLength` and
  `maxLength`; numbers in `[minimum, maximum]` (1–100 by default), a
  multiple of `multipleOf`; `true`; the first of `oneOf`/`anyOf`, all of
  `allOf`. `$ref`s are followed until a cycle or a depth of 8: the
  property is left out there (`null` if it is required and nullable,
  else `{}` or `[]`);
- bodies over 100 characters go to `data/<set>/<operation>.json`; an
  answer with no JSON body (204, an image, text) has its status and an
  empty (or the example's) body;
- `[settings] scope` is the servers' paths (`/v1/**`), else the paths'
  first segments; `--scope GLOB` generates only the operations whose path
  it matches.

The values are deterministic: each operation draws from a generator
seeded by `--seed` (default 1) and its name, so the same spec and seed
give the same bytes, and a new operation leaves the others alone. What
the generator cannot make fit the spec (a `pattern`, a binary body, an
example the schema refuses) is a `# TODO:` comment above its mock, and is
listed. The file is then checked against the spec, as `mock check` would.

`--collections` makes each list and item path pair (`/pets` with a GET,
`/pets/{id}` with a GET) one `[[collection]]` of three generated items
instead of mocks, so create-then-list flows work: its `id` key is `id`
(or the item parameter's name, if only that is a property), a list body
that wraps the array in an object gets `list = { wrap = "…" }`, and a
nested pair (`/users/{userId}/todos`) gets `params` naming the field each
parent id is in (`{ userId = "user_id" }`).

## Keeping mocks right: `mock check`

```
$ chrome-cdp mock check
e2e/mocks/base.toml:4:1: error [openapi-body] mock "product by id" does not fit the spec's GET /products/{id} 200: $.price_cents: the spec requires it, the mock has none
e2e/mocks/base.toml:9:1: warning [unreachable] mock "gone" never answers: "product by id" (line 4) comes first and matches every request it does
```

It checks every mock file in the mocks directory (or the sets named):

| Rule | Severity | What |
|---|---|---|
| `load` | error | what `mock use` would refuse: the format, missing data files, invalid templates |
| `unreachable` | warning | a mock an earlier mock of its file always answers for |
| `collection-shadowed` | warning | a collection a mock answers every request of (in its file, or in a `default_mocks` set applied after it) |
| `duplicate-collection` | warning | two files with a collection of one name |
| `undeclared-state` | warning | a `when`, `set_state` or `unset_state` name, or a `{{state.x}}` in an answer (a mock's body, headers or `.tmpl.` data file, a `[[websocket]]`'s messages), no `[state]` declares |
| `ws-unreachable` | warning | a `[[websocket]]` an earlier one of its file takes every socket of |
| `ws-rule-unreachable` | warning | an `on_message` rule after one that answers every message (no `match`, or `match = "*"`) |
| `openapi-path` | error | the spec has no such path |
| `openapi-method` | error | the path has no such method |
| `openapi-status` | error | the operation does not list the status (exact, `4XX` or `default`) |
| `openapi-body` | error | the JSON body does not fit the response schema |
| `openapi-uncovered` | warning | an operation of the spec that no mock or collection stands for (the first 20, then a count) |

With `openapi = "api/openapi.yaml"` in `[e2e]` (or `--openapi FILE`), every
mock's path, method, status and JSON body (inline, or a data file whose
name is not a template) are checked against the operation it stands for,
which catches mocks the real API has moved away from. The spec may be
OpenAPI 3 or Swagger 2, JSON or YAML; paths are matched below the servers'
(or `basePath`'s) path, so `/api/products/:id` is `/products/{id}` of a
server at `/api`. Bodies are checked for their shape (types, required and
unknown properties, enums, array items, `allOf`/`anyOf`/`oneOf`, local
`$ref`s); template strings stand for any value. Collections' paths (with
any method) and seed items are checked too. A set scoped to full URLs
that none of the spec's servers is under (a third party's mocks, such as
`http://127.0.0.1:*/v1/**`) is not checked against it.

`openapi-uncovered` names each operation of the spec that no mock or
collection of the checked sets stands for: a mock whose path and method
land on that operation (`/api/products/:id`, `/api/products/7`). A
catch-all such as `path = "/api/**"` answers every operation and stands
for none, so an `errors/api-down` set does not hide the gaps.

Errors exit 1, warnings alone 0; `--json` has every problem with its
`file`, `line`, `col`, `severity`, `rule`, `message` and `hint`.
`chrome-cdp test` runs the check on the sets it uses before it starts a
browser, stops on errors and shows the warnings; the operations with no
mock are one line (`12 spec operations have no mock (chrome-cdp mock
check … --openapi to list)`). `mock_check` in `[e2e]` says what stops the
run: `"errors"` (the default), `"warnings"` (for CI that wants the mocks
to follow the spec), or `"off"` (no check).

## Tidy files: `fmt`

```
$ chrome-cdp fmt
formatted e2e/scenarios/checkout.cdp
12 files, 1 formatted
```

`fmt` lays out every `.cdp` file below the project root and every mock
file of the mocks directory (or the files and folders named) the same way:

- `.cdp` files: bodies indented 2 spaces; one blank line before each
  `@test` and hook (and the comments right above it); quoted words in
  single quotes unless they need `${…}` or contain a `'` (test names and
  other directive arguments take double quotes); long flags instead of
  short ones (`-o` becomes `--output`); settings in a stable order
  (`@skip @only @mocks @tags @backend @safe @timeout @retries
  @viewport/@device @allow-console-errors`); trailing comments lined up.
- Mock files: the keys of each `[[mock]]` in the order `name`, `match`,
  `respond`, `sequence`, … (likewise for `[[collection]]` and
  `[[websocket]]`), `key = value` spacing, inline tables kept inline.

Comments stay, and so does the meaning: the file is read again after
formatting and must be the same scenario (each changed line must parse to
the same command), or `fmt` leaves it alone with an error. A file with
syntax errors is not formatted; `lint` lists them. `fmt --check` writes
nothing, prints a diff and exits 1 when a file would change, for CI.

## Catch mistakes before a run: `lint`

```
$ chrome-cdp lint
error: e2e/scenarios/cart.cdp:3:3: error [ref] `e12` is a ref: refs name elements of one session's snapshot, so they break in a scenario
  fix: use a stable locator such as role=button[name="Save"], label=Email or text=Sign in; `chrome-cdp history --as-scenario` writes them for the refs of a session
  also: e2e/scenarios/cart.cdp:9:3: error [unknown-command] unrecognized subcommand 'clik'
  also: e2e/scenarios/cart.cdp:4:3: warning [sleep] `wait 500` sleeps 500ms whatever the page does: slow when it is quick, flaky when it is slow
  checked: 2 files: 2 errors, 1 warning
```

`lint` checks the same files without running anything; mock files get
the `mock check` rules above. Scenario rules:

| Rule | Severity | What |
|---|---|---|
| `syntax` | error | the grammar, and directives used wrongly (`@timeout soon`) |
| `unknown-command`, `unknown-flag`, `bad-args` | error | a line clap refuses, with the corrected line as the hint |
| `unknown-directive` | error | `@tag` (did you mean `@tags`?) |
| `unknown-device`, `unknown-backend` | error | `@device` or `@backend` naming none |
| `locator` | error | a locator that does not parse |
| `include-missing`, `include-cycle` | error | an `@include` file that is not there, or includes itself |
| `include-param` | error | a step file's `${name}` its `@include` line does not pass |
| `undefined-var` | error | a `${var:x}` missing from the vars of a backend that runs the test; a `${saved.x}` no `http --save` saves; a missing `data:` file |
| `ref` | error | a ref (`e12`): refs belong to one session's snapshot |
| `duplicate-test` | error | two tests of a file with one name |
| `mock-only` | warning | `mock state`, `mock seed`, `mock ws send`, `mock sse send` or `expect mock-state` in a test a real backend runs (mark it `@backend mock`) |
| `mock-literal` | warning | a value of the mock data files written into a test a real backend runs; the real data may differ (use `${var:…}`) |
| `sleep` | warning | `wait 500`: wait for a condition instead |
| `no-expect` | warning | a test with no `expect` step |

A file with directive errors (`@timeout soon`, a missing `@include`) still
has its steps checked, so one pass lists everything; only a syntax error
stops the rest of its file, since the lines around it are guesses.

Each step is checked for every backend that runs its test, so a variable
missing from one backend's vars file is found before that backend's run.
`${env:…}` and `${secret:…}` are not read, so `lint` needs no secrets.
Errors exit 1, warnings alone 0. `--format github` prints GitHub workflow
annotations, so the problems show up on the pull request's lines;
`--format json` (or `--json`) has each problem's `file`, `line`, `col`,
`severity`, `rule`, `message` and `hint`. `chrome-cdp test` runs the
scenario rules first and does not start on errors; it counts the
warnings in one line. The workflow `init e2e` writes runs `fmt --check`
and `lint --format github` before the tests.

## The format as data: `schema scenario`

`chrome-cdp schema scenario` prints, as JSON, what a `.cdp` file may hold:
the directives (with their usage and kind), every command a line may run
with its arguments and flags (short and long forms, value names, allowed
values), the global flags, the variable forms and the mock-only commands.
It is generated from the binary's own command definitions, so it matches
the chrome-cdp that prints it. Editors and agents can build completion and
checks on it; `chrome-cdp schema mocks` is the mock files' JSON Schema.

