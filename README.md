# chrome-cdp

A single static binary that drives Chrome over the Chrome DevTools Protocol,
built first for AI agents working from a shell. It aims to replace Playwright
for agents: one command per step, compact output, refs instead of selectors,
and errors that say what to run next.

**Status:** early development. Only `chrome-cdp --version`, `chrome-cdp
schema` and a temporary raw CDP call work so far:

```sh
chrome-cdp __direct cdp Browser.getVersion --chrome /path/to/chrome
chrome-cdp __direct cdp --target page Runtime.evaluate '{"expression":"1+1"}' --chrome /path/to/chrome
chrome-cdp schema version   # JSON Schema of `chrome-cdp --version --json`
```

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
