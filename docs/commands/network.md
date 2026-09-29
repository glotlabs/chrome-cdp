<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp network`

Show the page's requests, or one in full, or clear the log

```text
chrome-cdp network [OPTIONS]
chrome-cdp network <COMMAND>
```

Options:

- `--failed`: Only requests that failed, or got a status of 400 or more
- `--type <TYPES>`: Only these resource types, comma-separated: document, script, stylesheet, image, font, media, xhr, fetch, websocket, other, …
- `--url <PATTERN>`: Only URLs that match: a glob (`**/api/*`), a `/regex/`, or a part of the URL
- `--method <METHOD>`: Only this HTTP method (GET, POST, …)
- `--status <STATUS>`: Only these statuses, comma-separated: 404, 4xx or 500-599
- `--since <WHEN>`: Where to start: `last` (what this command has not shown yet), a request id such as 12 (0 for all), a duration such as 30s (started in the last 30 seconds), or a mark (see `mark`) (default `last`)
- `--limit <N>`: Show at most this many requests, the newest (0 = all) (default `50`)
- `-f, --follow`: Keep printing new requests (and pending ones as they end) as they come, until Ctrl-C (or --timeout, if given)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Every tab records its requests from the moment it opens (the newest 2,000;
see `open --network-buffer`). By default only requests this command has not
shown before are printed; one shown while pending is shown again when it
ends. Each line reads: #id METHOD STATUS URL TYPE SIZE TIME.

Examples:
  chrome-cdp network
  chrome-cdp network --failed --since 0            # every failed request so far
  chrome-cdp network --type xhr,fetch --url '**/api/**'
  chrome-cdp network --status 4xx --method POST
  chrome-cdp network --follow --type fetch         # watch requests as they come
  chrome-cdp network show 42 --body                # the response body of #42
```

## `chrome-cdp network show`

Show one request in full: status, sizes, timing, initiator, and its headers and bodies on request

```text
chrome-cdp network show [OPTIONS] <ID>
```

Arguments:

- `<ID>`: The request's id, as `network` lists it: 42 or #42

Options:

- `--headers`: List its request and response headers
- `--body`: Fetch and show the response body
- `--request-body`: Show the request body
- `-o, --output <PATH>`: Save the response body to this file (decoded: an image as its bytes) and print its path, instead of showing it
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Bodies are fetched from Chrome only when asked for. Chrome keeps them while
its buffer has room and the page that loaded them is open; if it no longer
has one, the output says so. JSON bodies are pretty-printed.

Examples:
  chrome-cdp network show 42
  chrome-cdp network show 42 --body
  chrome-cdp network show 42 --headers --request-body
  chrome-cdp network show 42 --body -q > body.json   # just the body
  chrome-cdp network show 7 -o logo.png              # save a body, binary too
```

## `chrome-cdp network clear`

Forget the tab's requests so far (ids go on from where they were)

```text
chrome-cdp network clear [OPTIONS]
```

```text
Examples:
  chrome-cdp network clear
  chrome-cdp network clear --tab t2
```

