# Plan 07 — Network control, emulation & state

Status: TODO
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
  tablets, generated from Playwright's MIT/Apache device descriptors with
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
- `state save <file>` writes JSON in a **Playwright-compatible
  `storageState` format** (cookies + localStorage per origin, plus our
  extension for sessionStorage and IndexedDB, opt-in with `--indexeddb`).
  Existing Playwright auth fixtures then work unchanged.
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
  `state load` → you are logged in. Compatibility test: load a state file
  produced by real Playwright (checked-in fixture).

## Acceptance criteria

- [ ] Every emulation and route setting is applied to new tabs before their
      first script runs.
- [ ] Playwright storageState files load unchanged.
- [ ] Route overhead < 1 ms/request p95 with 20 rules.

## Decisions log

_(append here)_
