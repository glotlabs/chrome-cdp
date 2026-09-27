# Plan 02 — Browser lifecycle & daemon

Status: TODO
Depends on: 01

## Goal

`chrome-cdp open https://example.com` works on a clean Linux or macOS machine.
It finds or installs Chrome, starts a per-session daemon in the background,
launches Chrome headless over a pipe, and returns in under 1.5 s cold and under
30 ms warm. Every later command talks to that daemon.

## Milestone 2.1 — Chrome discovery

Resolution order (first hit wins; `chrome-cdp status` prints which one was used
and why):

1. `--chrome <path>` flag, then `CHROME_CDP_CHROME` env var.
2. Browsers installed by `chrome-cdp install` in the cache dir (below).
3. System installs:
   - **Linux:** `google-chrome-stable`, `google-chrome`, `chromium`,
     `chromium-browser` on `PATH`; `/opt/google/chrome/chrome`;
     `/snap/bin/chromium` (warn: snap confinement blocks `/tmp` profiles, so
     use a profile under `$HOME`).
   - **macOS:** `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`,
     the same under `~/Applications`, Chrome Canary, Chromium.app.
4. Nothing found: the error hint says to run `chrome-cdp install`. With
   `--auto-install` or `CHROME_CDP_AUTO_INSTALL=1`, it installs automatically.

For the found browser, check the version (`--version` output, or read
`Info.plist` on macOS). Warn if it is below the minimum from overview §3.

## Milestone 2.2 — `chrome-cdp install`

- Source: the Chrome for Testing JSON endpoints
  (`https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json`
  and `known-good-versions-with-downloads.json`).
- `install [--channel stable|beta|dev|canary] [--version X] [--headless-shell] [--force]`.
- Platform keys: `linux64`, `mac-arm64`, `mac-x64`. Chrome for Testing has
  **no linux-arm64 build**. On linux-arm64, fall back to the system `chromium`
  and print distro-specific install hints (apt/dnf/apk/pacman).
- Cache dir: `$XDG_CACHE_HOME/chrome-cdp` or `~/.cache/chrome-cdp` on Linux,
  `~/Library/Caches/chrome-cdp` on macOS. Override with `CHROME_CDP_CACHE`.
  Layout: `browsers/<channel>-<version>-<platform>/`.
- Download with a progress bar on a TTY (silent otherwise), verify the size,
  extract the zip (`zip` crate, preserving unix modes and symlinks; macOS app
  bundles need both), then write the `INSTALLATION_COMPLETE` marker last so a
  partial install is never used.
- **macOS:** remove the `com.apple.quarantine` xattr recursively (`xattr` crate)
  so Gatekeeper does not block the unsigned test build. Test this on both
  architectures.
- **Linux:** after install, run `chrome --version`. If shared libraries are
  missing, parse the `ldd` output and print the exact `apt-get install ...`
  line (keep a known list for Debian/Ubuntu/Fedora). Provide
  `chrome-cdp install --with-deps` that runs that line itself (it needs sudo;
  prompt first).
- Also: `install --list` (installed and available browsers) and
  `uninstall <id>`.
- Use a file lock (`fs4`) so parallel agents don't install twice.

## Milestone 2.3 — Launch

- Flags: `--remote-debugging-pipe`, `--user-data-dir=<profile>`,
  `--no-first-run`, `--no-default-browser-check`, `--disable-background-networking`,
  `--disable-background-timer-throttling`, `--disable-backgrounding-occluded-windows`,
  `--disable-renderer-backgrounding`, `--disable-breakpad`,
  `--disable-client-side-phishing-detection`, `--disable-component-update`,
  `--disable-default-apps`, `--disable-dev-shm-usage` (Linux),
  `--disable-features=Translate,MediaRouter,OptimizationHints,...`,
  `--disable-hang-monitor`, `--disable-ipc-flooding-protection`,
  `--disable-popup-blocking`, `--disable-prompt-on-repost`, `--disable-sync`,
  `--metrics-recording-only`, `--password-store=basic`, `--use-mock-keychain`
  (macOS: avoids keychain prompts), `--export-tagged-pdf`, and `--headless` unless
  `--headed`. Keep the list in one place, document it, and let users add flags
  with `--chrome-arg`.
- `--no-sandbox` only when running as root or inside a detected container
  without user namespaces. Print a warning when it is used.
- **Ubuntu 23.10+ AppArmor.** If `kernel.apparmor_restrict_unprivileged_userns=1`,
  Chrome's sandbox fails ("No usable sandbox!") for any binary without an
  AppArmor profile that grants `userns`. Ubuntu ships a profile only for
  `/opt/google/chrome/chrome`, so browsers from `chrome-cdp install` abort
  on launch. Detect this before launching: check the sysctl and look for a
  profile covering the binary. Then fail with a hint to run
  `chrome-cdp install --with-deps`, which (with sudo, after prompting) writes
  `/etc/apparmor.d/chrome-cdp`:
  `profile chrome-cdp @{HOME}/.cache/chrome-cdp/browsers/*/{chrome-linux64/chrome,chrome-headless-shell-linux64/chrome-headless-shell} flags=(unconfined) { userns, }`
  and reloads it with `apparmor_parser -r`. Do not fall back to
  `--no-sandbox` silently. `chrome-cdp doctor` (plan 10) checks for this too.
  The maintainer's dev machine (Ubuntu 26.04) hit exactly this.
- Linux without a display and `--headed`: fail with a hint (`xvfb-run`).
- Profiles:
  - The default is an ephemeral temp dir, deleted on close.
  - `--profile <name>` is persistent, under
    `<data dir>/profiles/<name>`. It keeps logins between runs.
  - `--user-data-dir <path>` is explicit.
  - Never use the user's real Chrome profile. Chrome 136+ refuses
    remote debugging on the default profile anyway. Document why.
- Put Chrome in its own process group. On daemon exit (clean or crash), kill
  the group. On Linux, also set `PR_SET_PDEATHSIG` via `prctl` so Chrome dies
  with the daemon. On macOS, the daemon runs a watchdog (kqueue `NOTE_EXIT`) and
  the CLI reaps stale PIDs recorded in the session file.
- Capture Chrome's stderr to `<session dir>/chrome.log` (rotated) for
  `chrome-cdp status --logs`.

## Milestone 2.4 — Daemon & sessions

- Session dir: `$XDG_RUNTIME_DIR/chrome-cdp/<session>/` on Linux (fallback
  `/tmp/chrome-cdp-<uid>/`), `$TMPDIR/chrome-cdp/<session>/` on macOS. It holds
  `sock`, `pid`, `meta.json` (chrome path, version, profile, start time),
  `daemon.log`, and `chrome.log`. Dir mode 0700, socket mode 0600.
- **macOS socket path limit is 104 bytes.** `$TMPDIR` is long. If the path is
  too long, hash the session name into a short path under `/tmp/ccdp-<uid>/`.
  Test with a long session name.
- Autostart: if the CLI finds no live socket, it spawns
  `chrome-cdp __daemon --session s ...` detached (`setsid`, stdio → log) and
  waits for the socket with backoff, up to 10 s. Use a lock file so concurrent
  first calls start only one daemon. A dead socket or stale pid is cleaned up.
- The daemon owns: the `cdp::Connection`, the engine state (tabs, ref registry,
  log buffers, routes), and the RPC server. Requests from multiple CLI calls may
  run at the same time. Per-tab operations are serialised by a per-tab async
  mutex, so two agents on one tab can't interleave a click. Reads such as
  `console` do not take the mutex.
- Idle shutdown: after `--idle-timeout` (default 30 min; `0` means never), the
  daemon closes Chrome and exits. If Chrome crashes, the daemon records the
  reason. The next command gets a clear `BrowserCrashed` error with a hint.
  `--restart` relaunches.
- Commands:
  - `open [url] [--headed] [--profile p] [--device d] [--chrome p] [--chrome-arg a]...`
    starts or ensures a session and optionally navigates. If the session exists
    with *different* launch options, it errors with a hint (`close` first, or
    use a new `--session`).
  - `connect <ws://...|http://host:port|port>` attaches the session to an
    existing browser over WebSocket. `close` then only detaches.
  - `close [--all]`, `status` (session, browser, version, tabs, uptime, pid),
    and `sessions` (list all live sessions).
  - `cdp <Domain.method> [json] [--target browser|page]` is the raw escape
    hatch, now daemon-backed.
- The CLI and daemon exchange a version on connect. If they differ (after an
  upgrade), the CLI tells the old daemon to shut down gracefully and starts a
  new one.

## Tests

- Unit tests: discovery resolution order (fake filesystem), CfT JSON parsing,
  platform key mapping, socket path shortening, and flag assembly.
- Integration tests:
  - cold `open` → `status` → `close`, then assert that no Chrome process remains;
  - two named sessions run at once;
  - `kill -9` the daemon, then assert Chrome dies (Linux) or is reaped by the
    next CLI call (macOS);
  - 10 parallel `open` calls on a fresh session → exactly one daemon;
  - idle timeout (set to 2 s in the test);
  - version-mismatch restart.
- A nightly CI job runs `install` for real on each OS/arch and verifies launch.

## Acceptance criteria

- [ ] A fresh macOS arm64 or Ubuntu x86_64 VM with no Chrome gets from
      `chrome-cdp install && chrome-cdp open https://example.com` to success.
- [ ] Cold open < 1.5 s (with Chrome already installed) and warm no-op command
      < 30 ms p50, measured by `xtask bench`.
- [ ] No orphan Chrome processes in any test (checked by a CI step that
      searches for leftover processes).

## Decisions log

_(append here)_
