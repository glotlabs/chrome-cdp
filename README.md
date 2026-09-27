# chrome-cdp

A single static binary that drives Chrome over the Chrome DevTools Protocol,
built first for AI agents working from a shell. It aims to replace Playwright
for agents: one command per step, compact output, refs instead of selectors,
and errors that say what to run next.

**Status:** early development. Sessions (`open`, `connect`, `status`,
`sessions`, `close`), raw CDP calls, `install`/`uninstall` and `schema` work
so far:

```sh
chrome-cdp install                    # Chrome for Testing, if there is no Chrome
chrome-cdp open https://example.com   # starts the session's browser
chrome-cdp cdp --target page Runtime.evaluate '{"expression":"document.title"}'
chrome-cdp status
chrome-cdp close
chrome-cdp schema open      # JSON Schema of `chrome-cdp open --json`
```

Each session (`--session <name>`, default `default`) has a background daemon
that keeps its browser warm between commands; the first command that needs a
browser starts it. The daemon closes the browser and exits after 30 minutes
without commands (`open --idle-timeout 2h`, `0` for never), on `close`, or if
killed, taking Chrome with it. `connect 9222` attaches a session to a Chrome
you started with `--remote-debugging-port=9222` instead; `close` then leaves
that browser running. Session files (socket, logs, `meta.json`) live in
`$XDG_RUNTIME_DIR/chrome-cdp/<session>` on Linux and
`$TMPDIR/chrome-cdp/<session>` on macOS; `status` shows the path and
`status --logs` the end of Chrome's log.

It finds Chrome by itself: `--chrome <path>`, then `$CHROME_CDP_CHROME`, then
browsers installed in the chrome-cdp cache (`~/.cache/chrome-cdp`, or
`~/Library/Caches/chrome-cdp` on macOS; `$CHROME_CDP_CACHE` overrides it),
then a system Google Chrome or Chromium. `status` shows which one it used.

`chrome-cdp install` downloads [Chrome for
Testing](https://googlechromelabs.github.io/chrome-for-testing/) into that
cache: `--channel stable|beta|dev|canary`, `--version 154`, `--headless-shell`
for the smaller headless-only build, `--force` to download again, and `--list`
for what is installed and available. `chrome-cdp uninstall <id>` removes one.
On Linux it checks that the browser starts; if system libraries are missing it
prints the `apt-get`/`dnf` line, and `--with-deps` runs it (with sudo, asking
first). On Ubuntu 23.10+, `--with-deps` also writes the AppArmor profile that
Chrome's sandbox needs there. `--auto-install` (or `CHROME_CDP_AUTO_INSTALL=1`)
installs Chrome when none is found. `CHROME_CDP_CFT_URL` points at a mirror of
the Chrome for Testing index.

Chrome runs headless with its sandbox on and a temporary profile that is
deleted when the session closes. `open --headed` opens a window,
`--profile <name>` keeps logins between runs, `--user-data-dir <path>` picks
the directory, and `--chrome-arg=<flag>` adds a Chrome flag; `open --restart`
applies new ones to a running session. Chrome never outlives its session
daemon, even `kill -9`. [docs/chrome-flags.md](docs/chrome-flags.md) lists the flags and
explains the sandbox and profile rules.

Every command prints its result on stdout and notes and errors on stderr,
takes `--json` (one `{"ok":…}` object) and `-q`, and exits 0 ok, 1 action
failed, 2 usage, 3 timeout, 4 browser/daemon, 5 dialog. Defaults for
`--session` and `--timeout` can live in `~/.config/chrome-cdp/config.toml`
or a project's `.chrome-cdp.toml`.

## Development

```sh
cargo xtask ci      # fmt --check, gen-cdp --check, clippy -D warnings, tests
cargo xtask gen-cdp # regenerate CDP types (crates/cdp/protocol/README.md)
cargo xtask dist    # release binary in dist/ (size budget: 15 MB)
cargo deny check    # licenses and advisories
```

Integration tests need a real Chrome: set `CHROME_PATH` to a Chrome or
Chromium binary. Command output is pinned by golden files in
`crates/chrome-cdp/tests/golden/`; after an intended change, rewrite them with
`UPDATE_GOLDEN=1 cargo test -p chrome-cdp --test golden`.

## Docs

The design and roadmap live in [docs/plans/](docs/plans/00-overview.md).
