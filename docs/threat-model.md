# Threat model

This is the maintainers' view of chrome-cdp's security: what it protects,
from whom, where each defence lives in the code, and what it leaves to the
user on purpose. [security.md](security.md) is the short version for users;
this page backs each of its claims. It was written for the v1 security
review (plan 10, milestone 10.7) and must change with any feature that
opens a port, writes a file with secrets, or acts on what a page sends.

## What is worth protecting

- **The browser.** Whoever controls it acts as the user on every site the
  profile is logged in to: cookies, storage, saved passwords, open pages.
- **Secrets at rest.** Named profiles, `state save` files, HAR files,
  traces, saved response bodies, `meta.json` (the session's launch
  settings, `--http-credentials` included), `${secret:…}` values of a
  test run.
- **The machine.** Chrome's sandbox between web content and the user's
  files; the binaries chrome-cdp installs and runs (Chrome, itself).
- **The agent's context.** What chrome-cdp prints is read by an agent
  that acts on it.

## Who is trusted

| Actor | Trust | Why |
|---|---|---|
| The user who runs chrome-cdp, and their agent | full | They can already run any program as themselves. |
| Other processes of the same uid | full | They can `ptrace` the daemon or read its memory; no boundary is possible. |
| Project files: `chrome-cdp.toml`, `.cdp` scripts, mock sets | code | Like a `package.json`: running `chrome-cdp test` runs its web servers and backends. |
| Other local users | none | They must not reach the browser, its sockets or its files. |
| Web pages, frames, their servers | none | Untrusted input, including text meant to steer an agent. |
| The network | none | Downloads and `connect` cross it. |
| GitHub releases, Chrome for Testing's servers | integrity by HTTPS | See [what is left](#what-is-left). |

## Boundaries and their defences

### Other local users cannot reach the browser

| Defence | Where | Checked by |
|---|---|---|
| Launched Chrome talks over `--remote-debugging-pipe`; no TCP port | `crates/browser/src/launch/flags.rs` | `flags.rs` tests |
| `--chrome-arg` cannot replace the pipe; a `--remote-debugging-port`/`-address` warns | `flags.rs` (`check_extra`, `debug_port_warning`) | `flags.rs` tests |
| Session dirs are `0700`, owned by the user, not symlinks; created and checked before a daemon starts, and before a client connects or lists sessions | `crates/daemon/src/paths.rs` (`create_private`, `check_private`), `client.rs` (`connect`, `live_sessions`) | `paths.rs` tests, `tests/lifecycle.rs` |
| The socket is `0600` inside that dir (the dir closes the moment between `bind` and `chmod`) | `crates/daemon/src/transport/unix.rs` | its test |
| `meta.json`, `daemon.log`, `chrome.log`, the lock files are `0600` | `meta.rs`, `client.rs`, `launch/stderr.rs` | |
| Chrome dies with its daemon (a watchdog kills its process group) | `crates/browser/src/launch/process.rs` | `xtask leaks`, the suite's `Sandbox` leak check |
| `connect` warns when the endpoint, or the WebSocket URL `/json/version` names, is not loopback | `crates/engine/src/endpoint.rs` (`remote_warning`) | its tests |

The socket speaks line-delimited JSON-RPC with no authentication beyond
the file modes, no size limit on a line and no idle timeout: every client
is the user. `protocol::rpc::decode_request` is fuzzed
(`fuzz/fuzz_targets/rpc_decode.rs`, nightly) so no line crashes the daemon
or, as a response, the CLI.

### Secrets at rest are the user's only

| File | Mode | Where |
|---|---|---|
| Temporary profile | `0700` temp dir, deleted on close (the watchdog too) | `crates/browser/src/launch/profile.rs` |
| Named profile | `0700`; an existing one is closed again, another user's refused | `profile.rs` (`keep_private`) |
| `state save` | `0600`, an existing file too, before a byte is written | `crates/engine/src/state/commands.rs` |
| HAR, trace, `network show -o` bodies | `0600`, an existing `-o` file too | `crates/engine/src/observe/save.rs` (`save_private`) |
| Screenshots, PDFs, `.results/` of `test` | the umask's | may be read by CI steps of another user |
| Downloads | the session dir (`0700`); the page's file name loses its directories and leading dots | `crates/engine/src/tabs/downloads.rs` |

Secrets outside files: passwords show as `(password)` in snapshots;
`${secret:…}` values are masked in everything the CLI prints or the runner
writes; `--http-credentials` shows as `***` in the session's `history`;
`HttpCredentials`' `Debug` hides the password. `meta.json` keeps the
credentials so a daemon restarted by an upgrade keeps them (it is `0600`).

### Pages cannot act beyond their tab

What a page sends that chrome-cdp acts on:

- **Downloads**: saved in the session dir under a cleaned name, never a
  path the page chose.
- **Bindings** (`expose`, the mock shim for WebSockets and SSE): the page
  can log to the session's console and ask for a configured mock's
  answer. Neither writes files or runs commands.
- **Dialogs**: answered by the session's policy.
- **HTTP authentication challenges**: answered with `--http-credentials`,
  from every origin unless `--http-credentials-origin` names one; a proxy
  never gets them unless it is the origin named
  (`crates/engine/src/tabs/auth.rs`).
- **Text**: console messages, titles, dialog messages and anything
  `snapshot`, `text` or `eval` return. It reaches the terminal with
  control characters shown as symbols (`protocol::format::printable`), so
  a page cannot send escape sequences to the user's terminal. It reaches
  the agent as it is: prompt injection is not something chrome-cdp can
  filter (see [When an agent drives it](security.md#when-an-agent-drives-it)).

chrome-cdp's own page scripts (the locator engine, actionability,
snapshots, input) run in an isolated world (`__chrome_cdp`), where page
code cannot patch the DOM APIs they call or see their results. Scripts
that must run in the page's world do so by design: user init scripts,
`eval --world main`, `wait --fn`, the fake clock, `expose` and the mock
shim.

Chrome's sandbox stays on; chrome-cdp turns it off only where Chrome
cannot start with it (root, a container without user namespaces) and warns
each time.

### Mock and fixture servers stay local

`mock serve` and the E2E runner's mock and static servers bind to
`127.0.0.1` by default; `--host` must be a literal IP. They read files
only under the mock set's dir (a path rendered from a request that adds
`..` or a root is refused, `crates/mock/src/respond.rs`) and never write
files, run commands or make outbound requests. `mock record` and
`mock import` name data files from URLs and methods cleaned to
`a-z0-9._-`; a HAR entry whose method is not an HTTP token is refused,
and the writer refuses any data file path that is not plain names under
the base (`crates/mock/src/record.rs`, `crates/chrome-cdp/src/mock.rs`).

### Downloads are what they claim to be

| Defence | Where |
|---|---|
| HTTPS for every download, the URLs an index lists and every redirect; plain HTTP only to a loopback host (tests, a local mirror) | `crates/browser/src/download.rs` |
| `upgrade` checks the archive's SHA-256 against the release manifest, stages the new binary next to the old one, runs it, then renames it over; a manifest whose version or archive is not a plain name is refused | `crates/chrome-cdp/src/upgrade.rs` |
| `install.sh` checks the archive's `.sha256`; curl runs with `--proto =https --proto-redir =https --tlsv1.2` | `install.sh` |
| The Chrome zip may not write outside its dir: no unsafe path, no symlink pointing out, nothing written through a symlink an earlier entry made; setuid bits dropped | `crates/browser/src/unzip.rs` |
| `install --with-deps` shows every `sudo` command before it runs it (argv, no shell) and writes an AppArmor profile only for a cache path without control characters | `crates/chrome-cdp/src/install.rs`, `crates/browser/src/launch/sandbox.rs` |

### The dependencies

CI runs `cargo deny` (licenses, bans, sources and the RustSec advisories;
OpenSSL is banned, TLS is `rustls`) and `cargo audit` on every push, and
both advisory checks nightly. The `.cdp` parser and the RPC decoder are
fuzzed nightly (`cargo xtask fuzz`); a crash opens an issue with its input.

## Powerful on purpose

`eval`, `cdp`, `init-script`, `route`, `mock`, `state load`, `cookies`
and `storage` can do anything the browser can: read every cookie, change
settings, rewrite responses. They are not a boundary, since only the user
can call them; an agent should not pass page text to them
([security.md](security.md#powerful-on-purpose)).

## What is left

These are known and accepted for v1, or tracked:

- **Authenticity of releases.** The SHA-256 that `upgrade` and
  `install.sh` check comes from the same host as the archive, so it
  protects against corruption, not against a compromised release. Signed
  releases (minisign or Sigstore, with the key in the binary) are future
  work. Chrome for Testing publishes no checksums; its zips are trusted by
  HTTPS alone.
- **Project config runs commands.** `chrome-cdp test` in an untrusted
  checkout runs the `[e2e.web_server]` and backend commands its
  `chrome-cdp.toml` names, as `npm test` runs `package.json` scripts.
  Each is printed on stderr (`running: <command> (from <config>)`) before
  it starts, so none runs unseen, but nothing asks first.
- **`mock serve` is reachable by any local web page.** With `cors =
  "auto"` it answers any origin, and it does not check `Host`, so a page
  in another browser can read or change its fake data.
- **A page can fill the session dir with downloads.** There is no quota.
- **Some reports run in the page's world.** The title and status `goto`
  reports and the `emulate` locale check can be faked by the page itself
  (it only lies about itself).
- **`XDG_RUNTIME_DIR`'s absence.** The fallback dirs under `/tmp` are
  checked, so another user can at worst stop a session from starting
  (by making the dir first), never take it over.
