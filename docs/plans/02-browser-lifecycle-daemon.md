# Plan 02 — Browser lifecycle & daemon

Status: DONE (2.1-2.4; the fresh-VM check and the benchmarks are still to run)
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
- [x] No orphan Chrome processes in any test (checked by a CI step that
      searches for leftover processes).

## Decisions log

_(append here)_
- 2.1: Discovery lives in `browser` (`discover`, `locate`, `installed`, `cache_dir`). It reads the machine through a `Host` value (OS, arch, `HOME`, `PATH`, `XDG_CACHE_HOME`, `CHROME_CDP_CHROME`, `CHROME_CDP_CACHE`, and a root for fixed system paths), so the resolution-order tests run on a temp-dir "filesystem".
- 2.1: `--chrome` no longer uses clap's `env`; discovery reads `CHROME_CDP_CHROME` itself, so it can say which one was used (`given by --chrome` / `given by CHROME_CDP_CHROME`) and word its error per source. A bad explicit path is a `ChromeNotFound` error, never a silent fallback to other browsers.
- 2.1: `--chrome` also accepts a bare name looked up on `PATH` (`--chrome chromium`) and a macOS `.app` bundle (the binary inside it is used).
- 2.1: Installed browsers must have `INSTALLATION_COMPLETE` and match this host's platform key. Several installs rank full Chrome before headless shell, then stable > beta > dev > canary, then the newest version. Their version comes from the directory name, so no process is run.
- 2.1: Snap is detected by a path under `/snap/`, a symlink to `snap`, or a small wrapper script that execs `/snap/bin/…` (Ubuntu's `chromium-browser`). A snap only gets a warning for now; milestone 2.3 must put its profile outside `/tmp` and hidden dirs (snap confinement blocks `~/.cache` too).
- 2.1: The minimum is `MIN_CHROME_MAJOR = 150` (the vendored protocol is Chrome 154's, minus 4). A unit test pins it to `PROTOCOL_REVISION`, so moving the protocol pin forces a look at it. Too old, and "cannot tell the version", are warnings, not errors. When the command then fails, they are attached to the error as `warning:` notes, because error envelopes carry no warnings.
- 2.1: The version comes from `Info.plist` for macOS app bundles (a string search, no plist crate), else from `<chrome> --version` with a 5 s cap.
- 2.1: `status` does not exist yet, so the choice and its reason (`Source::describe`) are logged at `-v` by `__direct cdp`. `Chrome` and `Source` are `Serialize` for milestone 2.4's `meta.json` and `status`.
- 2.1: `--auto-install` / `CHROME_CDP_AUTO_INSTALL` and the "run `chrome-cdp install`" hint wait for milestone 2.2: a hint naming a command that does not exist would mislead. The not-found hint says to install Chrome or Chromium, pass `--chrome`, or set `CHROME_CDP_CHROME`, and lists every place searched (`searched:` note, `details.searched`).
- 2.1: The golden case `direct-no-chrome-given` depended on what the machine has installed, so it became `direct-chrome-env-missing`. The not-found output is checked by a unit test and by a CLI test that skips on machines with a system Chrome. The test fake Chrome now answers `--version`.
- 2.2: Chrome for Testing has published linux-arm64 builds since Chrome 153, so the plan's "no linux-arm64 build" is out of date. Linux arm64 maps to `linux-arm64`. The distro Chromium hint (apt/dnf/apk/pacman/zypper, and snap on Ubuntu) now covers only machines with no build at all and `--version`s with no build for this platform.
- 2.2: Full Chrome and headless shell of one version share `browsers/<channel>-<version>-<platform>/`, as the zips unpack and as 2.3's AppArmor glob expects. Each product is unpacked in `browsers/.staging-*` and moved in with one `rename`, then the marker is written. A crash leaves only a staging dir, which the next install removes.
- 2.2: `--version` takes a full version or a prefix of whole parts (`154` picks the newest 154.x with a build for this platform) from `known-good-versions-with-downloads.json`. The install is labelled with the channel that version is current on, else `pinned`, which discovery ranks after canary. `--channel` and `--version` conflict.
- 2.2: The lock is std's `File::lock` on `<cache>/install.lock` (stable since Rust 1.89; MSRV is 1.98), not `fs4`. `uninstall` takes it too. A waiting install prints a line on a terminal.
- 2.2: The download is checked against `Content-Length`, and the zip crate checks each entry's CRC-32 as it reads. The index has no hashes. GCS's `x-goog-hash` MD5 would need another crate, so it is not checked.
- 2.2: HTTP is reqwest 0.13 with `rustls-no-provider` plus rustls's ring provider (aws-lc-rs needs cmake), and rustls-platform-verifier, so system and corporate CAs work. cargo-deny passes. The release binary grew from 2.0 MB to 4.5 MB.
- 2.2: `--timeout` bounds connecting and each wait for data, not the whole download. A stall is a `Timeout` error (exit 3).
- 2.2: New error kind `InstallFailed` (exit 4) for failed downloads and unpacking. An unknown `--version` or uninstall id is `BadArgs`. On Linux, an install that cannot start (`--version` fails) is `LaunchFailed`, with the missing libraries from `ldd` and the `apt-get`/`dnf` line. The install stays, so rerunning with `--with-deps` skips the download.
- 2.2: `--with-deps` asks on a terminal. Without one, it runs only as root or with passwordless sudo (CI); otherwise it fails with the line to run. The package manager's output goes to stderr. The library table covers Debian/Ubuntu (with `t64` names from Ubuntu 24.04 and Debian 13) and Fedora/RHEL. Writing the AppArmor profile is milestone 2.3's part of `--with-deps`.
- 2.2: Offline, `install` keeps an installed build of the channel and warns; `--list` shows the installs and warns.
- 2.2: `CHROME_CDP_CFT_URL` replaces the index URL, for mirrors. Tests point it at a fake server in `browser::testing` (feature `test-support`), which serves script "browsers" that answer `--version`.
- 2.2: `--auto-install` / `CHROME_CDP_AUTO_INSTALL` is on `__direct cdp` for now, through `install::find_chrome`, which 2.4's `open` should call. The not-found hint is now `chrome-cdp install`, or the distro package where there is no build.
- 2.2: Installing a newer version leaves older installs of the channel in place, with a warning that names the `uninstall` command. There is no automatic cleanup.
- 2.2: The quarantine walk uses the `xattr` crate. It builds on every unix and is tested on Linux with a `user.*` attribute. It has not run on a Mac yet; the nightly job covers that.
- 2.2: `install` prints one line ending in the binary's path, and `-q` prints just the path. `--list` prints one line per install (a table would cut the path) and a table of channels. The output checklist counts `install` and `uninstall` as fixed-size.
- 2.2: `.github/workflows/nightly.yml` installs both products on all four runners and runs them. Its launch step is `continue-on-error` until 2.3 deals with Ubuntu's AppArmor sandbox rule. 2.3's profile glob must also cover `chrome-linux-arm64/chrome` and `chrome-headless-shell-linux-arm64/chrome-headless-shell`.
- 2.2: No acceptance box is ticked. The fresh-VM check needs 2.4's `open`, the timings are deferred (benchmarks run after all plans), and orphan checks come with launch.
- 2.3: Launch lives in `browser::launch`: `flags` (the one flag list), `profile`, `sandbox` (with an `/etc/apparmor.d` reader in `apparmor`), `process`, `stderr`, and `sys` for the libc calls. `launch()` returns `ErrorInfo` directly: its errors are for users, and 2.4's daemon passes them through as is. `__direct cdp` uses it with the shared `LaunchArgs` (`--headed`, `--profile`, `--user-data-dir`, `--chrome-arg`) that 2.4's `open` should flatten too.
- 2.3: Flags: the plan's list, plus `--disable-field-trial-config` (no field trials, so runs behave the same) and `--disable-search-engine-choice-screen` (a headed dialog). `--disable-features` covers Translate, MediaRouter, DialMediaRouteProvider, OptimizationHints, AcceptCHFrame, AutofillServerCommunication, GlobalMediaControls, LensOverlay, HttpsUpgrades (it breaks local `http://` servers) and PaintHolding. `--disable-dev-shm-usage` and `--password-store=basic` are Linux only, `--use-mock-keychain` macOS only. `docs/chrome-flags.md` explains each one, and a test fails if one is missing there. A user `--disable-features`/`--enable-features` is merged with ours (Chrome honours only the last one). `--chrome-arg` values must be `--flags`; `--user-data-dir` and the debugging pipe are refused.
- 2.3: `--no-sandbox` is added as root, or in a container (`$container`, `/.dockerenv`, `/run/.containerenv`) where user namespaces fail: the `max_user_namespaces`/`unprivileged_userns_clone` sysctls, else `unshare --user true`, since a seccomp filter can block them. If that cannot be told (no `unshare`), the sandbox stays on. Both cases, and a user's `--chrome-arg=--no-sandbox`, print a warning.
- 2.3: The AppArmor check runs before launch only for browsers in chrome-cdp's cache, which is the case `install --with-deps` can fix. Any other binary is launched, and Chrome's own "No usable sandbox" is explained afterwards, so a misread custom profile can never block a working system Chrome. The reader follows profile headers (on several lines too), `userns` rules, `@{VAR}` definitions (`@{HOME}` gets its default value plus this user's home) and AppArmor globs. A setuid-root `chrome-sandbox` also counts. `CHROME_CDP_NO_USERNS_CHECK=1` turns the check off, as an escape hatch, and the CLI tests set it so their output does not depend on the machine.
- 2.3: `install --with-deps` writes the profile with `sudo install -m 644` and loads it with `apparmor_parser -r`, after one prompt; `run_fix` became `run_privileged`. The profile uses `abi <abi/4.0>` (Ubuntu 24.04, the first ABI with `userns`) and covers `chrome-linux64`, `chrome-linux-arm64` and both headless-shell dirs. The default cache uses `@{HOME}/.cache/chrome-cdp`, so it covers every user. Another cache (`CHROME_CDP_CACHE`, `XDG_CACHE_HOME`) gets `chrome-cdp-<fnv32 of the path>` with its absolute path, so it never overwrites the default. Without `--with-deps`, `install` warns. The sudo path could not run here (no sudo); the profile text is checked by parsing it back.
- 2.3: The nightly job installs with `--with-deps` and launches both products, without `continue-on-error`.
- 2.3: Processes: Chrome leads its own process group, and close kills the group after the browser exits. On Linux, `PR_SET_PDEATHSIG` is set in `pre_exec`. That needs `unsafe`, so `launch::sys` is a second `#[allow(unsafe_code)]` module next to `cdp`'s fd module. The death signal follows the *thread* that spawned Chrome, so 2.4's daemon must launch from a runtime worker, never from `spawn_blocking`.
- 2.3: The kqueue watchdog became a watchdog on every OS: `/bin/sh` in its own process group reads a pipe from chrome-cdp. If chrome-cdp dies, the pipe closes, and the watchdog kills Chrome's group and deletes the run dir. It needs no `unsafe` and is tested on Linux. Reaping stale pids from the session file is 2.4's.
- 2.3: Crashpad handlers `setsid` out of Chrome's group, but they exit with the browser (checked by hand and by the tests' process checks).
- 2.3: Each launch gets a private run dir (`chrome-cdp-*` in the temp dir, or under `~/snap/<snap>/common/chrome-cdp` for a snap). It holds the temporary profile and serves as Chrome's `TMPDIR`: Chrome keeps its `org.chromium.Chromium.*` socket dir there, which leaked into `/tmp` whenever Chrome was killed. Close, `Drop` and the watchdog delete it.
- 2.3: Profiles: the data dir is `$XDG_DATA_HOME/chrome-cdp` or `~/.local/share/chrome-cdp` (Linux), `~/Library/Application Support/chrome-cdp` (macOS). Names are `[A-Za-z0-9][A-Za-z0-9._-]{0,63}`, and dirs are created 0700. `--user-data-dir` inside an everyday Chrome/Chromium user data dir is `BadArgs`. A kept profile whose `SingletonLock` names a live pid on this host is `LaunchFailed`, since Chrome would hand the launch to that browser and exit.
- 2.3: Snaps: named profiles and the run dir go under `~/snap/<snap>/common/chrome-cdp` (the snap name comes from `/snap/bin/<name>`, else `chromium`). A `--user-data-dir` a snap cannot write (under `/tmp`, or a hidden dir in `$HOME`) gets a warning. Discovery's snap warning was dropped. This is not yet tested with a real snap.
- 2.3: `--headed` on Linux needs `DISPLAY` or `WAYLAND_DISPLAY`, else `LaunchFailed` with an `xvfb-run -a` hint. With chrome-headless-shell it is `BadArgs`.
- 2.3: Chrome's stderr goes to `LaunchOptions::log_file`, rotated at 8 MB (`LOG_LIMIT`) and at each launch, keeping one `.1`. It is written 0600. 2.4 points it at `<session dir>/chrome.log`; `__direct cdp` has no session and passes none.
- 2.3: `__direct cdp` handles SIGINT, SIGTERM and SIGHUP. Dropping the `Launched` kills the group and deletes the run dir. It then prints `error: interrupted by SIGTERM; Chrome was closed` and exits 128+n, the shell convention; no §3 kind fits a signal.
- 2.3: `install --json` gains an optional `apparmor_profile` field (the profile `--with-deps` wrote).
- 2.3: CI fails if a process with `--remote-debugging-pipe` or `chrome-cdp-watchdog` in its command line survives the tests. The acceptance box is ticked because the launch tests assert it, and a full local run left no processes and no temp dirs. The step has not run on GitHub yet. The cdp crate's own Chrome tests now keep Chrome's `TMPDIR` in their profile.
- 2.3: The fresh-VM box waits for 2.4's `open`, and the timing box is deferred: benchmarks run after all plans.
- 2.4: Crates: `protocol::rpc` holds the JSON-RPC 2.0 envelope (a result is a `protocol::Output`, a failure carries the `ErrorInfo` in `error.data`, so the CLI renders both locally). `daemon` has `paths`, `meta`, `server`, `client`, `rpc` (method params) and `output` (text forms), with all Unix-socket code in `transport/unix.rs`. `engine::Engine` owns the browser; CDP error mapping moved there from the binary. `__direct` and `direct.rs` are gone.
- 2.4: Params stay a JSON `Value` in the envelope, so an older daemon answers an unknown method with `-32601` instead of failing to parse. `hello`, `status` and `close` must stay compatible across builds: a new CLI uses them to inspect and replace an old daemon.
- 2.4: A daemon holds `daemon.lock` (flock) while it runs; clients test liveness with a shared `try_lock`, never with the pid. Starting one happens under `start.lock`; stale `sock`/`pid`/`meta.json` are removed under that lock too, so a starting daemon's files are never touched. A dead daemon's Chrome is killed only if `ps` shows `--remote-debugging-pipe` and its recorded `--user-data-dir` (pids get reused).
- 2.4: `setsid` is still unstable in std (`process_setsid`), and a `pre_exec` would need another `unsafe` module. The daemon gets its own process group (`process_group(0)`), ignores SIGHUP, runs from `/`, and writes stdout and stderr to `daemon.log` (rotated past 1 MB, one `.1` kept).
- 2.4: The daemon runs a current-thread runtime on its main thread. `PR_SET_PDEATHSIG` follows the thread that spawned Chrome, and this is the one thread sure to live as long as the daemon. If plan 04's snapshot work needs more CPU, move that work to other threads; never launch Chrome from them.
- 2.4: Session dirs: Linux `$XDG_RUNTIME_DIR/chrome-cdp/<name>`, else `/tmp/chrome-cdp-<uid>/<name>`; macOS `$TMPDIR/chrome-cdp/<name>`, else `/tmp/chrome-cdp-<uid>`. When `<dir>/sock` does not fit `sun_path` (104 bytes on macOS, 108 on Linux), the session lives in `/tmp/ccdp-<uid>/<FNV-1a 64 of the long dir's path>`. Hashing the path rather than the name keeps two base dirs apart; `meta.json` records the long path so `sessions` can list only its own. Base and session dirs are created 0700 and must be real dirs owned by us and closed to others; the socket is 0600. Session names follow the profile-name grammar (at most 64 chars).
- 2.4: The build id is the version, the git commit and the binary's mtime, so a rebuilt binary also replaces its daemon. After a replacement, options the command does not give come from the old daemon's `meta.json` (its launch options, or its `connect` endpoint).
- 2.4: `open` options left out mean "whatever runs", so `open <url>` works on any session. Given ones (`--chrome`, `--headed`, `--profile`/`--user-data-dir`, `--chrome-arg`) must match the running browser, else `BadArgs` with a hint for `--restart` or another `--session`. `open` on a crashed browser starts a new one, with the crash as a warning; `--restart` also replaces a live one.
- 2.4: `--device` waits for plan 07: a flag that is accepted but ignored would mislead.
- 2.4: `open <url>` does the least navigation this milestone needs (`Page.navigate`, then the load event, bounded by `--timeout`; `NavigationFailed` on `errorText`). A URL without a scheme gets `https://`, or `http://` for localhost. Plan 03's `goto` replaces it.
- 2.4: `cdp --target page` now means the current page: the last one used if it is still open, else the first page, else a new `about:blank`. Plan 03's tab manager replaces this rule. `cdp --target page` and `open <url>` hold the page's async mutex; browser calls and `status` take none.
- 2.4: `cdp` gives a session without a browser one, as `open` would (with a warning naming it). `status`, `close` and `sessions` never start a daemon; `status` of a session with no daemon exits 0 with state `stopped`, and `close` of one says it was not running.
- 2.4: Chrome is found (and with `--auto-install` installed) on the CLI side, so the progress bar shows; the daemon then launches that `Chrome`, sent over RPC. Argument checks, `--chrome-arg`, `--profile`, the URL and the `connect` endpoint are checked before any daemon starts. When a command fails on a daemon it started, it closes that daemon again.
- 2.4: Idle: 30 min by default; `--idle-timeout` (and `CHROME_CDP_IDLE_TIMEOUT`) on `open` and `connect`, `0` for never. Only `open`, `connect` and `cdp` count as activity, so `sessions` or `status` polling keeps nothing alive. A daemon with no browser exits after 10 s, but never while a client is connected: the CLI keeps its connection while it downloads Chrome.
- 2.4: A crash is noticed through `Connection::closed`. The state becomes `crashed` with the reason (the last line of Chrome's stderr), and commands fail with `BrowserCrashed` and the hint `chrome-cdp open   (starts a new browser), or chrome-cdp status --logs shows why`. A startup failure's hint is now `chrome-cdp status --logs` (with `--session`), since Chrome's stderr goes to the session's `chrome.log`.
- 2.4: `connect` takes a port, `host:port`, `http://host:port` (a plain HTTP/1.1 GET of `/json/version`) or a `ws://` URL. `wss://` and `https://` are `BadArgs` until the WebSocket transport has TLS. Connection failures are `LaunchFailed`, the nearest §3 kind.
- 2.4: `status` shows the state (`running`, `crashed`, `idle`, `stopped`), browser, Chrome path and source, profile, tabs, uptime, daemon pid, idle timeout and dir; `-q` prints the state. `status --logs [--limit N]` adds the end of `chrome.log`, for stopped sessions too. `close` returns once Chrome and the daemon have exited; `close --all` closes every live session. The daemon removes `sock`, `pid` and `meta.json` on exit and keeps the logs.
- 2.4: `uninstall` refuses a browser that a live session launched.
- 2.4: Tests run each session in a `Sandbox` (its own `XDG_RUNTIME_DIR` and `TMPDIR`, idle timeout 2 min, `close --all` on drop). CI's leftover-process step also looks for `chrome-cdp __daemon`. The golden case `missing-subcommand` is gone until a command has subcommands again (plan 03's `tab`). Golden files mask pids, uptimes, the run dir's random suffix and the Chrome version.
- 2.4: The fresh-VM box stays open: this machine is not a fresh VM. The timing box stays open, deferred: benchmarks run after all plans. The check is written as the ignored test `crates/chrome-cdp/tests/bench.rs`, for plan 10's `xtask bench` to adopt.
