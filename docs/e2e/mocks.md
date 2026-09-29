# Mock file reference

<!-- Generated from `chrome-cdp schema mocks` by the mock crate's tests;
run `UPDATE_GOLDEN=1 cargo test -p chrome-cdp-mock reference` after changing the format. -->

A chrome-cdp mock file: one mock set. Each `[[mock]]` answers the requests it matches; within a file the first match wins, then the `[[collection]]`s are tried, and sets applied later are tried first. Data files (`json_file`, `body_file`, `file`, `seed`) are relative to the mocks directory.

The format as JSON Schema: [`mocks.schema.json`](mocks.schema.json) (or
`chrome-cdp schema mocks`). Start a mock file with

```toml
#:schema https://raw.githubusercontent.com/glotlabs/chrome-cdp/main/docs/e2e/mocks.schema.json
```

and Taplo (Even Better TOML) checks and completes it in the editor.
Templates (`{{params.id}}`, `{{json.qty}}`, `{{uuid}}`, …) and how
matching works are described in plan 11 and
[scenarios.md](scenarios.md); `chrome-cdp mock explain` shows which
mock answers a request.

## The file

| Key | Type | What |
|---|---|---|
| `[settings]` | table ([`[settings]`](#settings)) | Settings for the whole set. |
| `[state]` | table of any values | Named values and where they start, e.g. `logged_in = false`. Mocks require them (`when`) and change them (`set_state`); every set in use shares them, and `mock reset` starts them over. |
| `[[mock]]` | list of tables ([`[[mock]]`](#mock)) | The mocks, tried in the order written. |
| `[[collection]]` | list of tables ([`[[collection]]`](#collection)) | Fake REST resources kept in memory, tried after the set's mocks. |
| `[[websocket]]` | list of tables ([`[[websocket]]`](#websocket)) | Fake WebSocket servers: the page's `WebSocket`s to these URLs talk to the mocks instead of the network. |

<a id="settings"></a>

## `[settings]`

| Key | Type | What |
|---|---|---|
| `cors` | `"auto"` \| `"off"` | `auto`: answers carry CORS headers and preflights for mocked requests are allowed, so cross-origin API hosts work. `off`: CORS is left to the mocks' own headers. |
| `latency` | text | The delay every answer of the set gets: `0ms`, `100ms`, or a range such as `50ms..200ms` (seeded, so reruns match). A mock's `delay` wins. |
| `scope` | text; or list of text | The requests the set owns: path globs (`/api/**`) or full-URL globs (`https://api.example.com/**`). An owned request no mock answers is handled by `unmatched`; other requests no mock answers go to the network. |
| `unmatched` | `"fail"` \| `"passthrough"` \| `"abort"` | What an owned request that no mock answers gets: `fail` (a 501 answer, and the request counts as unmatched), `passthrough` (it goes to the real server) or `abort` (a network error). |

<a id="mock"></a>

## `[[mock]]`

A request it matches and the answer.

| Key | Type | What |
|---|---|---|
| `after_sequence` | `"repeat-last"` \| `"cycle"` \| `"unmatched"` | With `sequence`, what comes after its last answer: `repeat-last` (the default), `cycle` (from the first again) or `unmatched` (the mock stops matching, so the next one answers). |
| `match` (required) | table ([`match`](#match)) | The requests it answers. Every condition given must hold. |
| `name` | text | What `mock list`, `mock log` and `mock explain` call it; `METHOD path` when not set. |
| `respond` | table ([`respond`](#respond)) | The answer. |
| `sequence` | list of tables ([`respond`](#respond)) | Answers given in turn instead of `respond`: the first to the first request, the second to the next, … (`mock reset` starts over). |
| `set_state` | table of any values | Named values to change after answering: `{ logged_in = true, user = "{{json.email}}" }`. Strings may be templates. |
| `unset_state` | list of text | Named values to remove after answering (a `when` of `"*"` then no longer holds), e.g. `["user"]`. |
| `when` | table of any values | Named values (`[state]`) that must hold for the mock to match: `{ logged_in = true }`. `"*"` means any value but none; other values match as `match.json` does. |

<a id="collection"></a>

## `[[collection]]`

A fake REST resource. `GET path` lists the items, `GET path/:id` gets one, `POST path` adds one, `PUT` and `PATCH path/:id` replace and change one, and `DELETE path/:id` removes it.

| Key | Type | What |
|---|---|---|
| `id` | text | The items' id field; `id` when not set. |
| `items` | list of any TOML value | The items it starts with, written here instead of a `seed` file. |
| `list` | table ([`[[collection]] list`](#collectionlist)) | How `GET path` lists the items. |
| `name` (required) | text | What `mock state`, `mock seed` and `expect mock-state` call it. |
| `new_id` | `"increment"` \| `"uuid"` | How a POST without an id gets one: `increment` (the highest number plus one) or `uuid`. When not set: `increment` if the starting items' ids are numbers, else `uuid`. |
| `path` (required) | text | The list's path, e.g. `/api/todos`; items are at `/api/todos/:id`. |
| `seed` | text | A data file (a JSON array of objects) with the items it starts with, relative to the mocks directory. |

<a id="websocket"></a>

## `[[websocket]]`

A fake WebSocket server. A page's `WebSocket` to a URL it matches opens at once and talks to it: `on_open` sends its messages, and each message the page sends gets the answer of the first `on_message` rule that matches it. Scenarios push more with `mock ws send`.

| Key | Type | What |
|---|---|---|
| `name` | text | What `mock list` and `mock log` call it; `WS <url>` when not set. |
| `on_message` | list of tables ([WebSocketRule](#websocketrule)) | Answers to the page's messages: the first rule whose `match` fits a message answers it; a message no rule fits gets no answer. |
| `on_open` | list of tables ([WebSocketAction](#websocketaction)) | What the server does once the socket is open, in order. |
| `protocol` | text | The subprotocol the socket reports (`ws.protocol`); the first one the page asked for when not set. |
| `url` (required) | text | The sockets it answers: a glob on the URL's path (`/ws/**`, `/ws/rooms/:room`), or on the full URL (`wss://push.example.com/**`). |

<a id="match"></a>

## `match`

Conditions on the request, all of which must hold.

| Key | Type | What |
|---|---|---|
| `body` | text: A glob on the whole body.; or `{ regex = … }`: A regular expression searched in the body. | The body as text: a glob, or `{ regex = "…" }`. |
| `form` | table of text; or integer; or number; or boolean | Fields of a form body (`application/x-www-form-urlencoded`) that must be present; values are globs. |
| `graphql` | table ([`match.graphql`](#graphql)) | A GraphQL request (a POST body, or a GET query string). |
| `headers` | table of text; or integer; or number; or boolean | Request headers that must be present (any case); values are globs. |
| `json` | any TOML value | The JSON body must contain this: objects match a subset of keys, `"*"` matches any value and strings are globs. Arrays must match element by element unless `json_mode = "contains"`. |
| `json_mode` | `"exact"` \| `"contains"` | How `json` matches arrays: `exact` (the default) or `contains` (each expected element matches some element). |
| `method` | text; or list of text | The HTTP method, or a list of them (any case); any method when not set. |
| `path` | text | A glob on the URL's path: `/api/products/:id` captures `id` (`{{params.id}}`), `*` matches within a segment, `**` across segments. Any host. |
| `query` | table of text; or integer; or number; or boolean | Query parameters that must be present (others may be too); values are globs (`"*"` for any value). |
| `regex` | text | A regular expression searched in the full URL (query included); named groups are parameters. |
| `url` | text | A glob on the full URL without its query, e.g. `https://api.example.com/v1/**`. |

<a id="respond"></a>

## `respond`

The answer.

| Key | Type | What |
|---|---|---|
| `abort` | text | Fail the request with a network error instead: `failed`, `connectionreset`, `timedout`, `connectionrefused`, … |
| `body` | text | A text body (a template), `text/plain` unless `headers` says otherwise. |
| `body_file` | text | A body from a data file, typed by its extension. Files with `.tmpl.` in their name are rendered as templates. |
| `delay` | text | How long to wait before answering (`300ms`, `2s`); replaces the set's latency. |
| `file` | text | A file served as it is, typed by its extension (images, fonts, …). |
| `headers` | table of text | Response headers. Values may be templates. |
| `json` | any TOML value | A JSON body; its strings may be templates. |
| `json_file` | text | A JSON body from a data file (relative to the mocks directory). The path may be a template; a file that does not exist answers 404. Files named `*.tmpl.json` have their strings rendered as templates. |
| `passthrough` | boolean | Send the request to the real server. |
| `patch` | any TOML value | With `passthrough`: deep-merge this into the real JSON response (`null` removes a key). |
| `sse` | list of tables ([SseEvent](#sseevent)) | Server-Sent Events, sent in order (`text/event-stream`), each after its own `delay`; the stream then stays open. A page's `EventSource` and `mock serve` get them one by one; a `fetch` gets them all at once. |
| `status` | integer | The HTTP status; 200 when not set. |

<a id="collectionlist"></a>

## `[[collection]] list`

| Key | Type | What |
|---|---|---|
| `filter_query` | boolean | Query parameters filter the items by the field of the same name (`?done=true`); parameters no item has are ignored. |
| `page_param` | text | The query parameter with the page number (from 1); pages only when set. |
| `page_size` | integer | The page size when the request gives none; 20 when not set. |
| `page_size_param` | text | The query parameter with the page size. |
| `wrap` | text | Answer `{ "<wrap>": [items], "total": N }` instead of the bare array. Lists always carry an `X-Total-Count` header. |

<a id="websocketrule"></a>

## WebSocketRule

An `on_message` rule: the messages it answers, and the answer.

| Key | Type | What |
|---|---|---|
| `close` | integer | Close the socket with this code (1000, or 3000-4999) instead. |
| `delay` | text | How long to wait before answering. |
| `match` | any TOML value | Which messages: a table matches JSON messages as `match.json` does (a subset; `"*"` for any value), a string is a glob on the text. Every message when not set, binary ones too (only then). |
| `reason` | text | With `close`: the reason the page sees. |
| `send` | any TOML value | The answer: a string is sent as it is, anything else as JSON text. |
| `send_base64` | text | A binary answer instead: its bytes in base64 (`AQID`), a template too (`{{body}}` of a binary message is its base64). |

<a id="websocketaction"></a>

## WebSocketAction

A WebSocket mock's action: send a message, or close the socket.

| Key | Type | What |
|---|---|---|
| `close` | integer | Close the socket with this code (1000, or 3000-4999). |
| `delay` | text | How long to wait first (`300ms`, `2s`), after the action before it. |
| `reason` | text | With `close`: the reason the page sees. |
| `send` | any TOML value | The message: a string is sent as it is, anything else as JSON text. Strings may be templates (`{{json.id}}` is the page's message's). |
| `send_base64` | text | A binary message instead: its bytes in base64 (`AQID`), a template too (`{{body}}` of a binary message is its base64). |

<a id="graphql"></a>

## `match.graphql`

| Key | Type | What |
|---|---|---|
| `operation` | text | The operation's name: `operationName`, or the name in the query. |
| `variables` | any TOML value | Variables the request must have, matched as `json` is. |

<a id="sseevent"></a>

## SseEvent

A Server-Sent Event of `respond.sse`.

| Key | Type | What |
|---|---|---|
| `comment` | text | A comment line (`: text`), which pages do not see. |
| `data` | any TOML value | The event's data: a string is sent as it is (a line each), anything else as JSON text. Strings may be templates. |
| `delay` | text | How long to wait before sending it (`1s`), after the event before it. |
| `event` | text | The event's type (`addEventListener("price", …)`); `message` when not set. |
| `id` | text | The event's id (`lastEventId`). |
| `retry` | integer | The reconnection time the event sets, in ms. |
