# Writing tests and mocks: history, record, import, check

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
takes them; without it, the page's fetch and XHR requests) and writes:

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
| `undeclared-state` | warning | a `when`, `set_state` or `unset_state` name no `[state]` declares |
| `openapi-path` | error | the spec has no such path |
| `openapi-method` | error | the path has no such method |
| `openapi-status` | error | the operation does not list the status (exact, `4XX` or `default`) |
| `openapi-body` | error | the JSON body does not fit the response schema |

With `openapi = "api/openapi.yaml"` in `[e2e]` (or `--openapi FILE`), every
mock's path, method, status and JSON body (inline, or a data file whose
name is not a template) are checked against the operation it stands for,
which catches mocks the real API has moved away from. The spec may be
OpenAPI 3 or Swagger 2, JSON or YAML; paths are matched below the servers'
(or `basePath`'s) path, so `/api/products/:id` is `/products/{id}` of a
server at `/api`. Bodies are checked for their shape (types, required and
unknown properties, enums, array items, `allOf`/`anyOf`/`oneOf`, local
`$ref`s); template strings stand for any value. Collections' paths and
seed items are checked too.

Errors exit 1, warnings alone 0; `--json` has every problem with its
`file`, `line`, `col`, `severity`, `rule`, `message` and `hint`.
`chrome-cdp test` runs the check on the sets it uses before it starts a
browser, stops on errors and shows the warnings.
