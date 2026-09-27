# How chrome-cdp launches Chrome

chrome-cdp starts Chrome with a fixed set of flags. They live in one
place, `crates/browser/src/launch/flags.rs`, and a test checks that this page
lists every flag in that file. Add your own flags with `--chrome-arg`, e.g.
`--chrome-arg=--window-size=1280,800`. A `--disable-features` or `--enable-features` you
pass is merged with chrome-cdp's list, because Chrome only honours the last
one of each. Features you enable are removed from the disabled list.

`--chrome-arg` cannot set `--user-data-dir` (use `--user-data-dir` or
`--profile`) or the remote debugging pipe, which chrome-cdp owns.

## Flags

| Flag | Where | Why |
|---|---|---|
| `--remote-debugging-pipe` | all | Talk CDP over fds 3 and 4. No TCP port is opened, so nothing else on the machine can drive the browser. |
| `--headless` | unless `--headed` | Chrome's full headless mode (the same browser, without a window). |
| `--no-first-run` | all | Skip the first-run dialogs. |
| `--no-default-browser-check` | all | Do not ask to become the default browser. |
| `--disable-background-networking` | all | No background requests (safe browsing lists, suggestions and so on) that add noise to `network`. |
| `--disable-background-timer-throttling` | all | Timers in background tabs run at full speed. |
| `--disable-backgrounding-occluded-windows` | all | Hidden and covered windows are not treated as background. |
| `--disable-renderer-backgrounding` | all | Background tabs keep their normal priority, so a tab the agent is not looking at still makes progress. |
| `--disable-breakpad` | all | No crash reporting. |
| `--disable-client-side-phishing-detection` | all | No phishing classifier work on every page. |
| `--disable-component-update` | all | Do not download component updates. |
| `--disable-default-apps` | all | Do not install the default apps. |
| `--disable-dev-shm-usage` | Linux | Use `/tmp` instead of `/dev/shm`, which is often only 64 MB in containers and makes Chrome crash. |
| `--disable-field-trial-config` | all | No field trials, so behaviour does not change from run to run. |
| `--disable-hang-monitor` | all | No "page unresponsive" dialogs. |
| `--disable-ipc-flooding-protection` | all | Fast bursts of input and navigation are not throttled. |
| `--disable-popup-blocking` | all | Pages can open popups, which become tabs the agent can see. |
| `--disable-prompt-on-repost` | all | Reloading after a POST does not ask. |
| `--disable-search-engine-choice-screen` | all | No search engine choice dialog in headed mode. |
| `--disable-sync` | all | No Google account sync. |
| `--metrics-recording-only` | all | Metrics are recorded locally but never uploaded. |
| `--password-store=basic` | Linux | Do not use the desktop keyring (it can prompt, or hang without a desktop). |
| `--use-mock-keychain` | macOS | Do not use the macOS keychain, which would prompt. |
| `--export-tagged-pdf` | all | `pdf` output is tagged, so it is accessible. |
| `--no-sandbox` | see below | Only as root, or in a container without user namespaces. |
| `--user-data-dir=<dir>` | all | The profile (see below). |

`--disable-features` turns off:

- `Translate`: the translate bar.
- `MediaRouter`, `DialMediaRouteProvider`: casting, which scans the network.
- `OptimizationHints`: optimization guide downloads.
- `AcceptCHFrame`: an extra client hints round trip, which changes request counts.
- `AutofillServerCommunication`: autofill requests to Google.
- `GlobalMediaControls`: the media controls popup.
- `LensOverlay`: Google Lens.
- `HttpsUpgrades`: automatic `http://` → `https://` upgrades, which break local test servers.
- `PaintHolding`: holding the old page's paint during same-origin navigations, so screenshots show the new page.

The first page is `about:blank`.

## The sandbox

Chrome's sandbox stays on. chrome-cdp adds `--no-sandbox` only when:

- it runs as root, where Chrome refuses to start with the sandbox; or
- it runs in a container (`/.dockerenv`, `/run/.containerenv` or
  `$container`) that cannot create user namespaces.

It prints a warning when it does. You can also pass
`--chrome-arg=--no-sandbox` yourself, which also warns.

On Ubuntu 23.10 and later, `kernel.apparmor_restrict_unprivileged_userns=1`
blocks the sandbox for any binary that has no AppArmor profile granting
`userns`. Ubuntu ships one for `/opt/google/chrome/chrome` only, so browsers
from `chrome-cdp install` fail with "No usable sandbox!". chrome-cdp checks for
this before it launches one and says to run `chrome-cdp install --with-deps`.
That command writes `/etc/apparmor.d/chrome-cdp` (with sudo, after asking)
and loads it with `apparmor_parser -r`. It covers the browsers in
`~/.cache/chrome-cdp` of every user, for x86_64 and arm64. A cache elsewhere
(`CHROME_CDP_CACHE`, `XDG_CACHE_HOME`) gets its own `chrome-cdp-<hash>`
profile for its path. chrome-cdp never falls back to `--no-sandbox` by
itself.

If the check misjudges your machine (it reads the profiles in
`/etc/apparmor.d`), `CHROME_CDP_NO_USERNS_CHECK=1` turns it off. Chrome
then reports the problem itself, with the same hint.

## Profiles

- By default each launch gets a new, empty profile in the temp dir, deleted
  when Chrome closes.
- `--profile <name>` keeps a profile (logins, cookies, storage) between runs
  in `<data dir>/profiles/<name>`. The data dir is `$XDG_DATA_HOME/chrome-cdp`
  or `~/.local/share/chrome-cdp` on Linux, and
  `~/Library/Application Support/chrome-cdp` on macOS.
- `--user-data-dir <path>` uses that directory.

A profile that another Chrome has open is an error: Chrome would hand the
launch to that browser and exit.

chrome-cdp refuses your everyday Chrome profile (`~/.config/google-chrome`,
`~/Library/Application Support/Google/Chrome` and the like). Driving it would
expose your logins to every command, a crash could damage it, and Chrome 136
and later refuse remote debugging on the default profile anyway. Log in once
in a `--profile` instead.

A snap Chromium cannot see `/tmp` or hidden directories in your home, so its
profiles live under `~/snap/<snap>/common/chrome-cdp/` instead.

## Processes

Chrome runs in its own process group. When chrome-cdp closes it, it kills the
whole group, so no helper process stays behind. If chrome-cdp itself dies
(even from `kill -9`):

- on Linux, `PR_SET_PDEATHSIG` kills Chrome at once;
- everywhere, a small watchdog (`sh`) holds a pipe to chrome-cdp. When the
  pipe closes, it kills Chrome's process group and deletes the launch's
  temporary directory.

Each launch has its own temporary directory (`chrome-cdp-*` in the temp
dir). It holds the temporary profile and serves as Chrome's `TMPDIR`, so the
socket directory Chrome keeps there (`org.chromium.Chromium.*`) goes away
with it even when Chrome is killed.

Chrome's stderr is kept in the session's `chrome.log` (rotated at 8 MB, one
old file kept as `chrome.log.1`). `-vv` also prints it.
