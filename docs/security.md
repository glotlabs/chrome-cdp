# Security model

chrome-cdp gives whoever runs it full control of a browser: every page it
opens, every cookie it holds, and any JavaScript. The model is simple:
**the browser belongs to the user who started it, and nobody else can
reach it.** This page says how that holds, what is powerful on purpose,
and what to watch for when an agent drives it. The
[threat model](threat-model.md) has the details: who is trusted, where
each defence lives in the code, and what is left.

## Nobody else can reach the browser

- **No TCP port.** chrome-cdp launches Chrome with
  `--remote-debugging-pipe`: the DevTools Protocol runs over two file
  descriptors that only the session's daemon holds. Nothing listens on
  `localhost:9222`, so other users, other containers and web pages cannot
  connect to it.
- **A user-only socket.** The CLI talks to the daemon over a Unix socket
  that is mode `0600`, in a session dir that is mode `0700`:
  `$XDG_RUNTIME_DIR/chrome-cdp/<session>/` on Linux (itself private to
  your uid), `$TMPDIR/chrome-cdp/<session>/` on macOS (a per-user
  directory). The daemon's logs and `meta.json` in it are `0600` too.
  `chrome-cdp doctor` checks the dir's owner and mode.
- **The browser dies with its session.** Chrome never outlives its daemon,
  even after `kill -9`, and the daemon exits after 30 minutes without
  commands (`open --idle-timeout`).

The exception is `chrome-cdp connect`, which attaches to a Chrome you
started with `--remote-debugging-port`. That port has no authentication:
anyone who can reach it controls that browser. Keep it on `127.0.0.1` and
close the browser when done; never expose it on a network you do not
trust. `connect` warns when the endpoint, or the WebSocket URL its
`/json/version` names, is not a loopback address.

## Chrome's sandbox stays on

Chrome runs with its sandbox, which confines what a malicious page can do
to the machine. chrome-cdp turns it off only where Chrome cannot start
with it, and warns every time:

- as root (run as a normal user instead);
- in a container that blocks user namespaces
  ([docker.md](docker.md#the-sandbox) shows how to allow them).

On Ubuntu 23.10+, AppArmor blocks the sandbox for binaries without a
profile; `chrome-cdp install --with-deps` writes the profile for
chrome-cdp's own Chrome installs rather than giving up the sandbox
([chrome-flags.md](chrome-flags.md#the-sandbox)). `chrome-cdp doctor`
says whether the sandbox is on.

## Profiles, state files and other secrets

- **Temporary profiles by default.** Each session gets a fresh profile in
  a private temp dir, deleted when the session closes. Nothing a page
  stored survives it.
- **Named profiles keep logins.** `open --profile work` keeps cookies,
  storage and saved logins in `~/.local/share/chrome-cdp/profiles/work`
  (`~/Library/Application Support/chrome-cdp/profiles/work` on macOS),
  mode `0700`. Treat the directory like a password manager's.
- **State files hold secrets.** `state save auth.json` writes session
  cookies and tokens, readable by you only (`0600`). Keep state files out
  of version control and out of CI logs and artifacts.
- **HAR files, traces and network bodies** can hold tokens and personal
  data from the requests they recorded, so chrome-cdp writes them
  readable by you only (`0600`). Screenshots, PDFs and `report.md` of a
  failed test can show secrets too. Review them before sharing.
- **Passwords stay out of snapshots.** A password field shows as
  `(password)` and never with its value, in `snapshot`, `find` and the
  action reports.
- **Credentials on the command line** are visible to other processes of
  the machine and end up in shell history. Pass them by environment
  variable (`fill label=Password "${env:APP_PASSWORD}"` in scripts) or
  from stdin (`open --http-credentials -`). The session's `history`
  shows `--http-credentials ***`.
- **HTTP credentials go to every origin that asks** unless
  `--http-credentials-origin https://app.test` names the one they are
  for; then a third-party frame or a redirect to another site gets a
  cancelled challenge instead. A proxy's challenge never gets them unless
  its origin is the one named.

## Powerful on purpose

These commands can do anything the browser can, by design. They are not a
boundary; whoever can run chrome-cdp as you can already use them:

- `eval` runs any JavaScript in a page, and `init-script` in every page of
  the session;
- `cdp` sends any DevTools Protocol call, including ones that read every
  cookie or change browser settings;
- `route`, `mock` and `block` change what pages receive;
- `expose` lets pages call back into the session's console log;
- `state load`, `cookies set` and `storage` write credentials into the
  browser.

chrome-cdp's own scripts run in an isolated JavaScript world, so a page
cannot see or tamper with them, and they cannot be confused by a page that
overrides `document.querySelector` or `Array.prototype`.

## Local servers

`mock serve` listens on `127.0.0.1` unless `--host` says otherwise;
`--host 0.0.0.0` makes the mocks reachable from the network. The E2E
runner's web servers (`[e2e.web_server]`) are your own commands, started
from the project's `chrome-cdp.toml`: running `chrome-cdp test` in a
project runs them, so treat that file like a `package.json` script.

## When an agent drives it

Pages are untrusted input. Text read from a page (`snapshot`, `text`,
`markdown`, `console`) can contain instructions written to hijack an agent
("ignore previous instructions and …"). chrome-cdp prints what the page
says; it cannot tell data from instructions. (It does keep a page from
sending escape sequences to your terminal: control characters show as
symbols such as `␛`.) When an agent browses sites you do not control:

- run it in a session with a temporary profile, not one logged in to your
  accounts (`--profile`, `--state`);
- keep the sandbox on, and prefer a container or VM for untrusted sites;
- do not let it `eval` or `cdp` text taken from a page.

## Installing and updating

- `install.sh` and `chrome-cdp upgrade` download release archives over
  HTTPS from GitHub and check each one's SHA-256 before installing it.
- `chrome-cdp install` downloads Chrome for Testing over HTTPS from
  Google's servers (or the mirror `CHROME_CDP_CFT_URL` names) into your
  cache, and checks that it starts.
- Downloads never fall back to plain HTTP, even on a redirect; a mirror
  may use plain HTTP only on this machine.
- chrome-cdp is written in Rust, uses TLS from `rustls` (no OpenSSL), and
  opens no network connections of its own besides these downloads and
  the requests you ask for (`http`, `goto`).

## Reporting a vulnerability

Please report security problems privately through a [GitHub security
advisory](https://github.com/glotlabs/chrome-cdp/security/advisories/new)
rather than a public issue.
