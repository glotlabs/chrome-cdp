<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp route`

Mock, block or change the page's requests

```text
chrome-cdp route [OPTIONS] <PATTERN>
```

Arguments:

- `<PATTERN>`: Which URLs: a glob (`**/api/*`), a `/regex/`, or a part of the URL

Options:

- `--method <METHODS>`: Only these HTTP methods (GET, POST, …), comma-separated
- `--resource-type <TYPES>`: Only these resource types, comma-separated: document, script, stylesheet, image, font, media, xhr (or fetch: Chrome does not tell them apart here), websocket, other, …
- `--times <N>`: Remove the rule after it handled this many requests
- `--delay <DURATION>`: Hold each matched request this long first (e.g. 2s, 500ms)
- `--status <CODE>`: Answer with this status (200 unless given)
- `--body <TEXT>`: Answer with this text (`-` reads stdin); text/plain unless a --header sets Content-Type
- `--json-body <JSON>`: Answer with this JSON (`-` reads stdin), as application/json
- `--body-file <PATH>`: Answer with this file, read now; its extension sets Content-Type
- `--header <NAME:VALUE>`: A response header, as 'Name: value'. Repeat for more
- `--abort <REASON>`: Fail the request with this network error (default `failed`): failed, aborted, timedout, accessdenied, connectionrefused, connectionreset, namenotresolved, internetdisconnected, blockedbyclient, …
- `--set-header <NAME:VALUE>`: Send the request on with this header set, as 'Name: value'. Repeat for more
- `--remove-header <NAME>`: Send the request on without this header. Repeat for more
- `--rewrite-url <URL>`: Send the request to this URL instead (the page does not see it)
- `--patch-json <JSON>`: Send the request on and merge this JSON into the response's JSON (`-` reads stdin): objects merge key by key, null removes a key, anything else replaces

```text
Adds a rule for requests whose URL matches PATTERN: a glob (`**/api/*`,
where * stops at / and ** does not), a /regex/, or a part of the URL. Rules
belong to the browser context (the session's own, unless --context): they
apply to every tab, popup, frame and worker in it, the ones opened later
too, before the page's first script runs (Chrome lets a shared worker be
intercepted only as it starts, and under load may start it first). The
first matching rule, in the order they were added, handles a request; the
rest go on untouched. `routes` lists them, `unroute` removes them.

One action per rule:
  answer it:      --json-body, --body, --body-file, --status, --header
  fail it:        --abort [REASON]  (failed, blockedbyclient, timedout, …)
  change it:      --set-header, --remove-header, --rewrite-url
  patch it:       --patch-json (the real response, with the patch merged in)
--delay holds a matched request first (alone: then sends it on).

While routes exist, requests skip service workers and the HTTP cache.
Mocked responses to another origin allow the page's origin (CORS).

Examples:
  chrome-cdp route '**/api/items' --json-body '[]'
  chrome-cdp route /api/login --method POST --status 401 --json-body '{"error":"bad password"}'
  chrome-cdp route '**/api/cart' --patch-json '{"items": [], "total": 0}'
  chrome-cdp route /api/ --status 500 --body 'down' --times 1
  chrome-cdp route '**/*.js' --delay 2s
  chrome-cdp route analytics.example.com --abort
  chrome-cdp route /api/ --set-header 'Authorization: Bearer test' --remove-header cookie
  chrome-cdp route 'https://prod.example.com/api/**' --rewrite-url http://localhost:3000/api
  chrome-cdp route /report.pdf --body-file ./fixtures/report.pdf
```

