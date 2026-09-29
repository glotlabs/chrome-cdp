<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp http`

Send an HTTP request from the browser context, outside the page

```text
chrome-cdp http [OPTIONS] <METHOD> <URL>
```

Arguments:

- `<METHOD>`: The method: GET, POST, PUT, PATCH, DELETE, …
- `<URL>`: The URL: absolute, or a path relative to base_url

Options:

- `--json-body <JSON>`: A JSON body (sent as application/json); `-` reads stdin
- `--body <TEXT>`: A text body; `-` reads stdin
- `--header <HEADER>`: A request header, `Name: value` (repeatable)
- `--expect-status <STATUSES>`: The statuses that pass, comma-separated: 201, 2xx, 4xx, or any (default `2xx`)
- `--save <NAME>`: In a test scenario: keep the JSON answer as ${saved.NAME…}
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Sends the request the way the context's pages would: the context's route
rules and mock sets answer it first (so with mocks, a collection takes the
POST), else it goes to the server with the context's cookies (HttpOnly
too). Cookies the answer sets go into the context, so `http POST
/api/login` logs its pages in. A URL starting with / is relative to
base_url. The page never sees the request, and `expect request` does not
count it.

In a test scenario (`chrome-cdp test`), it arranges data through the API
in @before and cleans up in @after; --save NAME keeps the JSON answer for
later steps as ${saved.NAME.field}. It fails unless the status is
--expect-status (default 2xx).

Examples:
  chrome-cdp http GET /api/products
  chrome-cdp http POST /api/orders --json-body '{"sku":"kb-1","qty":1}' --save order
  chrome-cdp http DELETE /api/orders/42 --expect-status 2xx,404
  chrome-cdp http POST /api/login --json-body '{"email":"ada@example.com"}' -q
```

