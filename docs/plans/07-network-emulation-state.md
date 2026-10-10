# Plan 07 — Network control, emulation & state

Status: DONE (7.1-7.5, checked at close-out; the route overhead benchmark is built and runs with the benchmarks after all plans)
Depends on: 03 (parallel with 04–06)

## Goal

Let agents shape the environment the page runs in: mock APIs, simulate
devices and bad networks, and save and restore logged-in state. All settings
are **session-scoped and sticky**. They apply to existing tabs and are
re-applied to every new tab, popup and OOPIF *before* its scripts run (via
the auto-attach pause from plan 03).

## Milestone 7.1 — Request routing (mocking)

This is the imperative, per-rule routing engine. Plan 11 builds file-based mock
sets, stateful mocks, and request journals on top of it, so design the engine
so that rules can be added in bulk per browser context, and so that it records
every routed request and the rule that handled it.

- Use the `Fetch` domain with request-stage patterns (and response-stage
  patterns when a rule needs the real response).
- `route <url-glob|/regex/> [--method M] [--resource-type T]` with one action:
  - `--status 200 --body '<text>' | --body-file f | --json '<json>'
    [--header K:V]...` (fulfill);
  - `--abort [failed|blockedbyclient|timedout|…]`;
  - `--delay 2000` (then continue);
  - `--set-header K:V`, `--remove-header K`, `--rewrite-url <url>` (continue
    modified);
  - `--patch-json '<jq-like merge>'`: fetch the real response and deep-merge
    the patch. Useful for "what if the API returns an empty list".
  - `--times N`: the rule expires after N matches.
- `routes` lists rules with hit counts. `unroute <id|--all>` removes them.
- `block <glob>...`: sugar for `--abort`, with presets `--block ads`,
  `--block images`, `--block fonts`, `--block media` for speed.
- Rules are evaluated in the daemon. The first matching rule wins, in order of
  addition. Unmatched requests continue immediately; measure the overhead and
  keep it under 1 ms per request. Enable `Fetch` only while at least one rule
  exists, so it has zero cost otherwise.
- Service workers can bypass `Fetch`. Provide `--bypass-service-worker`
  (`Network.setBypassServiceWorker`), and turn it on automatically while
  routes exist. Document this.

## Milestone 7.2 — Emulation

- `emulate <device>`: a built-in device table (about 40 common phones and
  tablets, generated from Apache-2.0 device descriptors with
  attribution). Each entry sets viewport, DPR, `isMobile`, `hasTouch` and UA.
  `emulate --list`. `emulate reset`.
- `viewport <W>x<H> [--dpr N] [--mobile]`: `Emulation.setDeviceMetricsOverride`.
  In headless mode, the default viewport is **1280×800**, fixed and
  documented.
- `geo <lat,lon> [--accuracy]` (grants the permission automatically), `tz
  <IANA>`, `locale <bcp47>` (`Emulation.setLocaleOverride` + Accept-Language),
  `ua <string>`.
- `media [--color-scheme dark|light] [--reduced-motion reduce] [--type print]
  [--forced-colors active]`.
- `offline on|off`, `throttle <preset|custom>` (`slow-3g`, `fast-3g`, `4g`,
  and `--latency --down --up`), `cpu-throttle <rate>`.
- `permissions grant|deny|reset <name...> [--origin]` (`Browser.setPermission`):
  geolocation, notifications, clipboard-read/write, camera, microphone and
  midi. Fake media streams come from the launch flags
  `--use-fake-device-for-media-stream` and `--use-fake-ui-for-media-stream`,
  on when `--fake-media` is given.
- `time freeze <iso> | time advance <ms> | time reset`
  (`Emulation.setVirtualTimePolicy`, or a `Date`/timer override init script).
  Needed to test time-dependent UIs deterministically. Evaluate which
  approach is robust, and record the result.

## Milestone 7.3 — Init scripts & page-level injection

- `init-script add <file|--code js> [--world main|isolated]` / `list` /
  `remove`: `Page.addScriptToEvaluateOnNewDocument` on all current and future
  targets.
- `expose <name>` (stretch goal): `Runtime.addBinding`, so page JS can call
  back. Calls appear in the `console` log as `binding <name>(args)`.

## Milestone 7.4 — Cookies, storage & auth state

- `cookies [--url u] [--domain d]`, `cookies set <name>=<value> [--domain
  --path --secure --httponly --samesite --expires]`, and `cookies clear`
  (`Storage.getCookies`/`setCookies`, browser-wide).
- `storage local|session [get <key>|set <k> <v>|clear|list] [--origin]` via
  `DOMStorage` or evaluation in a frame of that origin.
- `state save <file>` writes JSON in the common **`storageState`
  format** (cookies + localStorage per origin, plus our extension for
  sessionStorage and IndexedDB, opt-in with `--indexeddb`). Existing
  storageState auth files then work unchanged.
- `state load <file>`, and `open --state <file>`: apply the cookies before the
  first navigation. localStorage is seeded by an init script scoped to its
  origin, which then removes itself.
- Warn that state files hold secrets. Write them with mode 0600.
- `--profile <name>` (plan 02) is the heavier alternative for real persistent
  profiles.

## Milestone 7.5 — Extensions (stretch)

- `open --extension <dir>`: `--load-extension` +
  `--disable-extensions-except`. This requires `--headless` (new headless
  supports extensions). Test with a trivial fixture extension.

## Tests

- Routing: fulfill/abort/delay/modify/patch-json/times. Rules apply to an
  OOPIF and a popup opened afterwards, and to a request fired by an inline
  script on first load (proves pre-script application). There is also an
  overhead micro-benchmark and a service worker fixture.
- Emulation: each setting is verified from page JS (`matchMedia`,
  `navigator.language`, `Intl…timeZone`, `navigator.geolocation`,
  `navigator.onLine`, `devicePixelRatio`, `ontouchstart`).
- State round trip: log in on the fixture → `state save` → new session →
  `state load` → you are logged in. Compatibility test: load a storageState
  file saved by another tool (checked-in fixture).

## Acceptance criteria

- [x] Every emulation and route setting is applied to new tabs before their
      first script runs.
- [x] storageState files from other tools load unchanged.
- [ ] Route overhead < 1 ms/request p95 with 20 rules.

## Decisions log

_(append here)_
- 7.1: One `Fetch` pattern (`*`, request stage) per page and iframe session while any rule exists; the daemon matches rules itself. Our patterns (globs where `*` stops at `/`, `/regex/`, parts of URLs, as `wait --url` takes them) do not map onto Fetch's wildcards, and one pattern keeps rule changes to a single `Fetch.enable` per session. `--patch-json` asks for the response stage per request (`continueRequest` with `interceptResponse`), so other responses are never paused.
- 7.1: `route`'s JSON answer is `--json-body`, not the plan's `--json`: `--json` is the global output flag (plan 08 §4) on every command. `--delay` takes a duration with a unit (`2s`, `500ms`), like `--timeout`.
- 7.1: A rule has exactly one action; the CLI refuses two (e.g. `--abort` with `--json-body`). `--delay` combines with any action (a slow 500 is one rule); alone it delays and continues.
- 7.1: `block` takes patterns and the presets as flags (`block --images --fonts --media --ads`), one rule each, shown by their label in `routes`. Images, fonts and media go by resource type; `--ads` is a regular expression over 25 well-known ad and tracking hosts and their subdomains.
- 7.1: Rules live in the daemon's session settings, not the browser: they apply to every tab, popup and OOPIF (set up while the target is paused, so before its first script) and survive `open --restart`. They do not survive a daemon replaced by a CLI upgrade (the old daemon's rules are not handed over); a follow-up. The commands take no tab lock; `--tab` gets a warning, since rules are no tab's.
- 7.1: `Fetch` reports a `fetch()` as `XHR`, so for `--resource-type` `xhr` and `fetch` are one type; the journal shows Chrome's name (`xhr`).
- 7.1: While routes exist, requests skip service workers (`Network.setBypassServiceWorker`) and the HTTP cache (`Network.setCacheDisabled`): a cache hit or a service worker's answer never reaches `Fetch`. `open --bypass-service-worker` is a session setting (kept like `--ignore-https-errors`) that skips service workers without routes too.
- 7.1: A fulfilled request with an `Origin` gets `Access-Control-Allow-Origin: <origin>` and `-Allow-Credentials: true` unless the rule sets its own, and a CORS preflight for a request a fulfilling rule matches is answered 204 allowing what it asks, so mocking another origin's API needs no CORS headers. Preflights are no hits.
- 7.1: `--times N` removes the rule after its N-th match; when the last rule goes, interception stops. Ids (`r1`) are never reused in a session.
- 7.1: The request journal (1,000 entries, `routes --log`) records every request routing saw, matched or not, with the rule and outcome (`fulfill 200`, `abort blockedbyclient`, `continue`, `patch json (not JSON; not patched)`, `preflight allowed`). Rules carry an optional browser context (unset by the CLI) and are added in bulk (`route` and `block` send a list), for plan 11's per-context mock sets.
- 7.1: `Fetch` events now come through a lossless stream (`Session::events_lossless`, an unbounded channel per listener) in their own task per session: a dropped `Fetch.requestPaused` would hang the page. `--http-credentials` shares that task and the same `Fetch.enable`.
- 7.1: Requests of dedicated and shared workers are not routed yet (their sessions get no `Fetch.enable`); service workers are bypassed instead. A follow-up.
- 7.1: The overhead benchmark is `overhead_with_20_rules` in `crates/engine/tests/route.rs` (`#[ignore]`; 300 sequential fetches with and without 20 non-matching rules, p95 difference). Not run: deferred, benchmarks run after all plans. The acceptance criterion stays unticked.
- 7.1: The first acceptance criterion stays unticked until 7.2's emulation settings are applied the same way; routes are (tested for a request fired by an inline script on first load, a popup, and an OOPIF).
- 7.2: The device table is 45 phones, tablets and desktops plus their landscape entries (86 names), taken from an Apache-2.0 device descriptor list (attributed in `crates/engine/src/emulate/devices.rs`) with its names, viewports, screens, pixel ratios, `isMobile`, `hasTouch` and user agents. Names match loosely (case, spaces and punctuation do not count), so `emulate iphone 15` and `emulate Pixel 7 landscape` need no quotes; an unknown name gets up to three "did you mean" names.
- 7.2: The headless default viewport is 1280x800 through `Emulation.setDeviceMetricsOverride` on every page, since headless Chrome's own window gives an odd 780x437. Only for browsers chrome-cdp launched headless, and not when a `--chrome-arg=--window-size=…` sets the size; headed and connected browsers keep their window's. Two goldens (`scroll-by`, `click-obscured`) changed with it. Chrome routes input to out-of-process iframes and wheel scrolling by the real window, not the emulated viewport (a click at y=603 into a cross-site frame was lost in a 780x437 window), so a headless page's window is also grown (`Browser.setWindowBounds`, never shrunk) to hold its viewport.
- 7.2: Emulation belongs to the session, like routes (`Settings::emulation`): a change goes to every page and iframe session at once (only the calls for what changed), and each new one gets the settings during its setup, while paused. The engine records settings without a browser; the CLI needs a running session. No command takes a tab lock; `--tab` gets a warning.
- 7.2: Every command has its own reset (`viewport reset`, `tz reset`, `throttle off`, `cpu-throttle off`, `offline off`, `time reset`, …); `emulate reset` turns every emulation setting off, but not permissions. A command prints the setting it changed; `emulate` alone prints all of them. All of them share one `--json` data type (`EmulateData`: the whole state and the default viewport).
- 7.2: `viewport` takes `--touch` besides the plan's `--dpr` and `--mobile`, since `tap` needs touch (5 touch points). A device sets touch with its viewport; a later `viewport` or `ua` drops the device's name from the state but keeps what it did not change.
- 7.2: Out-of-process iframes get every setting except device metrics: Chrome refuses `setDeviceMetricsOverride` on anything but a top-level target, so such a frame keeps `devicePixelRatio` 1, and under a mobile viewport its first script sees `innerWidth` 0. The first-script test checks the rest there.
- 7.2: `locale` sets `Emulation.setLocaleOverride` and the `Accept-Language` (and `navigator.languages`) through `setUserAgentOverride`'s `acceptLanguage`, with an empty user agent standing for the browser's own; an empty user agent without a language clears the override. `en_GB` is taken as `en-GB`.
- 7.2: Chrome refuses an unknown time zone or locale only while no override is set; with one set it keeps the old one silently. So `tz` and `locale` check the name with `Intl` in the active tab first, then apply, and roll back if Chrome still refuses (`BadArgs`).
- 7.2: `offline` and `throttle` use `Network.emulateNetworkConditions` (deprecated, but it is what sets `navigator.onLine`; the `…ByRule` variant does not). The presets are Chrome DevTools': slow-3g (2s, 400/400 kbit/s), fast-3g (562.5ms, 1440/675), 4g (165ms, 8100/1350). Custom values are `--latency <duration>`, `--down`/`--up` in kbit/s.
- 7.2: `geo` defaults to an accuracy of 10 m and grants geolocation to every origin (recorded among the session's permissions); `geo reset` stops the position but leaves the grant.
- 7.2: `permissions grant|deny|reset <name…> [--origin]` takes 19 Permissions API names (`midi-sysex` is `midi` with sysex), all accepted by Chrome 154. The session keeps them and a new browser gets them at start; `permissions` lists them, and `reset` without names is `Browser.resetPermissions`. `clipboard`'s grants are recorded too, so they are listed and reset. `clipboard` still turns focus emulation off after its call: 7.2 adds no focus setting to restore.
- 7.2: `open --fake-media` adds `--use-fake-device-for-media-stream` and `--use-fake-ui-for-media-stream` to the Chrome args (a launch option: a running browser without them needs `--restart`). `open --device <name>` is a session setting, not a launch option: it emulates the device before loading the URL, needs no restart, and is kept when a CLI upgrade replaces the daemon.
- 7.2: `time` evaluated Chrome's virtual time: with `Emulation.setVirtualTimePolicy` `pause`, timers never fire and a `goto` never reaches its load event; `advance` fires a 5 s timer at once. Neither is a frozen clock a page keeps running under. `time freeze` instead adds an init script (main world, every frame, also run in the documents there are) that replaces `Date` with a proxy giving the frozen time for `new Date()`, `Date()` and `Date.now()`, and `Intl.DateTimeFormat`'s `format`/`formatToParts` without a date; timers, `performance.now()` and animations stay real. `time advance` moves it on (ms, or `90s`, `2h`), `time reset` takes the script out of every document. Times without an offset are UTC; without a time, `freeze` takes now. Workers keep the real clock. (Plan 11.6 changed `advance`: it now also moves `performance.now()` and runs the timers due by then; see its decisions log.)
- 7.2: Actions under emulation: clicks through the actionability pipeline land at a pixel ratio of 2, on Pixel 7 without a meta viewport (the page laid out 980 wide and zoomed out), and on an iPad in landscape, and `tap` works on a device; no coordinate changes were needed. CSS `zoom` and pinch zoom stay untested (a follow-up).
- 7.2: `tap`'s error without touch now suggests `emulate 'iPhone 15'` instead of a raw CDP call.
- 7.2: Like routes, emulation settings and permissions do not survive a daemon replaced by a CLI upgrade (only `open --device` does, as a session option); a follow-up.
- 7.3: Init scripts and exposed functions live in the session's settings (`Injector`), like routes and emulation, so they reach every tab, popup and OOPIF while it is paused and survive `open --restart`. Each page and iframe session records what it has by keys that are never reused (script ids; an exposed name gets a new serial each time), with the id Chrome gave, and a change sends each session only the difference, compared and recorded under the state lock so a session set up meanwhile gets each item once. It replaces the tab manager's unremovable internal list; 7.4's origin-scoped localStorage seeding can add its scripts through it.
- 7.3: `--world isolated` is chrome-cdp's own isolated world, the one `eval --world isolated` uses, not a new world per script, so an agent can read what the script set there. A script could overwrite chrome-cdp's helpers in that world; the help says where it runs.
- 7.3: `init-script add` takes `--now` (not in the plan): Chrome's `runImmediately`, to run the script in the open documents too. Without it a script runs from each page's next load, and the output says `reload` runs it in the current page.
- 7.3: The CLI reads the file (or stdin with `-`, or `--code`), up to 5MB, and sends the source; the absolute path is kept for `init-script list` and as the script's `sourceURL` (`file:///…`, or `init-script-s1.js` for code), so errors it throws name it at its own line numbers. A script is compiled (`Runtime.compileScript`, not run) in the active tab first, and one with a syntax error is refused (`BadArgs`, with its line and column). `init-script remove` says open pages keep what a script did until they load again.
- 7.3: `expose <name…>` (the stretch goal) is done. A `Runtime.addBinding` under a hidden name (`__chromeCdpExpose_<name>`) plus a wrapper script (run in the open documents too) that takes the binding off the global object and puts a function under the name: it takes any arguments, sends them as a JSON array (`undefined` as `null`, bigints as strings) and returns a promise, though without a return value. A call is a console entry of level info, source `binding`, kind the name, text `binding onSave({"id":1}, "x")`. `expose` alone lists the functions; `unexpose <name…>|--all` (not in the plan) takes them away, from the open documents too. Exposing a name again is a no-op.
- 7.3: Workers get neither: `Page.addScriptToEvaluateOnNewDocument` is not theirs.
- 7.3: The frozen clock (7.2) stays its own init script (`Host::clock`), since `time advance` replaces it in place; it could move onto the injector later. Like routes and emulation, init scripts and exposed functions do not survive a daemon replaced by a CLI upgrade (the follow-up now lists them too).
- 7.4: `cookies` reads and writes the browser's cookies (`Storage.getCookies`/`setCookies`/`clearCookies` on the browser session), so they belong to every tab and `--tab` gets a warning. `--url` asks the active tab's `Network.getCookies` for that URL, so Chrome's own matching (domain, path, Secure) picks them; `--domain` keeps a domain and its subdomains; `--name` (not in the plan) narrows both the list and `cookies clear`. A filtered `clear` sets each cookie again, expired: `Network.deleteCookies` is not on the browser session.
- 7.4: `cookies set --domain` follows CDP: `example.com` is host-only, `.example.com` goes to subdomains too (Chrome makes no domain cookie for `localhost` or an IP). Without `--url` or `--domain` the cookie is the active tab's URL's. `--expires` takes seconds since 1970, an ISO time or a time from now (`30d`); Chrome cuts it to 400 days. The command reads back what Chrome stored and reports a refused cookie as `BadArgs` with the likely reason (SameSite=None without `--secure`, `__Host-`/`__Secure-` rules).
- 7.4: `storage` is one command with positionals, `storage local|session [list|get KEY|set KEY VALUE|remove KEY|clear] [--origin]` (`remove` is not in the plan), and one schema. It evaluates in chrome-cdp's isolated world of the tab's main frame, or its first frame of `--origin` (an iframe's), rather than `DOMStorage`: the same code serves `state`, the page's own globals cannot interfere, and a page without an origin (`about:blank`) gets a clear `NotFound`. `get` of a missing key is `NotFound` (exit 1); `remove` of one is a no-op that says so.
- 7.4: A state file is `storageState` JSON, written pretty with mode 0600 (also over an existing file) by the daemon, with a warning that it holds secrets. Cookies without a SameSite are written `Lax`. Each origin also gets `sessionStorage` (chrome-cdp's extension, which other tools ignore; from the tab where the origin is found first, the active tab first), and with `--indexeddb` the format's `indexedDB` field and value encoding (`key`/`value` when JSON holds them, else `keyEncoded`/`valueEncoded`: `{d}`, `{m, id}`, typed arrays, …), so files move both ways. Origins with nothing stored are left out.
- 7.4: `state save` reads the origins of the frames open in some tab. Origins the context saw earlier are not tracked yet, so the help says to save while the app is open (a follow-up).
- 7.4 (follow-up): the tab manager records the origin of every document a frame of each context commits, and `state save` reads the origins no frame has open now in a hidden target (`Target.createTarget` with `hidden`) on a session of its own, whose `Fetch` answers every request with `<html></html>`. A hidden target is type `other` to auto-attach, so the tab manager neither lists it nor gives it routes, init scripts or emulation, without a special case.
- 7.4: `state load` adds the cookies (other cookies stay; expired ones are skipped with a warning; if Chrome refuses the batch, they are set one by one and the refused ones named) and replaces each listed origin's storage (localStorage always, sessionStorage and IndexedDB databases when the file has them). An origin open in a tab gets it at once. Any other gets a seed: an isolated-world init script (through 7.3's injector, not shown by `init-script list`) that fills the storage of the origin's first new document before the page's scripts run, then calls a binding that exists only in that world (`__chromeCdpStateSeeded`); the daemon then takes the seed out of every session, so the page's later changes stay. Its IndexedDB restore queues the delete and open back to back and writes the records in the upgrade transaction, so a page opening the database meanwhile waits for all of it.
- 7.4: `open --state <file>` reads the file in the CLI before anything starts, loads it after the launch (and `--device`) and before the URL, and prints a `state:` line. It is never kept in `meta.json`: it holds secrets, and a daemon replaced by an upgrade must not load it again. `--profile` (plan 02) stays the heavier alternative, named in the help.
- 7.4: The compatibility fixture in `testdata/state/` was saved by another tool after logging in on `testdata/site/state/login.html`. The test replaces only its origin, since the fixture server's ports differ per run. The fixture server gained `?set-cookie=` for the login's HttpOnly cookie.
- 7.5: `open --extension <dir>` (repeatable) loads unpacked extensions with `Extensions.loadUnpacked` over the pipe, after the launch and before the URL, with `--enable-unsafe-extension-debugging` on the command line, instead of the plan's `--load-extension` + `--disable-extensions-except`: Google Chrome 137 and later ignore `--load-extension`, and with it a broken manifest fails silently, where `loadUnpacked` names the problem (`BadArgs`, and the launch is undone). Chrome 154 for Testing loads extensions in the default (new) headless mode this way, content scripts, service worker and `chrome-extension://` pages included. `--disable-extensions-except` is not needed: chrome-cdp's temporary profiles have no other user extensions.
- 7.5: `open` and `status` print one `extension:` line per extension (name, version, `chrome-extension://<id>/`, directory), and `--json` has them under `browser.extensions`, so an agent can `goto` the extension's popup or options page. Extensions are a launch option like `--chrome-arg`: another set on a running session needs `--restart`, and a daemon replaced by an upgrade relaunches with them. An unpacked extension loaded this way is not kept in a `--profile`, so it is there only while `--extension` is given. chrome-headless-shell is refused before the launch (it cannot load extensions).
- Close-out: every milestone, the Tests section and the criteria were checked against the code and the real-Chrome tests (all pass). The first-script test now also covers the position, forced colors and network throttling (a first-script `fetch` held back by the latency), in the first tab, a popup and a cross-origin iframe; offline cannot be checked there, since an offline page does not load, and a CPU throttle has nothing deterministic to observe. `open --fake-media` gained a CLI test (`getUserMedia` gives an audio and a video track with no prompt).
- Close-out: a popup leaves its opener tab `hidden` in headless Chrome even while it is chrome-cdp's active tab, and Chrome holds back a hidden page's geolocation requests (they never answer), so the first-script test checks the iframe before opening the popup. The tab's visibility belongs to plan 03's tabs; a follow-up.
- Close-out: "Route overhead < 1 ms/request p95 with 20 rules" stays unticked. Deferred: benchmarks run after all plans (`overhead_with_20_rules`, `#[ignore]`d in `crates/engine/tests/route.rs`).
