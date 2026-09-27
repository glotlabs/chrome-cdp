# chrome-cdp

A single static binary that drives Chrome over the Chrome DevTools Protocol,
built first for AI agents working from a shell. It aims to replace Playwright
for agents: one command per step, compact output, refs instead of selectors,
and errors that say what to run next.

**Status:** early development; no commands work yet.

## Development

```sh
cargo xtask ci      # fmt --check, gen-cdp --check, clippy -D warnings, tests
cargo xtask gen-cdp # regenerate CDP types (crates/cdp/protocol/README.md)
cargo xtask dist    # release binary in dist/ (size budget: 15 MB)
cargo deny check    # licenses and advisories
```

Integration tests need a real Chrome: set `CHROME_PATH` to a Chrome or
Chromium binary.

## Docs

The design and roadmap live in [docs/plans/](docs/plans/00-overview.md).
