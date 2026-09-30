# Playwright parity

What `chrome-cdp` offers for each Playwright capability an agent or a test
suite relies on, and whether the gap matters. It is the short form of
[migrating-from-playwright.md](migrating-from-playwright.md), which maps
every API call with examples; this page decides the v1 release.

| Status | Meaning |
|---|---|
| ✅ | a direct equivalent |
| 🟡 | partial, or covered another way (the note says how) |
| ❌ | missing |
| ➖ | not planned |

**Agent-critical** means an agent needs it routinely to open, operate or
verify a page, or to write and run E2E tests, and has no reasonable way
around it. **The v1 gate: no ❌ in an agent-critical row.** A test
(`crates/chrome-cdp/tests/parity.rs`) holds the tables to that, checks the
counts below, and checks that every command a row names exists.

Today: 134 rows; 120 ✅, 9 🟡, 0 ❌, 5 ➖. No row is ❌.

## Navigation

| Playwright | chrome-cdp | Status | Agent-critical |
|---|---|---|---|
| `page.goto(url)`, `waitUntil`, `timeout` | `goto URL` (waits for load and for the page to settle), `--wait domcontentloaded`, `--timeout 60s` | ✅ | yes |
| status of the main response | `goto` prints it; a 4xx/5xx is a warning, a failed load exits 1 | ✅ | yes |
| `goBack()`, `goForward()`, `reload()` | `back`, `forward`, `reload` (`--hard`) | ✅ | yes |
| `page.url()`, `page.title()` | `goto` and `tabs` print them; `eval document.title` | ✅ | yes |
| `baseURL` | `base_url` in `.chrome-cdp.toml`, so `goto /login` works | ✅ | no |
| `page.setContent(html)` | `goto 'data:text/html,…'`, or a `file://` URL | 🟡 | no |

## Locators

| Playwright | chrome-cdp | Status | Agent-critical |
|---|---|---|---|
| aria refs of Playwright MCP's snapshot | refs from `snapshot -i` and `find`: `click e12` | ✅ | yes |
| `getByRole(role, { name, exact, level, checked, … })` | `'role=button[name="Save"]'`, `[level=2]`, `[checked=true]`, `[include-hidden=true]`, … | ✅ | yes |
| `getByText(t, { exact })` | `text=Welcome`, `'text="Welcome"'`, `/regex/i` | ✅ | yes |
| `getByLabel` | `label=Email` | ✅ | yes |
| `getByPlaceholder` | `placeholder=Search` | ✅ | no |
| `getByAltText` | `alt=Logo` | ✅ | no |
| `getByTitle` | `title=Help` | ✅ | no |
| `getByTestId` (`testIdAttribute`) | `testid=submit` (`testid_attribute` in the config) | ✅ | yes |
| `locator(css)` | `'#id'`, `.cls`, `'[attr]'`, `'css=form button'` | ✅ | yes |
| `locator('xpath=…')` | `xpath=//a`, `//a` | ✅ | no |
| chaining: `a.locator(b)`, `a.getByRole(…)` | `'A >> B'` | ✅ | yes |
| `first()`, `last()`, `nth(i)` | `>> nth=0`, `>> nth=-1`, `>> nth=2` | ✅ | yes |
| `filter({ hasText })`, `filter({ visible })` | `>> has-text=Total`, `>> visible=true` | ✅ | yes |
| `filter({ has, hasNot, hasNotText })` | `>> has="…"`, `>> has-not="…"`, `>> has-not-text=Paid` | ✅ | no |
| `a.or(b)`, `a.and(b)` | `'A >> or="B"'`, `'A >> and="B"'` | ✅ | no |
| strict mode | several matches are `Ambiguous` (exit 1), listed with refs | ✅ | yes |
| Playwright's CSS extensions (`:has-text()`, `:visible`, …) | `has-text=`, `visible=true`, `nth=` | 🟡 | no |
| `browser_generate_locator` (MCP) | `locator e12` prints a ref's most robust locator (`--all`: every candidate); `history --as-scenario` writes them for a whole session | ✅ | no |

## Auto-wait and actionability

| Playwright | chrome-cdp | Status | Agent-critical |
|---|---|---|---|
| actionability: attached, visible, stable, enabled, not covered, scrolled into view | every action; a failure says what blocks it (`Obscured` names the covering element and the click that clears it) | ✅ | yes |
| waiting for the navigation an action causes | `click` and the others wait for it and for the requests they sent, and report them; `--no-wait-after` | ✅ | yes |
| `force: true` | `--force` | ✅ | no |
| `trial: true` | `why LOC --for click` runs the action's checks now without acting, and names each one's cause (what covers it, which ancestor hides or disables it); `expect LOC visible`, `expect LOC enabled` | ✅ | no |
| `waitForSelector`, `locator.waitFor({ state })` | `wait LOC --state visible\|hidden\|attached\|detached\|enabled` | ✅ | yes |
| `waitForURL`, `waitForLoadState` (incl. `networkidle`) | `wait --url`, `wait --load`, `wait --idle`, `wait --settle` | ✅ | yes |
| `waitForFunction` | `wait --fn 'JS'` | ✅ | yes |
| `waitForResponse`, `waitForRequest` | `wait --response URL --status 200`, `wait --request URL` (since the previous command began, so no race) | ✅ | yes |
| text to appear or go | `wait --text Saved`, `wait --text-gone Loading` | ✅ | yes |

## Actions

| Playwright | chrome-cdp | Status | Agent-critical |
|---|---|---|---|
| `click` (button, modifiers, position, clickCount) | `click LOC --button right --modifiers Shift --position 10,20 --count 3` | ✅ | yes |
| `dblclick` | `dblclick LOC` | ✅ | yes |
| `hover` | `hover LOC` | ✅ | yes |
| `fill`, `clear` | `fill LOC text`, `fill LOC ''` | ✅ | yes |
| `pressSequentially`, `keyboard.type` | `type text --in LOC --delay 50ms` | ✅ | yes |
| `press`, `keyboard.press` | `press Enter --in LOC`, `press Control+A`, `Mod+A` | ✅ | yes |
| `keyboard.down` / `up` | `keyboard down Shift`, `keyboard up Shift` / `--all`: held modifiers go with later clicks, keys, `mouse` and `drag` | ✅ | no |
| `check`, `uncheck`, `setChecked` | `check LOC`, `uncheck LOC` (a no-op when already so) | ✅ | yes |
| `selectOption` | `select LOC value…` (a value, else a label) | ✅ | yes |
| `setInputFiles`, the file chooser | `upload LOC file…` (an input, or a button that opens the chooser), `upload LOC --clear`; `upload LOC DIR` for a folder (webkitdirectory) input; `upload LOC file… --drop` for a drop zone | ✅ | yes |
| `dragTo` | `drag LOC TARGET` (mouse events, or HTML5 drag and drop) | ✅ | yes |
| `tap` | `tap LOC` (with touch emulation) | ✅ | no |
| `focus`, `blur` | `focus LOC`, `blur LOC` | ✅ | yes |
| `scrollIntoViewIfNeeded`, `mouse.wheel` | `scroll LOC`, `scroll --by 0,600`, `scroll --to bottom` | ✅ | yes |
| `page.mouse` | `mouse move\|down\|up\|wheel` | ✅ | no |
| `dispatchEvent` | `eval --in LOC 'el => el.dispatchEvent(…)'` | 🟡 | no |
| clipboard | `clipboard read`, `clipboard write TEXT` | ✅ | no |
| `highlight` | `highlight LOC` | ✅ | no |
| — | `forms` lists each form's fields (name, kind, state, value, validation message); `fill-form --values JSON` fills a whole form by field names in one step (`--submit`) | ✅ | no |

## Reading the page

| Playwright | chrome-cdp | Status | Agent-critical |
|---|---|---|---|
| `innerText`, `textContent`, `allInnerTexts` | `text [LOC]` | ✅ | yes |
| `inputValue` | `value LOC` | ✅ | yes |
| `getAttribute` | `attr LOC NAME` | ✅ | yes |
| `innerHTML`, `page.content()` | `html [LOC]` | ✅ | yes |
| `count` | `count LOC` | ✅ | yes |
| `boundingBox` | `box LOC` | ✅ | no |
| `isVisible`, `isEnabled`, `isChecked`, … | the snapshot shows states; `count 'LOC >> visible=true'`, `expect LOC enabled --timeout 1ms` | 🟡 | no |
| `evaluate(fn)`, `locator.evaluate`, `frame.evaluate` | `eval 'JS'`, `eval --in LOC 'el => …'`, `eval --frame NAME` | ✅ | yes |
| `evaluate(fn, arg)` | the value written into the script | 🟡 | no |
| `evaluateHandle`, `JSHandle`, `ElementHandle` | refs replace handles | ➖ | no |

## Dialogs, downloads, popups, frames and shadow DOM

| Playwright | chrome-cdp | Status | Agent-critical |
|---|---|---|---|
| dialogs (`page.on('dialog')`, `accept(text)`) | an alert is accepted; a confirm or prompt exits 5 until `dialog accept [TEXT]` or `dialog dismiss`; `open --dialogs accept` | ✅ | yes |
| downloads | always accepted into the session's directory (`open --downloads-dir` picks another); `wait --download -q` prints the path, `downloads` lists them, `downloads --clear` deletes them | ✅ | yes |
| popups | the action reports `→ new tab t2`; `tab t2`, `--tab t2`, `open --follow-popups` | ✅ | yes |
| frames (`frameLocator`, `contentFrame`, `page.frame`, `page.frames()`) | `'frame=NAME >> LOC'`, an iframe's ref (`'e30 >> text=Pay'`), `eval --frame`, `snapshot --scope`, `frames` lists them; cross-origin iframes too | ✅ | yes |
| `page.workers()`, `context.serviceWorkers()` | `frames` lists the tab's dedicated, shared and service workers | ✅ | no |
| shadow DOM | pierced by every locator but XPath, and by `snapshot` | ✅ | yes |

## Screenshots, PDF and video

| Playwright | chrome-cdp | Status | Agent-critical |
|---|---|---|---|
| `page.screenshot` (`fullPage`, `mask`, `type`, `quality`, `omitBackground`, `scale`) | `screenshot -o PATH --full --mask LOC --format jpeg --quality 70 --omit-background --scale device` | ✅ | yes |
| `locator.screenshot` | `screenshot LOC` | ✅ | yes |
| `page.pdf` (`headerTemplate`, `footerTemplate`, `scale`) | `pdf -o PATH --format A4 --landscape --margin 1cm --footer HTML --scale 0.8` | ✅ | no |
| `recordVideo` | not in v1: `test --trace` takes a screenshot after every step, and a failed test's `report.md` has the page | ➖ | no |

## Console, errors and network

| Playwright | chrome-cdp | Status | Agent-critical |
|---|---|---|---|
| `page.on('console')`, `worker.on('console')`, `waitForEvent('console')` | `console` (`--level warn`, `--grep`, `--follow`), `wait --console REGEX`, recorded from the tab's start, its frames' and workers' too (shared and service workers in each tab of their origin) | ✅ | yes |
| `page.on('pageerror')` | `errors` (uncaught exceptions, `console.error`, failed loads; `--sourcemaps`) | ✅ | yes |
| `page.on('request' \| 'response' \| 'requestfailed')` | `network` (`--failed`, `--url`, `--type`, `--status 4xx`, `--follow`) | ✅ | yes |
| `response.body()`, `json()` | `network show ID --body` | ✅ | yes |
| `page.on('websocket')`, frames | `network --type websocket` lists them; `network frames ID` their frames (`--grep`, `--dir`, `--follow`); `wait --ws URL --frame REGEX` waits for one | ✅ | no |
| `page.route` + `fulfill` / `abort` / `continue` / `fetch` | `route URL --json-body … \| --status … \| --abort \| --set-header … \| --rewrite-url … \| --patch-json …`; `unroute`, `routes`, `block` | ✅ | yes |
| `page.routeWebSocket` | a `[[websocket]]` mock, `mock ws send` | ✅ | no |
| HAR record (`recordHar`) | `har start`, `har stop -o FILE` | ✅ | no |
| HAR replay (`routeFromHAR`) | `mock import FILE.har`, then `mock use NAME` | ✅ | no |
| `context.request` (`APIRequestContext`) | `http METHOD URL` with the context's cookies | ✅ | no |
| `exposeFunction`, `exposeBinding` | `expose NAME`: calls are logged in `console` (`wait --binding NAME` waits for one); `--returns JSON` or `--returns-seq JSON-ARRAY` answers them. The answer is data, not a Node function | ✅ | no |
| `addInitScript` | `init-script add FILE`, `init-script add --code 'JS'`, `open --init-script FILE` | ✅ | no |
| `page.coverage` | `coverage` | ✅ | no |
| `newCDPSession` | `cdp Domain.method '{…}'` (`--listen` for events) | ✅ | no |

## State, contexts, emulation and clock

| Playwright | chrome-cdp | Status | Agent-critical |
|---|---|---|---|
| `storageState` (save, load) | `state save FILE`, `state load FILE`, `open --state FILE` (Playwright's JSON format) | ✅ | yes |
| cookies | `cookies`, `cookies set name=value`, `cookies clear` | ✅ | no |
| localStorage, sessionStorage | `storage local\|session list\|get\|set\|remove\|clear` | ✅ | no |
| `browser.newContext()` | `context new`, then `--context c1` on any command; `test` runs each test in a fresh one | ✅ | no |
| a persistent profile (`launchPersistentContext`) | `open --profile NAME`, `open --user-data-dir DIR` | ✅ | no |
| `connectOverCDP` | `connect 9222`, `connect ws://…` | ✅ | no |
| devices | `emulate 'iPhone 15'`, `open --device …` (Playwright's device list) | ✅ | yes |
| viewport, `deviceScaleFactor`, `isMobile`, `hasTouch` | `viewport 390x844 --dpr 3 --mobile --touch` | ✅ | yes |
| `userAgent` | `ua '…'` | ✅ | no |
| `geolocation` | `geo 59.91,10.75` | ✅ | no |
| `timezoneId`, `locale` | `tz Europe/Berlin`, `locale de-DE` | ✅ | no |
| `colorScheme`, `reducedMotion`, `forcedColors`, `emulateMedia` | `media --color-scheme dark --reduced-motion reduce --forced-colors active --type print` | ✅ | no |
| `offline` | `offline on` | ✅ | no |
| `permissions` | `permissions grant notifications camera` | ✅ | no |
| network and CPU throttling (CDP) | `throttle slow-3g`, `cpu-throttle 4` | ✅ | no |
| `httpCredentials`, `ignoreHTTPSErrors`, `serviceWorkers: 'block'` | `open --http-credentials user:pass --ignore-https-errors --bypass-service-worker` | ✅ | no |
| `clock.setFixedTime`, `install`, `fastForward` | `time freeze 2026-01-31T09:30:00Z`, `time advance 90s` | ✅ | no |
| `clock.runFor`, `pauseAt`, paused timers | `time pause`, `time run-for 1s` (every interval tick, a frame every 16ms), `time pause-at T`, `time resume` | ✅ | no |
| extensions (`--load-extension` in a persistent context, `context.serviceWorkers()` for their background) | `open --extension DIR`, `extensions load\|reload\|uninstall`, `extensions popup NAME` (its popup opens as a tab), `extensions storage NAME get\|set\|clear`; its service worker's console is in `console` and `errors` | ✅ | no |

## Tracing, snapshots and assertions

| Playwright | chrome-cdp | Status | Agent-critical |
|---|---|---|---|
| tracing and the Trace Viewer | `trace start`, `trace stop` (a Chrome performance trace); each command reports what it did; `test --trace` screenshots every step | 🟡 | no |
| `accessibility.snapshot()`, `ariaSnapshot()` | `snapshot` (`-i`, `--scope LOC`, `--diff`); a big page in `--budget N` tokens, alike cards or rows folded with `--fold`, a folded part shown with `--expand r4` | ✅ | yes |
| visibility and state matchers (`toBeVisible`, `toBeEnabled`, `toBeChecked`, `toBeFocused`, `toBeInViewport`, …) | `expect LOC visible\|hidden\|enabled\|disabled\|editable\|checked\|focused\|in-viewport\|attached\|detached` (`--not`) | ✅ | yes |
| `toHaveText`, `toContainText`, `toHaveValue(s)` | `expect LOC text T [--exact]`, `expect LOC value V`, `expect LOC values A B` | ✅ | yes |
| `toHaveCount` | `expect LOC count 3` (`--min`, `--max`) | ✅ | yes |
| `toHaveAttribute`, `toHaveClass`, `toContainClass`, `toHaveCSS`, `toHaveId` | `expect LOC attr NAME [V]`, `expect LOC class C`, `expect LOC css color red` | ✅ | no |
| `toHaveURL`, `toHaveTitle` | `expect --url /dashboard/`, `expect --title Home` | ✅ | yes |
| `toHaveScreenshot` | `expect-screenshot NAME` (experimental) | ✅ | no |
| `toMatchAriaSnapshot` | `expect-snapshot NAME`: a stored baseline of `snapshot -i` lines, which may be a template (`"*"`, `/regex/` values and `...` lines) | ✅ | no |
| `toHaveRole`, `toHaveAccessibleName`, `toHaveAccessibleDescription` | `expect 'role=button[name="Save"]' count 1`, or `snapshot --scope LOC` | 🟡 | no |
| `expect.soft`, `expect.poll` | `run --continue`; `wait --fn` | 🟡 | no |
| `toPass`, plain-value `expect` | the shell compares `-q` output | ➖ | no |
| — | `expect --no-errors`, `expect --no-failed-requests`, `expect --a11y` | ✅ | no |
| — | `layout` lints how the page looks (covered, overlapping, clipped, off-screen and too small elements, sideways scrolling, contrast on pixels), per `--viewports`; `expect --layout --baseline NAME` fails on findings the baseline does not list | ✅ | no |
| — | `compare A B` diffs two pages (URLs on two origins, tabs, sessions or `snapshot --json` files) by structure, and with `--visual` their pixels as named regions and a diff image; `--exit-code` exits 1 when they differ | ✅ | no |
| — | `explore URL` maps an app (links, and with `--click-buttons` buttons, tabs and menus; never forms or destructive controls unless told) in a context of its own: its states, how to reach each (`-o DIR` writes a `.cdp` scenario per state), and each state's console errors, failed requests, broken links and `--layout` findings; `--exit-code` exits 1 on a problem | ✅ | no |

## Test runner

| Playwright | chrome-cdp | Status | Agent-critical |
|---|---|---|---|
| test files, `test()`, hooks, `only`/`skip`/tags/`--grep` | `.cdp` scenarios with `@test`, `@before`/`@after`, `@only`, `@skip`, `@tags`, run by `test` (`--grep`, `--tags`) | ✅ | yes |
| isolation between tests | a fresh browser context per test (cookies, storage, mocks, emulation) | ✅ | yes |
| `page.route` mocking in tests | mock files (`@mocks`, `default_mocks`), or `route` lines | ✅ | yes |
| HAR replay in tests | `mock import FILE.har` writes a mock set the scenarios use | ✅ | no |
| `webServer` | `[e2e.web_server]`: a command to start, or a folder to serve | ✅ | yes |
| the same tests against other environments | backends: `test --backend staging` | ✅ | no |
| `retries`, `workers`, sharding, `--repeat-each` | `test --retries 2 --jobs 4 --shard 2/4 --repeat 20` | ✅ | no |
| reporters (`list`, `junit`, `json`, `html`) | `test --reporter text,junit,json,html`, and a `report.md` per failure | ✅ | yes |
| `--update-snapshots` | `test --update-snapshots` | ✅ | no |
| fixtures, `test.step` | step files through `@include` with parameters; `#` comments name steps | 🟡 | no |
| `projects` (other browsers) | Chromium only | ➖ | no |
| `codegen` (a recorder) | none; `history --as-scenario` turns an agent's session into a test | ➖ | no |
| converting existing tests | `convert spec.ts` (best effort) | ✅ | no |

## Gaps

No row is ❌.

Video (➖) is not in v1: an agent cannot watch one, and a failure already
leaves step screenshots (`test --trace`) and a `report.md`. The raw
`Page.startScreencast` frames need an ack per frame, so `cdp --listen` is
no substitute either.
