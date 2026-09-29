# Development

How to build, test and change chrome-cdp itself. The design and roadmap
live in [plans/](plans/00-overview.md); [releasing.md](releasing.md) is how
a release is cut.

```sh
cargo xtask ci      # fmt --check, gen-cdp --check, clippy -D warnings, tests
cargo xtask e2e --repeat 20  # the whole suite 20 times, no retries (flakiness gate)
cargo xtask leaks   # no Chrome, daemon or socket file of chrome-cdp is left
cargo xtask soak    # 1,000 random commands, and an hour of logging (needs CHROME_PATH)
cargo xtask gen-cdp # regenerate CDP types (crates/cdp/protocol/README.md)
cargo xtask build-js # rebuild the injected helper from TypeScript (needs npx)
cargo xtask dist --target x86_64-unknown-linux-musl  # release binary in dist/;
                    # fails at +20% over its size in xtask/binary-sizes.txt
cargo xtask chrome stable  # install CfT stable (or beta, min, 150, system), print its path
cargo xtask release archive --target x86_64-unknown-linux-musl  # dist/*.tar.gz + .sha256;
                    # also manifest, notes, formula, npm, check (docs/releasing.md)
cargo xtask fixtures # serve testdata/site on two local origins
cargo xtask docs    # regenerate the command reference in docs/commands/
cargo deny check    # licenses, bans, sources and advisories
cargo audit         # RustSec advisories (needs cargo-audit)
cargo xtask fuzz --time 60  # each fuzz target for 60 s (needs cargo-fuzz and nightly)
cargo +nightly fuzz run rpc_decode  # one target: rpc_decode (the CLI <-> daemon
                    # decoder) or parse_script (the .cdp parser)
```

Integration tests need a real Chrome: set `CHROME_PATH` to a Chrome or
Chromium binary (`export CHROME_PATH=$(cargo xtask chrome stable)`). CI runs
them against Chrome for Testing stable on Linux and macOS, x86_64 and arm64,
and on Linux also against the system Google Chrome and the oldest supported
Chrome; a weekly job runs them against beta. They load pages from the fixture site in `testdata/site`,
served by `crates/fixtures` on two sites (`127.0.0.1` and `localhost`, so
frames from one in the other run out of process) and on an HTTPS origin with
a self-signed certificate; `cargo xtask fixtures`
serves it for poking at by hand, and its index lists every folder. It
includes the same todo app in React, Vue and Svelte (`testdata/site/apps/`),
prebuilt so tests need no Node. Tests run with
[cargo-nextest](https://nexte.st) (`.config/nextest.toml`): one process per
test and retries off, so a flaky test fails. The binary's tests use the
harness in `crates/chrome-cdp/tests/common/`: a `Sandbox` per test (its own
session and temp dirs) and a `Session` guard that closes its session on
drop; dropping the sandbox fails the test if a daemon, a Chrome or a socket
file of it is left. Command output is pinned by golden files in
`crates/chrome-cdp/tests/golden/`; after an intended change, rewrite them with
`UPDATE_GOLDEN=1 cargo test -p chrome-cdp --test golden` (and
`golden/cli.txt`, the commands and flags, with `UPDATE_GOLDEN=1 cargo test
-p chrome-cdp --bin chrome-cdp surface`). The snapshot corpus
(`testdata/site/snapshot/`: pages from five lines to 50,000 nodes, each with
an `.expected` tree) is checked by `cargo test -p chrome-cdp-engine --test snapshot`,
which also prints both engines' scores and output tokens; its golden outputs
live in `crates/engine/tests/snapshots/`.

chrome-cdp's in-page helper (the locator engine and the DOM walker) is
TypeScript in `crates/engine/src/injected/src/`. `cargo xtask build-js`
typechecks it and bundles it with esbuild into `injected/ccdp.js`, which is
committed, so building chrome-cdp needs no Node; a test fails when the bundle
is older than its sources.

## Docs

The user docs are Markdown in `docs/`, linked from its
[README](README.md). The [command reference](commands/README.md) is
generated from the clap definitions in `crates/chrome-cdp/src/cli.rs`:
after changing a command's help, run `cargo xtask docs`, or the test that
checks it (`help::reference`) fails. `crates/chrome-cdp/tests/guides.rs`
checks every relative link of the README and `docs/` (files and
headings), that code blocks run real commands, and that
[troubleshooting.md](troubleshooting.md) has a section for every error
kind; `tests/parity.rs` does the same for
[playwright-parity.md](playwright-parity.md), and `tests/docs.rs` checks
the E2E docs' examples with `fmt` and `lint`.
