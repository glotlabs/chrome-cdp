<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp goto`

Load a URL in the current tab and wait for it

```text
chrome-cdp goto [OPTIONS] <URL>
```

Arguments:

- `<URL>`: The URL; `-` reads it from stdin

Options:

- `--wait <EVENT>`: What to wait for before returning; --timeout bounds it (one of `settle`, `load`, `domcontentloaded`, `networkidle`, `commit`, `none`; default `settle`)

```text
Starts the session's browser first if it has none. Without a scheme,
https:// is added (http:// for localhost and IPs). A relative URL (/login,
./next, ?q=1, #top) resolves against base_url from the config
(.chrome-cdp.toml), else against the current page. Prints the HTTP status,
final URL (after redirects), title and time taken; a 4xx/5xx status is a
warning. A page that cannot load (DNS, connection refused, certificate …)
fails with exit code 1 and says why.

Examples:
  chrome-cdp goto https://example.com
  chrome-cdp goto localhost:3000/login
  chrome-cdp goto /settings                      # with base_url, or on the same site
  chrome-cdp goto example.com --wait domcontentloaded
  chrome-cdp goto file:///tmp/report.html
  chrome-cdp goto https://example.com --tab t2 -q   # prints the final URL
```

