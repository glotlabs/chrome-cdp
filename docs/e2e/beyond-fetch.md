# Beyond browser fetches: WebSockets, Server-Sent Events, `mock serve`

Mock sets answer the page's `fetch` and XHR requests in the browser, with
no port and no server. Three kinds of traffic need more, and the same mock
files cover them:

- the page's **WebSockets**: a `[[websocket]]` mock;
- the page's **EventSources** (Server-Sent Events): an `sse` answer;
- requests that never pass through the browser, such as an SSR
  framework's server-side fetches: **`mock serve`**, the sets as a real
  HTTP server.

## WebSockets

```toml
[settings]
scope = ["/api/**", "/ws/**"]

[[websocket]]
name = "prices"
url = "/ws/prices/:symbol"            # a path glob, or a full URL (wss://push.example.com/**)
protocol = "v1"                       # what ws.protocol says; the page's first when not set
on_open = [
  { send = { type = "hello", symbol = "{{params.symbol}}" } },
  { delay = "1s", send = { type = "price", value = 42 } },
]
on_message = [
  { match = { type = "ping" }, send = { type = "pong", id = "{{json.id}}" } },   # JSON subset
  { match = "subscribe *", send = "ok" },                                         # a glob on the text
  { match = "bye", close = 4000, reason = "bye" },
  { send_base64 = "{{body}}" },          # no `match`: any other message, binary too (echoed)
]
```

- A page's socket to a URL the mock matches opens at once, then gets the
  `on_open` actions in order, each after its `delay`. Each message the
  page sends gets the action of the first `on_message` rule that fits it,
  or nothing. `send` sends a string as it is, anything else as JSON text
  (keys in order); `send_base64` sends a binary message, its bytes in
  base64 (`AQID`). Strings are templates: `{{params.x}}` and `{{query.x}}`
  of the socket's URL, `{{json.x}}` and `{{body}}` of the page's message,
  `{{state.x}}`, `{{seq}}`, `{{uuid}}`, `{{now}}`.
- A binary message the page sends (an `ArrayBuffer`, a typed array or a
  `Blob`) fits only rules without `match`, and its `{{body}}` is its
  base64. The page gets binary messages as its `binaryType` says.
- A scenario pushes a message with `mock ws send` (a binary one with
  `--base64`), which waits (up to `--timeout`) for a matching socket to
  open:

  ```
  mock ws send '/ws/**' '{"type":"price","value":43}'
  expect '#price' text 43
  ```

- A socket in a set's `scope` that no `[[websocket]]` takes follows the
  set's `unmatched`: with `fail` (the default) it fails (`error`, then
  `close` 1006) and the test fails as for an unmatched request; with
  `passthrough` it is real. Sockets outside every scope are real.
- `mock list` lists the WebSocket mocks with their hits; `mock log` has
  each socket (`websocket`) with the mock that took it.

## Server-Sent Events

```toml
[[mock]]
name = "news"
match = { method = "GET", path = "/api/news" }
respond = { sse = [
  { data = { title = "First" } },
  { delay = "1s", event = "headline", id = "2", data = { title = "Second" } },
  { comment = "keep-alive" },
] }
```

Each event has `data` (a string as it is, anything else as JSON), and may
have `event` (its type), `id`, `retry` (ms), `comment` and `delay` (after
the event before it). A page's `EventSource` gets them one by one, and the
stream then stays open. A mock whose answer is a 200 `text/event-stream`
body (e.g. `body_file = "data/stream.txt"`) is read as events too; any
other answer fails the `EventSource`, as it would a real one. A `fetch` of
the URL gets all the events at once, since `Fetch` answers are not
streamed.

A scenario pushes one more event with `mock sse send`, which waits (up to
`--timeout`) for a matching stream to open and have its mock's own
events, then sends it to every such stream:

```
click 'Refresh prices'
mock sse send /api/prices '{"price":43}' --event price --id 7
expect '#price' text 43
```

`DATA` is sent as it is (`-` reads stdin); its newlines make several
`data:` lines, so the page gets them back as newlines.

## How: a stand-in in the page

Chrome's request interception sees neither WebSocket traffic nor a stream
it could answer bit by bit. So while a context has mock sets, chrome-cdp
replaces the page's `WebSocket` and `EventSource` with stand-ins, in every
document loaded (or reloaded) after `mock use`, and in the documents
already open for sockets they open later. Its dedicated, shared and
service workers get them too: a worker started later before its own
script runs, one running already for sockets it opens later. Each stand-in asks the daemon
what it gets (a mock, a failure, or the real thing, which it then wraps),
so real sockets take one more round trip to start. `mock off` gives
sockets opened later the browser's own classes back.

Limits: `mock ws send` only reaches sockets a `[[websocket]]` answers,
and `mock sse send` only streams an `sse` mock answers.

## `mock serve`: the mocks as an HTTP server

SSR frameworks (Next.js, Remix, SvelteKit load functions, Nuxt) fetch on
the server, where no browser can intercept. `mock serve` answers those
requests with the same mock files:

```
chrome-cdp mock serve --port 4010             # default_mocks
chrome-cdp mock serve checkout --no-defaults  # other sets
API_URL=http://localhost:4010 npm run dev     # point the app's server at it
```

It prints each request it answers (the same lines as `mock log`) until
Ctrl-C, and reads mock files again when they change. Answers are as in the
browser, except what only a browser can do: `abort` closes the connection,
`passthrough` answers 502 (there is no real server behind), and a request
no set owns answers 404. `sse` answers are streamed.

### In a test run

```toml
[e2e.web_server]
command = "npm run dev -- --port 3000"
url = "http://localhost:3000"
mock_server = { port = 4010 }           # host = "0.0.0.0" to listen on every interface
env = { API_URL = "http://localhost:4010" }
```

`chrome-cdp test` starts the mock server before the web server (which also
gets its URL as `CHROME_CDP_MOCK_URL`; with `port = 0`, the only way to
know it) and stops it at the end. It is not started with a real backend.

**State per test, if the app forwards a header.** The browser's requests
to the web servers, `base_url` and the mock server carry
`x-chrome-cdp-test: <test id>`, and the mock server keeps state per value
of that header, made from the test's own sets (`default_mocks` plus its
`@mocks`), with its frozen time. An app that forwards the header from the
incoming request to its API calls gets per-test state, as browser-side
mocks have:

```js
// e.g. in a Next.js server component or a SvelteKit load function
const test = request.headers.get('x-chrome-cdp-test');
const res = await fetch(`${process.env.API_URL}/api/cart`, {
  headers: test ? { 'x-chrome-cdp-test': test } : {},
});
```

**The limitation:** an app that does not forward it shares one state (the
`default_mocks`, with no `@mocks` of any test) across all tests, so
collections, sequences and named values carry over between tests and
parallel tests see each other's writes. Keep such mocks stateless, or
forward the header.

Unmatched requests to the mock server in the scope of a `fail` set fail
the test that sent them (when they carry its header), with the closest
mocks, as browser requests do. Each test's `requests.txt` ends with the
mock server's requests of the test (`== mock server …`).
