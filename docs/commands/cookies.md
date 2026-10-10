<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp cookies`

List, set or clear the browser's cookies

```text
chrome-cdp cookies [OPTIONS]
chrome-cdp cookies <COMMAND>
```

Options:

- `--url <URL>`: Only the cookies a request to this URL carries
- `--domain <DOMAIN>`: Only cookies of this domain and its subdomains
- `--name <NAME>`: Only cookies with this name
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Without a subcommand, lists the cookies of every tab (they are the
browser's): name=value (cut at 60 characters; --json has all of it),
domain and path, and flags. A domain with a leading dot (.example.com)
is sent to subdomains too; without one, to that host only.

Examples:
  chrome-cdp cookies
  chrome-cdp cookies --domain example.com
  chrome-cdp cookies --url https://app.example.com/api
  chrome-cdp cookies set theme=dark
  chrome-cdp cookies clear --name session
```

## `chrome-cdp cookies set`

Set a cookie, for the active tab's URL unless given --url or --domain

```text
chrome-cdp cookies set [OPTIONS] <NAME=VALUE>
```

Arguments:

- `<NAME=VALUE>`: The cookie: NAME=VALUE

Options:

- `--url <URL>`: The URL it is for (host, path and scheme)
- `--domain <DOMAIN>`: Its domain: example.com for that host only, .example.com for its subdomains too
- `--path <PATH>`: Its path (default: /, or the --url's directory)
- `--secure`: Sent over HTTPS only
- `--httponly`: Hidden from page scripts (document.cookie)
- `--samesite <POLICY>`: When it goes with cross-site requests (one of `strict`, `lax`, `none`)
- `--expires <TIME>`: When it expires: seconds since 1970, an ISO time, or from now (30d)

```text
Prints the cookie as Chrome stored it. --expires takes seconds since 1970,
an ISO time (2026-12-31T23:59:00Z), or a time
from now (90s, 30m, 12h, 30d); without it the cookie lasts the browser
session. Chrome cuts expiry to at most 400 days. It refuses some cookies: SameSite=None needs --secure, and
__Host- cookies need --secure, --path / and a --url.

Examples:
  chrome-cdp cookies set theme=dark
  chrome-cdp cookies set session=abc123 --url https://app.example.com --httponly --secure
  chrome-cdp cookies set consent=yes --domain .example.com --expires 30d
```

## `chrome-cdp cookies clear`

Remove cookies: all of them, or those the filters pick

```text
chrome-cdp cookies clear [OPTIONS]
```

Options:

- `--url <URL>`: Only the cookies a request to this URL carries
- `--domain <DOMAIN>`: Only cookies of this domain and its subdomains
- `--name <NAME>`: Only cookies with this name

```text
Examples:
  chrome-cdp cookies clear
  chrome-cdp cookies clear --domain example.com
  chrome-cdp cookies clear --name session
```

