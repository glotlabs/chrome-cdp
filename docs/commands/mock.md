<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp mock`

Answer the page's requests from mock files: use, list, log, explain

```text
chrome-cdp mock [OPTIONS] <COMMAND>
```

```text
A mock set is a TOML file in the project's mocks directory (`mocks` of
[e2e] in chrome-cdp.toml, else ./e2e/mocks): `mock use base` reads
e2e/mocks/base.toml. Each [[mock]] matches requests and answers them. Data
files are relative to the mocks directory:

  [settings]
  scope = ["/api/**"]      # requests the set owns; unmatched ones get a 501
  [[mock]]
  name = "product by id"
  match = { method = "GET", path = "/api/products/:id" }
  respond = { json_file = "data/products/{{params.id}}.json" }

Match on: method, path (:param, *, **), url, regex, query, headers, json
(a subset; "*" matches anything), form (urlencoded or multipart; a
file field's value is its file name), body, graphql. Answer with
status, headers and one of json, json_file, body, body_file or file; or
with abort, or passthrough (plus patch); delay holds the answer. Templates:
{{params.x}} {{query.x}} {{headers.x}} {{json.x}} {{form.x}}
{{form_files.x.filename}} {{uuid}} {{now}} {{seq}} {{faker.name}}
{{state.x}}. A json_file that does not exist
answers 404. `chrome-cdp schema mocks` prints the file format's JSON
Schema.

State: `sequence = [{…}, {…}]` answers in turn (after_sequence =
repeat-last, cycle or unmatched). [state] declares named values that
mocks require (`when = { logged_in = true }`) and change (`set_state`,
`unset_state`). A [[collection]] is a fake REST resource (name, path,
seed = "data/todos.json"): GET lists, POST adds, GET/PUT/PATCH/DELETE
path/:id; a path with parameters (/api/users/:user/todos) keeps each
user's items apart by their `user` field. `mock state` shows it all,
`mock reset` starts it over.

Beyond fetches: a [[websocket]] (url, on_open, on_message) answers the
page's WebSockets (`mock ws send` pushes more), `respond = { sse = [...] }`
streams Server-Sent Events to its EventSources, and `mock serve` answers
server-side requests as a real HTTP server.

Sets applied later are tried first, and `route` rules come before any
mock. Mock files are read again when they change. The commands need a
running session, except `mock explain` and `mock list --available`.

Examples:
  chrome-cdp mock use base checkout
  chrome-cdp mock list
  chrome-cdp mock log --unmatched
  chrome-cdp mock explain GET /api/products/7
  chrome-cdp mock state todos
  chrome-cdp mock serve --port 4010
  chrome-cdp mock off
```

## `chrome-cdp mock use`

Answer requests with mock sets, instead of the sets in use

```text
chrome-cdp mock use [OPTIONS] [SET]...
```

Arguments:

- `<SET>`: Mock sets to apply after the defaults: names in the mocks directory, or paths to .toml files

Options:

- `--add`: Layer the sets onto the ones in use (without the defaults)
- `--no-defaults`: Leave out the project's default_mocks
- `--dir <DIR>`: The mocks directory (default: `mocks` of [e2e], else ./e2e/mocks)

```text
The sets are the project's `default_mocks` ([e2e] in chrome-cdp.toml)
followed by the SETs named, so `mock use checkout` layers checkout onto the
defaults, as a scenario's `@mocks checkout` does. A SET is a name in the
mocks directory (`errors/api-down` is errors/api-down.toml) or a path to a
.toml file. Later sets are tried first. Every file is checked before any
is used; a problem is reported at file:line:col.

Examples:
  chrome-cdp mock use                      # the default_mocks
  chrome-cdp mock use checkout
  chrome-cdp mock use --add errors/api-down
  chrome-cdp mock use --no-defaults ./tmp/experiment.toml
```

## `chrome-cdp mock off`

Stop answering requests with mock sets (all, or those named)

```text
chrome-cdp mock off [OPTIONS] [SET]...
```

Arguments:

- `<SET>`: The sets to turn off; all when none is named

```text
Without mock sets or routes left, requests are no longer intercepted.

Examples:
  chrome-cdp mock off
  chrome-cdp mock off errors/api-down
```

## `chrome-cdp mock list`

List the mock sets in use, their mocks and hits

```text
chrome-cdp mock list [OPTIONS]
```

Options:

- `--available`: List the sets in the mocks directory instead (needs no session)
- `--dir <DIR>`: With --available: the mocks directory
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Sets are listed in the order they are tried, each with its mocks:
file:line, name, what it matches, what it answers, and how many requests
it answered. --available lists the sets in the mocks directory instead;
it needs no session.

Examples:
  chrome-cdp mock list
  chrome-cdp mock list --available
```

## `chrome-cdp mock log`

The requests the mocks answered, and those no mock answered

```text
chrome-cdp mock log [OPTIONS]
```

Options:

- `--unmatched`: Only the requests no mock answered (in a set's scope)
- `--all`: Every request routing saw, not only mocked and unmatched ones
- `--limit <N>`: Show at most this many requests, the newest (0 = all) (default `20`)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Each line reads: request number, tab, method, URL, type, then the mock
that answered it (file:line and name) and what it answered; or `no mock
in SET` for a request in a set's scope that no mock matched. --all adds
the requests no set had anything to do with.

Examples:
  chrome-cdp mock log
  chrome-cdp mock log --unmatched
  chrome-cdp mock log --all --limit 100
```

## `chrome-cdp mock reset`

Start the mocks' state over: counters, sequences, named values, collections

```text
chrome-cdp mock reset [OPTIONS]
```

```text
`{{seq}}` counts from 1 again, sequences start at their first answer,
named values are as [state] declares them, collections hold their seed
items again (seed files are read again), and `{{uuid}}` and
`{{faker.*}}` give the same values as after `mock use`.

Examples:
  chrome-cdp mock reset
```

## `chrome-cdp mock explain`

Which mock answers a request, and why the others do not

```text
chrome-cdp mock explain [OPTIONS] <METHOD> <URL>
```

Arguments:

- `<METHOD>`: The request's method: GET, POST, …
- `<URL>`: Its URL: absolute, or a path relative to base_url

Options:

- `--json-body <JSON>`: A JSON body (sent as application/json); `-` reads stdin
- `--body <TEXT>`: A text body; `-` reads stdin
- `--header <HEADER>`: A request header, `Name: value` (repeatable)
- `--mocks <SET>`: Explain with these sets (repeatable, or comma-separated) instead of the session's
- `--dir <DIR>`: With --mocks or the defaults: the mocks directory
- `--limit <N>`: Show at most this many mocks (0 = all) (default `30`)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Checks every mock of the sets in use, in the order they are tried, and
says which one answers the request and, for each other one, the first
condition that does not hold. A URL starting with / is relative to
base_url. Without a session using mock sets, it explains with --mocks, or
the project's default_mocks. Nothing is sent, and no hit is counted.

Examples:
  chrome-cdp mock explain GET /api/products/7
  chrome-cdp mock explain POST /api/login --json-body '{"email":"ada@example.com"}'
  chrome-cdp mock explain GET 'https://api.example.com/v1/cart?user=7' --mocks base
```

## `chrome-cdp mock state`

Show the mocks' named values and collections, or one of them

```text
chrome-cdp mock state [OPTIONS] [NAME]
```

Arguments:

- `<NAME>`: A named value or collection to show; all of them when not given

Options:

- `--set <NAME=VALUE>`: Change a named value first, NAME=VALUE (repeatable); VALUE is JSON, else text
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Without a NAME: every named value (name = value) and every collection
with its item count. With a NAME: that value, or the collection's items,
as JSON (-q prints it on one line). --set changes named values first;
the VALUE is JSON (true, 3, ["a"], null to remove it), else text. Only
names a [state] declares, or a mock has set, can be set.

Examples:
  chrome-cdp mock state
  chrome-cdp mock state todos
  chrome-cdp mock state --set logged_in=true
  chrome-cdp mock state user --set user=ada@example.com
```

## `chrome-cdp mock seed`

Fill a collection with other items, or its seed items again

```text
chrome-cdp mock seed [OPTIONS] <COLLECTION> [FILE]
```

Arguments:

- `<COLLECTION>`: The collection
- `<FILE>`: A JSON data file with the items; `-` reads them from stdin. Without it, the collection's own seed items

```text
FILE is a JSON array of objects, each with the collection's id field: a
path in the mocks directory (like a mock file's data files), else
relative to the current directory; `-` reads the JSON from stdin. Without
a FILE the collection starts over with its own seed items. Other
collections and named values are left as they are.

Examples:
  chrome-cdp mock seed todos data/many-todos.json
  chrome-cdp mock seed todos
  echo '[{"id":1,"title":"Only one"}]' | chrome-cdp mock seed todos -
```

## `chrome-cdp mock record`

Write a mock set from the requests the session sent (to a real backend)

```text
chrome-cdp mock record [OPTIONS]
```

Options:

- `--scope <GLOB>`: Record the requests these globs match (repeatable, or comma-separated): paths (`/api/**`) or full URLs. Default: the page's fetch and XHR requests, WebSockets and EventSources
- `-o, --output <FILE>`: The mock file to write (default: recorded.toml in the mocks directory)
- `--match-bodies`: Tell requests to one path apart by their bodies: match the JSON or form fields that do not look generated (ids, the time, tokens)
- `--force`: Overwrite the mock file (and its data files) if it exists
- `--dir <DIR>`: The mocks directory (default: `mocks` of [e2e], else ./e2e/mocks)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Reads the session's network log (every tab of the context, or --tab's),
keeps the requests in --scope (globs as a set's scope takes them; without
it, the page's fetch and XHR requests, WebSockets and EventSources), and
writes one [[mock]] per
distinct request: method, path, query (and a GraphQL operation). A request
sent more than once with other answers becomes a `sequence`. Bodies go to
data files in data/<set>/ (small ones inline); volatile headers (date,
etag, set-cookie unless a later request sent the cookie, …) are left out;
ids and timestamps get a comment where a template would do. The file is
checked as `mock use` would read it.

--match-bodies tells requests to one path apart by their bodies (two
logins as different users): their mocks match `json` (or `form`, or a
GraphQL request's variables) with the body's stable fields, leaving out
fields whose values differ and look generated (UUIDs, the time, long
tokens). Requests with equal stable fields still form a sequence.

A WebSocket becomes a [[websocket]]: the server's messages before the
page's first are on_open, and each page message the server answered is an
on_message rule (matching a JSON message's stable fields, other text
exactly) that sends the first answer, with the delays seen. An
EventSource's messages become an `sse` answer. Binary messages, and
answers after the first, are left out and listed.

Browse first, then record: bodies are asked of Chrome, which keeps them
while the page that loaded them is open.

Examples:
  chrome-cdp mock record --scope '/api/**'
  chrome-cdp mock record --scope '/api/**' --match-bodies
  chrome-cdp mock record --scope '/api/**' -o e2e/mocks/checkout.toml --force
  chrome-cdp mock record --scope 'https://api.example.com/**' --tab t2
```

## `chrome-cdp mock import`

Write a mock set from a HAR file, as `mock record` does

```text
chrome-cdp mock import [OPTIONS] <FILE>
```

Arguments:

- `<FILE>`: The HAR file; `-` reads it from stdin

Options:

- `--scope <GLOB>`: Import the requests these globs match (repeatable, or comma-separated): paths (`/api/**`) or full URLs. Default: the page's fetch and XHR requests, WebSockets and EventSources
- `-o, --output <FILE>`: The mock file to write (default: &lt;FILE's name&gt;.toml in the mocks directory)
- `--match-bodies`: Tell requests to one path apart by their bodies: match the JSON or form fields that do not look generated (ids, the time, tokens)
- `--force`: Overwrite the mock file (and its data files) if it exists
- `--dir <DIR>`: The mocks directory (default: `mocks` of [e2e], else ./e2e/mocks)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
The same conversion as `mock record`, from a HAR file (`har stop`, the
DevTools Network panel's "Save all as HAR", or any HAR recorder). The output is
<mocks dir>/<file's name>.toml unless -o says otherwise. --match-bodies
tells requests to one path apart by their bodies' stable fields.

Examples:
  chrome-cdp mock import requests.har --scope '/api/**'
  chrome-cdp mock import requests.har --match-bodies
  chrome-cdp mock import requests.har -o e2e/mocks/recorded.toml --force
```

## `chrome-cdp mock check`

Lint mock files, and check them against an OpenAPI spec

```text
chrome-cdp mock check [OPTIONS] [SET]...
```

Arguments:

- `<SET>`: The sets to check (names in the mocks directory, or .toml paths); every set in the mocks directory if none

Options:

- `--openapi <FILE>`: Check the mocks against this OpenAPI spec (JSON or YAML) instead of the config's `openapi`
- `--dir <DIR>`: The mocks directory (default: `mocks` of [e2e], else ./e2e/mocks)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Checks every mock file in the mocks directory (or the SETs named) for what
`mock use` would refuse (format, missing data files, templates), mocks an
earlier one always answers for, collections a mock always answers for,
collections of one name in two files, and state names no [state]
declares. With an OpenAPI spec (--openapi, or `openapi` in [e2e]), each
mock's path, method, status and JSON body must fit the operation it
stands for, which catches mocks the real API has moved away from, and
each operation of the spec no mock or collection answers is a warning
(openapi-uncovered; the first 20, then a count). Sets scoped to another
origin than the spec's servers (a third party's) are left out of it.

Each problem reads file:line:col, severity, rule and message. Errors exit
1; warnings alone exit 0. `chrome-cdp test` runs the check on the sets it
uses before it starts, and stops on errors (`mock_check = "warnings"`
in [e2e] stops on warnings too, "off" skips it); operations with no
mock are one line there.

Examples:
  chrome-cdp mock check
  chrome-cdp mock check base checkout --openapi api/openapi.yaml
  chrome-cdp mock check --json
```

## `chrome-cdp mock gen`

Write a starting mock set from an OpenAPI spec: one mock per operation

```text
chrome-cdp mock gen [OPTIONS]
```

Options:

- `--openapi <FILE>`: The OpenAPI spec (JSON or YAML; default: `openapi` of [e2e])
- `-o, --output <FILE>`: The mock file to write (default: generated.toml in the mocks directory)
- `--force`: Overwrite the mock file (and its data files) if it exists
- `--dir <DIR>`: The mocks directory (default: `mocks` of [e2e], else ./e2e/mocks)
- `--seed <N>`: Seeds the generated values: the same spec and seed give the same file (default `1`)
- `--scope <GLOB>`: Only the operations whose path these globs match (repeatable, or comma-separated): the spec's path (`/pets/**`) or the mock's, with the server's path
- `--collections`: Make each list and item path pair (`/pets`, `/pets/{id}`) a [[collection]] instead of mocks
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
For a project with an OpenAPI spec (3.x or Swagger 2.0, JSON or YAML) and
no mocks yet. Each operation gets a [[mock]] matching its method and path
(`{id}` becomes `:id`, under the first server's path) that answers its
lowest 2xx status (200 when the spec lists only `default`). The JSON body
is the spec's example (the media type's `example`, then the first of its
`examples`, then the schemas' own examples), and the rest is generated
from the schema: `default`, `enum[0]` or `const`, else a value of the
type and format (`name-1`, `user1@example.com`, a seeded UUID, a number
in range). Values are the same for the same spec and --seed. Bodies over
100 characters go to data files in data/<set>/.

--collections makes a list path and its item path (`/pets` and
`/pets/{id}`, both with a GET) one [[collection]] of three items instead
of mocks, so create-then-list flows work; a nested one
(`/users/{user}/todos`) keeps each parent's items apart.

What the generator cannot make fit the spec (a `pattern`, a binary
answer) is a `# TODO:` comment above its mock, and listed. The [settings]
scope is the servers' paths (`/v1/**`), else the paths' first segments.
--scope generates only the operations whose path a glob matches.

Examples:
  chrome-cdp mock gen
  chrome-cdp mock gen --openapi api/openapi.yaml --collections
  chrome-cdp mock gen --scope '/pets/**' -o e2e/mocks/pets.toml --force
```

## `chrome-cdp mock serve`

Serve mock sets as a real HTTP server, for server-side fetches (SSR)

```text
chrome-cdp mock serve [OPTIONS] [SET]...
```

Arguments:

- `<SET>`: Mock sets to serve after the defaults: names in the mocks directory, or paths to .toml files

Options:

- `--port <PORT>`: The port to listen on (0 picks a free one) (default `4010`)
- `--host <ADDR>`: The address to listen on (default `127.0.0.1`)
- `--no-defaults`: Leave out the project's default_mocks
- `--dir <DIR>`: The mocks directory (default: `mocks` of [e2e], else ./e2e/mocks)
- `--upstream <URL>`: Forward `passthrough` requests to this real server (its path is a prefix), instead of answering 502
- `--upstream-timeout <DURATION>`: How long a forwarded request may take before it answers 502 (default `30s`)

```text
Answers HTTP requests with the same mocks as `mock use` (the default_mocks
followed by the SETs named), for requests that never pass through the
browser: the fetches of an SSR framework's server (Next.js, Remix,
SvelteKit load functions). Point the app at it (API_URL=http://localhost:4010).
It runs until Ctrl-C and prints each request it answered; mock files are
read again when they change.

State (counters, sequences, named values, collections) is shared by all
requests, except that a request with an `x-chrome-cdp-test: <id>` header
gets state of its own for that id. `chrome-cdp test` sends the header with
its browser requests; an app that forwards it to its API calls gets state
per test. `abort` closes the connection, a request no set owns answers
404, and an `sse` answer streams its events.

`passthrough` (a mock's, or a set's unmatched = "passthrough") answers
502, as there is no real server behind, unless --upstream URL names one:
then the request goes there (same method, path under URL's path, query,
headers and body; Host is the upstream's), `patch` is merged into its JSON
answer, and the log says `passthrough → upstream 200`. An upstream that
does not answer within --upstream-timeout is a 502 naming it. Mock some
endpoints, and let the real server answer the rest.

A WebSocket upgrade a [[websocket]] mock matches is served by it, and the
log shows each socket's open, its messages in (←) and out (→), and its
close (`WS /ws/prices open`). `mock ws send --serve URL` and `mock sse
send --serve URL` push to its open sockets and streams, through control
requests under /__chrome-cdp/ that it takes from this machine only.

In a test run, `mock_server = { port = 4010 }` in [e2e.web_server] starts
one before the web server, with each test's own mock sets (`upstream =
"http://localhost:8080"` and `upstream_timeout = "30s"` forward too).

Examples:
  chrome-cdp mock serve --port 4010
  chrome-cdp mock serve checkout --no-defaults --port 0
  chrome-cdp mock serve base --host 0.0.0.0 --port 4010
  chrome-cdp mock serve third-party --upstream http://localhost:8080
```

## `chrome-cdp mock ws`

Talk to the page's mocked WebSockets

```text
chrome-cdp mock ws [OPTIONS] <COMMAND>
```

### `chrome-cdp mock ws send`

Send a message to the page's sockets that a [[websocket]] mock answers

```text
chrome-cdp mock ws send [OPTIONS] <URL> <MESSAGE>
```

Arguments:

- `<URL>`: Which sockets: a path glob (/ws/\*\*), a full-URL glob, or \* for all
- `<MESSAGE>`: The message, sent as it is; `-` reads stdin

Options:

- `--base64`: MESSAGE is base64: send its bytes as a binary message
- `--serve <URL>`: Push to the sockets of the mock server at URL (`mock serve`) instead of the browser's
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Hands MESSAGE, as it is (with --base64, its bytes as a binary message), to
every open socket of the context whose URL the glob matches (a path glob
such as /ws/**, a full-URL glob, or * for all), as if the fake server sent
it. Waits (up to --timeout) for such a socket to open. Only sockets a
[[websocket]] mock answers can be sent to: the page's and its workers'
`WebSocket`s talk to the mocks after `mock use`.

With --serve URL it goes to the sockets of that `mock serve` instead (a
client that is not a browser page: an SSR server, a native app). In a
`test` run with a mock server, it goes to both the browser's sockets and
the test's sockets of the mock server.

Examples:
  chrome-cdp mock ws send '/ws/**' '{"type":"price","value":42}'
  chrome-cdp mock ws send '*' hello
  chrome-cdp mock ws send /ws/feed AQID --base64    # a binary message: bytes 1, 2, 3
  echo '{"type":"bye"}' | chrome-cdp mock ws send /ws/prices -
  chrome-cdp mock ws send --serve http://127.0.0.1:4010 '/ws/**' hello
```

## `chrome-cdp mock sse`

Push events to the page's mocked EventSources

```text
chrome-cdp mock sse [OPTIONS] <COMMAND>
```

### `chrome-cdp mock sse send`

Push an event to the page's EventSources that an `sse` mock answers

```text
chrome-cdp mock sse send [OPTIONS] <URL-GLOB> <DATA>
```

Arguments:

- `<URL-GLOB>`: Which streams: a path glob (/api/prices), a full-URL glob, or \* for all
- `<DATA>`: The event's data, sent as it is; `-` reads stdin

Options:

- `--event <TYPE>`: The event's type (the page's `addEventListener(TYPE)`); `message` when not given
- `--id <ID>`: The event's id (the stream's `lastEventId`)
- `--retry <MS>`: The reconnection time the event sets, in ms
- `--serve <URL>`: Push to the streams of the mock server at URL (`mock serve`) instead of the browser's
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Hands one event to every open EventSource of the context whose URL the
glob matches (a path glob such as /api/prices, a full-URL glob, or * for
all), after the mock's own `sse` events, as if the server sent it. DATA is
sent as it is (`-` reads stdin); its newlines make several `data:` lines,
so the page gets them back as newlines. Waits (up to --timeout) for such
a stream to open. Only streams an `sse` mock answers can be pushed to:
the page's `EventSource`s talk to the mocks after `mock use`.

With --serve URL it goes to the streams of that `mock serve` instead. In
a `test` run with a mock server, it goes to both the browser's streams
and the test's streams of the mock server.

Examples:
  chrome-cdp mock sse send /api/prices '{"price":42}'
  chrome-cdp mock sse send '/api/**' 'sold out' --event stock --id 7
  chrome-cdp mock sse send '*' bye --retry 5000
  echo '{"price":43}' | chrome-cdp mock sse send /api/prices -
  chrome-cdp mock sse send --serve http://127.0.0.1:4010 /api/prices '{"price":44}'
```

