# Migrating from Playwright

This guide maps Playwright's API (the library, `@playwright/test`,
`playwright-cli` and Playwright MCP) to `chrome-cdp` commands. Each
section is a table: the Playwright call, what to run instead, and a status.

| Status | Meaning |
|---|---|
| ✅ | a direct equivalent |
| 🟡 | partly covered, or covered another way (the note says how) |
| ❌ | missing today (the note has a workaround) |
| ➖ | not planned |

`chrome-cdp convert spec.ts` translates simple Playwright tests into `.cdp`
scripts using these tables ([below](#convert-a-spec-file)). For a whole
`@playwright/test` suite with `page.route` mocks, see
[e2e/migrating.md](e2e/migrating.md).

## The model is different

Playwright is a library: one program holds the browser and runs every step
in order. `chrome-cdp` gives you **one shell command per step**, and a
background daemon keeps the browser, tabs and refs between commands.

| Playwright | chrome-cdp |
|---|---|
| a script or test file | shell commands, or a `.cdp` script run with `chrome-cdp run` (syntax: [scenario-syntax.md](scenario-syntax.md)) |
| `const browser = await chromium.launch()` | nothing: the first command starts the session's browser (`open` sets launch options) |
| `browser.newContext()` / `context.newPage()` | `context new` (then `--context c1` on any command) / `tab new` |
| a `Page` object | the active tab (`tabs`, `tab t2`), or `--tab t2` on any command |
| a `Locator` | a locator string (`'role=button[name="Save"]'`) or a **ref** (`e12`) from `snapshot -i` or `find` |
| `await` on each call | each command returns when it finished and prints what it caused (a navigation, a dialog, a failed request) |
| exceptions | exit codes (1 failed, 2 usage, 3 timeout, 4 browser, 5 dialog) and an `error:` line with a `hint:` naming the next command |
| JavaScript values | text on stdout; `-q` for the bare value, `--json` for one JSON object |

**Refs** are what an agent should use most. `snapshot -i` prints the page as an
accessibility tree, and every element you can act on gets a short ref:

```
$ chrome-cdp snapshot -i
page t1 "Login" http://localhost:3000/login
- textbox "Email" [e6]
- textbox "Password" [e7] (password)
- button "Sign in" [e8]
$ chrome-cdp fill e6 ada@example.com
```

A ref names exactly one element and stays valid while the element does. A
committed script should use locators (`label=Email`) instead, because refs
differ from one session to the next.

## A test, side by side

```ts
import { test, expect } from '@playwright/test';

test('signs in', async ({ page }) => {
  await page.goto('/login');
  await page.getByLabel('Email').fill('ada@example.com');
  await page.getByLabel('Password').fill(process.env.APP_PASSWORD!);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page).toHaveURL(/dashboard/);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome, Ada');
  await expect(page.locator('.row')).toHaveCount(3);
});
```

```
# signs-in.cdp: chrome-cdp run signs-in.cdp
goto /login
fill label=Email ada@example.com
fill label=Password "${env:APP_PASSWORD}"
click 'role=button[name="Sign in"]'
expect --url /dashboard/
expect 'role=heading[level=1]' text 'Welcome, Ada' --exact
expect .row count 3
```

`goto /login` resolves against `base_url` in `.chrome-cdp.toml` (like
`baseURL`), or against the current page.

## Browser, sessions and pages

| Playwright | chrome-cdp | |
|---|---|---|
| `chromium.launch()` | `open` (headless), or any command | ✅ |
| `launch({ headless: false })` | `open --headed` | ✅ |
| `launch({ executablePath })` | `open --chrome PATH`, or `CHROME_CDP_CHROME` | ✅ |
| `launch({ args })` | `open --chrome-arg=--flag` | ✅ |
| `launchPersistentContext(dir)` | `open --profile NAME` or `open --user-data-dir DIR` | ✅ |
| `chromium.connectOverCDP(url)` | `connect 9222` or `connect ws://…` | ✅ |
| `browser.newContext()` | `context new [url]`, then `--context c1` on any command: its own cookies, storage, routes, mocks and emulation; `context close c1` | ✅ |
| a separate browser | `--session NAME` on every command: its own daemon and Chrome | ✅ |
| `context.newPage()` | `tab new [url]` | ✅ |
| `page.bringToFront()` | `tab t2` (commands act on the active tab) | ✅ |
| `context.pages()` | `tabs` | ✅ |
| `page.close()` | `tab close [t2]` | ✅ |
| `browser.close()` | `close` | ✅ |
| `browser.version()` | `status` | ✅ |
| `launch({ downloadsPath })`, `acceptDownloads` | downloads are always accepted into the session's directory: `downloads` | ✅ |
| `context.newCDPSession(page)` | `cdp Domain.method '{…}'` (`--tab`, `--frame`, `--listen` for events) | ✅ |
| `page.pause()` | `open --headed`, then `inspect` opens DevTools for the tab | 🟡 |

## Navigation

| Playwright | chrome-cdp | |
|---|---|---|
| `page.goto(url)` | `goto URL` (waits for load, then for the page to settle) | ✅ |
| `goto(url, { waitUntil: 'domcontentloaded' })` | `goto URL --wait domcontentloaded` | ✅ |
| `goto(url, { timeout })` | `goto URL --timeout 60s` | ✅ |
| `page.goBack()` / `goForward()` / `reload()` | `back` / `forward` / `reload` (`--hard` skips the cache) | ✅ |
| `page.url()` | `eval location.href`; `goto` and `tabs` print it | ✅ |
| `page.title()` | `eval document.title` | ✅ |
| `page.content()` | `html` | ✅ |
| `page.setContent(html)` | `goto 'data:text/html,…'`, or a file: `goto file:///tmp/page.html` | 🟡 |
| `response.status()` of `goto` | `goto` prints it; a 4xx/5xx is a warning, a failed load exits 1 | ✅ |

## Locators

Every command that takes an element takes the same locator syntax (`chrome-cdp
help locators`). Text values match a **case-insensitive part** of the text,
like Playwright's default. Put the value in double quotes for an exact,
case-sensitive match (`exact: true`), or write `/regex/flags`.

| Playwright | chrome-cdp locator | |
|---|---|---|
| `getByRole('button', { name: 'Save' })` | `'role=button[name=Save]'` (part of the name) | ✅ |
| `getByRole('button', { name: 'Save', exact: true })` | `'role=button[name="Save"]'` | ✅ |
| `getByRole('heading', { level: 2, name: /intro/i })` | `'role=heading[level=2][name=/intro/i]'` | ✅ |
| `getByRole(r, { checked, disabled, expanded, pressed, selected, includeHidden })` | `role=r[checked=true]`, `[disabled=false]`, `[expanded=true]`, `[pressed=mixed]`, `[selected=true]`, `[include-hidden=true]` | ✅ |
| `getByText('Welcome')` | `'text=Welcome'` | ✅ |
| `getByText('Welcome', { exact: true })` | `'text="Welcome"'` (or just `'"Welcome"'`) | ✅ |
| `getByLabel('Email')` | `label=Email` | ✅ |
| `getByPlaceholder('Search')` | `placeholder=Search` | ✅ |
| `getByAltText('Logo')` | `alt=Logo` | ✅ |
| `getByTitle('Help')` | `title=Help` | ✅ |
| `getByTestId('submit')` | `testid=submit` (`testid_attribute` in `.chrome-cdp.toml` = `testIdAttribute`) | ✅ |
| `locator('#id')`, `locator('.cls')`, `locator('[attr]')` | the same: `'#id'`, `.cls`, `'[attr]'` | ✅ |
| `locator('form button')` | `'css=form button'` (a bare CSS selector must start with `#`, `.` or `[`) | ✅ |
| `locator('xpath=//a')`, `locator('//a')` | `xpath=//a`, `//a` | ✅ |
| `a.locator(b)`, `a.getByRole(…)` | `'A >> B'` | ✅ |
| `.first()` / `.last()` / `.nth(2)` | `>> nth=0` / `>> nth=-1` / `>> nth=2` | ✅ |
| `.filter({ hasText: 'Total' })` | `>> has-text=Total` | ✅ |
| `.filter({ visible: true })` | `>> visible=true` | ✅ |
| `.filter({ has: loc })`, `.filter({ hasNot })`, `hasNotText` | a CSS `:has(…)`, or a ref from `snapshot`/`find` | ❌ |
| `a.or(b)`, `a.and(b)` | a CSS selector list (`'css=.a, .b'`), or refs | ❌ |
| `frameLocator('#pay')`, `locator('iframe').contentFrame()` | `'frame=pay >> …'` (the frame's name, id or a part of its URL), or the iframe's ref: `'e30 >> text=Pay'` | ✅ |
| shadow DOM | every locator but XPath looks inside open shadow roots, as in Playwright | ✅ |
| strict mode (an action on several matches throws) | `Ambiguous` (exit 1), and the error lists the matches with refs | ✅ |
| `page.locator('text=…')`, `'role=…'` selector strings | mostly the same syntax; Playwright-only CSS extensions (`:has-text()`, `:text()`, `:visible`, `:nth-match()`) are not supported: use `has-text=`, `text=`, `visible=true`, `nth=` | 🟡 |
| `page.accessibility.snapshot()`, `locator.ariaSnapshot()` | `snapshot` (`-i` only what can be acted on, `--scope LOC` a part) | ✅ |

`find <words>` has no Playwright equivalent: it searches the page by role,
name and text, and prints the best matches with refs.

## Actions

Every action waits for the element to be attached, visible, stable,
enabled and not covered, scrolls it into view, then acts with real input
events, like Playwright's actionability checks. `--force` skips the checks
(`force: true`), `--timeout` bounds the wait.

| Playwright | chrome-cdp | |
|---|---|---|
| `loc.click()` | `click LOC` | ✅ |
| `click({ button: 'right', modifiers: ['Shift'], position: { x, y } })` | `click LOC --button right --modifiers Shift --position 10,20` | ✅ |
| `click({ clickCount: 3 })` | `click LOC --count 3` | ✅ |
| `click({ force: true })` | `click LOC --force` | ✅ |
| `click({ noWaitAfter: true })` | `click LOC --no-wait-after` | ✅ |
| `click({ trial: true })` | `expect LOC visible` and `expect LOC enabled` | 🟡 |
| `loc.dblclick()` | `dblclick LOC` | ✅ |
| `loc.hover()` | `hover LOC` | ✅ |
| `loc.tap()` | `tap LOC` (with touch emulation: `emulate 'iPhone 15'` or `viewport … --touch`) | ✅ |
| `loc.fill('x')` | `fill LOC x` | ✅ |
| `loc.clear()` | `fill LOC ''` | ✅ |
| `loc.pressSequentially('x', { delay })`, `loc.type()` | `type x --in LOC --delay 50ms` | ✅ |
| `loc.press('Enter')` | `press Enter --in LOC` | ✅ |
| `page.keyboard.press('Control+A')` | `press Control+A` (Playwright's `ControlOrMeta+A` is `Mod+A`) | ✅ |
| `page.keyboard.type('x')`, `insertText('x')` | `type x` | ✅ |
| `page.keyboard.down('Shift')` / `up()` | `click --modifiers Shift`, a chord (`press Shift+ArrowDown`), or `cdp Input.dispatchKeyEvent` | 🟡 |
| `loc.check()` / `uncheck()` / `setChecked(b)` | `check LOC` / `uncheck LOC` (a no-op when it already is) | ✅ |
| `loc.selectOption('blue')`, `selectOption(['a', 'b'])`, `{ label }` | `select LOC blue`, `select LOC a b` (a value, else a label) | ✅ |
| `loc.setInputFiles('a.png')`, `[]` | `upload LOC a.png`, `upload LOC --clear` | ✅ |
| `page.waitForEvent('filechooser')` + `setFiles` | `upload 'role=button[name="Choose"]' a.png` (clicks it and answers the chooser) | ✅ |
| `loc.dragTo(target)` | `drag LOC TARGET` (mouse events, or HTML5 drag and drop) | ✅ |
| `loc.focus()` / `loc.blur()` | `focus LOC` / `blur LOC` | ✅ |
| `loc.scrollIntoViewIfNeeded()` | `scroll LOC` | ✅ |
| `page.mouse.wheel(0, 600)` | `scroll --by 0,600` (or `mouse wheel 0 600`); `scroll --to bottom` | ✅ |
| `page.mouse.move(x, y)` / `down()` / `up()` | `mouse move X Y` / `mouse down` / `mouse up` | ✅ |
| `page.mouse.click(x, y)` | `mouse down X Y` then `mouse up` | ✅ |
| `loc.dispatchEvent('click')` | `eval --in LOC 'el => el.dispatchEvent(new Event("click", { bubbles: true }))'` | 🟡 |
| `loc.highlight()` | `highlight LOC` | ✅ |
| clipboard via `context.grantPermissions` + `evaluate` | `clipboard read`, `clipboard write TEXT` | ✅ |

## Reading the page

| Playwright | chrome-cdp | |
|---|---|---|
| `loc.innerText()`, `textContent()` | `text LOC` | ✅ |
| `loc.allInnerTexts()`, `allTextContents()` | `text LOC` (one line per match) | ✅ |
| `loc.inputValue()` | `value LOC` | ✅ |
| `loc.getAttribute('href')` | `attr LOC href` | ✅ |
| `loc.innerHTML()`, `page.content()` | `html LOC`, `html` | ✅ |
| `loc.count()` | `count LOC` | ✅ |
| `loc.boundingBox()` | `box LOC` | ✅ |
| `loc.isVisible()`, `isEnabled()`, `isChecked()`, … | `count 'LOC >> visible=true' -q` (0 when not), or `expect LOC enabled --timeout 1ms` (exit 1 when not) | 🟡 |
| `loc.all()` | `find`, or `snapshot --scope LOC` for refs to each | 🟡 |
| `page.evaluate(fn)` | `eval 'JS'` (a function is called; a promise awaited) | ✅ |
| `page.evaluate(fn, arg)` | put the value into the script: `eval "(n => n * 2)(21)"` | 🟡 |
| `loc.evaluate(el => …)` | `eval --in LOC 'el => el.value'` | ✅ |
| `frame.evaluate(…)` | `eval --frame NAME '…'` | ✅ |
| `page.evaluateHandle`, `JSHandle`, `ElementHandle` | refs replace handles; `eval` returns values | ➖ |
| — (no equivalent) | `markdown`: the page's readable content as Markdown | |

## Waiting

`chrome-cdp` never needs a sleep: actions wait for their element, and `goto`,
`click` and friends wait for the navigation and requests they cause. For
anything else, use `wait` (`chrome-cdp help waiting`).

| Playwright | chrome-cdp | |
|---|---|---|
| `page.waitForSelector(sel)`, `loc.waitFor()` | `wait LOC` | ✅ |
| `loc.waitFor({ state: 'hidden' })` (`'attached'`, `'detached'`) | `wait LOC --state hidden` (`attached`, `detached`, also `enabled`) | ✅ |
| `page.waitForURL('**/dashboard')` | `wait --url '**/dashboard'` (a part, a glob or `/regex/`) | ✅ |
| `page.waitForLoadState('load')` / `'domcontentloaded'` | `wait --load` / `wait --domcontentloaded` | ✅ |
| `page.waitForLoadState('networkidle')` | `wait --idle` (or `wait --settle`: idle and no DOM changes) | ✅ |
| `page.waitForFunction(() => window.ready)` | `wait --fn 'window.ready'` | ✅ |
| `page.waitForResponse('**/api/save')` | `wait --response '**/api/save' --status 200`, after the command that sends it | ✅ |
| `page.waitForRequest(url)` | `wait --request URL` | ✅ |
| `page.waitForEvent('download')` | `wait --download` (`-q` prints the file's path) | ✅ |
| `page.waitForEvent('popup')`, `context.waitForEvent('page')` | the action that opens it says `→ new tab t2`; then `tab t2`, or `open --follow-popups` | ✅ |
| `page.waitForTimeout(500)` | `wait 500` (prefer a condition) | ✅ |
| text to appear / disappear | `wait --text Saved` / `wait --text-gone Loading` (`--in LOC`) | ✅ |

`wait --request` and `--response` look at what happened since the previous
command began, so `click` then `wait --response` catches the request the
click sent. This replaces `Promise.all([page.waitForResponse(…), click()])`.

## Assertions

`expect` retries until the assertion holds or `--timeout` passes (5s by
default, as in Playwright). A failure exits 1 and says what it expected,
what it saw and what to run next. `--not` inverts element matchers,
`--url` and `--title` (`expect(…).not`).

| Playwright | chrome-cdp | |
|---|---|---|
| `toBeVisible()` / `toBeHidden()` | `expect LOC visible` / `hidden` | ✅ |
| `toBeAttached()`, `{ attached: false }` | `expect LOC attached` / `detached` | ✅ |
| `toBeEnabled()` / `toBeDisabled()` | `expect LOC enabled` / `disabled` | ✅ |
| `toBeEditable()` | `expect LOC editable` | ✅ |
| `toBeChecked()`, `{ checked: false }` | `expect LOC checked` / `unchecked` | ✅ |
| `toBeFocused()` | `expect LOC focused` | ✅ |
| `toBeInViewport()` | `expect LOC in-viewport` | ✅ |
| `toBeEmpty()` | `expect LOC text '' --exact`, or for a field `expect LOC value ''` | 🟡 |
| `toHaveText('Saved')` (the whole text) | `expect LOC text Saved --exact` | ✅ |
| `toContainText('Saved')` | `expect LOC text Saved` | ✅ |
| `toHaveText(/saved/i)` | `expect LOC text /saved/i` | ✅ |
| `toHaveText(['a', 'b'])` (a list) | `expect LOC count 2`, then `expect 'LOC >> nth=0' text a --exact`, … | 🟡 |
| `toHaveValue('x')` / `toHaveValues(['a', 'b'])` | `expect LOC value x` / `expect LOC values a b` | ✅ |
| `toHaveAttribute('href', '/x')`, `toHaveAttribute('open')` | `expect LOC attr href /x`, `expect LOC attr open` | ✅ |
| `toHaveId('main')` | `expect LOC attr id main` | ✅ |
| `toHaveClass('btn primary')` (the whole attribute) | `expect LOC attr class 'btn primary'` | ✅ |
| `toContainClass('primary')` | `expect LOC class primary` | ✅ |
| `toHaveCSS('color', 'rgb(255, 0, 0)')` | `expect LOC css color red` (colours compare as colours) | ✅ |
| `toHaveCount(3)` | `expect LOC count 3` (also `--min`, `--max`) | ✅ |
| `toHaveRole('button')`, `toHaveAccessibleName('Save')` | `expect 'role=button[name="Save"]' count 1`, or read `snapshot --scope LOC` | 🟡 |
| `toHaveAccessibleDescription`, `toHaveJSProperty` | `wait --fn`, or `eval --in LOC 'el => el.value'` | 🟡 |
| `expect(page).toHaveURL(/dashboard/)` | `expect --url /dashboard/` (a string is a part of the URL or a glob) | ✅ |
| `expect(page).toHaveTitle('Home')` | `expect --title Home --exact` (without `--exact`: contains) | ✅ |
| `toHaveScreenshot('home.png')` | `expect-screenshot home` (`--full`, `--mask LOC`, `--threshold`, `--max-diff-pixels`; the caret is hidden and animations stopped, like `animations: 'disabled'`; experimental) | ✅ |
| `toMatchAriaSnapshot(template)` | `expect-snapshot NAME [--scope LOC]`: a stored text baseline of `snapshot -i`, a diff on failure | 🟡 |
| `--update-snapshots` | `expect-snapshot NAME --update`, `expect-screenshot NAME --update` | ✅ |
| `expect(response).toBeOK()` | `wait --response URL --status 200`; `goto` warns on a 4xx/5xx | 🟡 |
| `expect.soft(…)` | `run script.cdp --continue` (or `batch --continue`) | 🟡 |
| `expect.poll(fn)` | `wait --fn 'JS'` | 🟡 |
| `expect(…).toPass()` | — | ➖ |
| `expect(value).toBe(…)` (plain values) | compare in the shell: `test "$(chrome-cdp value e5 -q)" = x` | ➖ |
| `timeout` option, `expect.configure({ timeout })` | `--timeout 10s` on the command, or `CHROME_CDP_TIMEOUT` | ✅ |
| — (no equivalent) | `expect --no-errors`, `expect --no-failed-requests [--origin self]`, `expect --a11y` | |

## Dialogs, downloads, popups and frames

| Playwright | chrome-cdp | |
|---|---|---|
| `page.on('dialog', d => d.accept())` | an `alert` and `beforeunload` are accepted by themselves; a `confirm` or `prompt` makes the command exit 5, then `dialog accept` / `dialog dismiss` | ✅ |
| `dialog.accept('text')` (a prompt) | `dialog accept 'text'` | ✅ |
| handle every dialog the same way | `open --dialogs accept` (or `dismiss`) | ✅ |
| `download.path()`, `saveAs()` | `wait --download -q` prints the path; `downloads` lists them | ✅ |
| popups | a new tab `t2` (reported by the action); `tab t2`, `--tab t2`, or `open --follow-popups` | ✅ |
| `page.frame(name)`, `frameLocator` | `frame=NAME >> LOC` in any locator, `eval --frame NAME`, `snapshot --scope frame=NAME` | ✅ |
| shadow DOM | pierced by every locator but XPath | ✅ |

## Screenshots, PDF and video

| Playwright | chrome-cdp | |
|---|---|---|
| `page.screenshot({ path })` | `screenshot -o PATH` (without `-o`, prints where it saved it) | ✅ |
| `screenshot({ fullPage: true })` | `screenshot --full` | ✅ |
| `loc.screenshot()` | `screenshot LOC` | ✅ |
| `screenshot({ mask: [loc] })` | `screenshot --mask LOC` (repeat it) | ✅ |
| `screenshot({ type: 'jpeg', quality, omitBackground })` | `screenshot --format jpeg --quality 70 --omit-background` | ✅ |
| `screenshot({ scale: 'device' })` | `screenshot --scale device` | ✅ |
| — | `screenshot --annotate` labels each element with its ref | |
| `page.pdf({ path, format, landscape, margin })` | `pdf -o PATH --format A4 --landscape --margin 1cm` | ✅ |
| `recordVideo` | frames: `cdp Page.startScreencast`; a video is not recorded | ❌ |

## Console, errors and network

The daemon records each tab's console and requests from the moment it
opens, so there is nothing to subscribe to: read the logs when you need
them. By default each read shows what the previous one did not.

| Playwright | chrome-cdp | |
|---|---|---|
| `page.on('console')` | `console` (`--level warn`, `--grep`, `--follow`) | ✅ |
| `page.on('pageerror')` | `errors` (uncaught exceptions, `console.error`, failed loads) | ✅ |
| `page.on('request')`, `'response'`, `'requestfailed'` | `network` (`--failed`, `--url`, `--type`, `--status 4xx`, `--follow`) | ✅ |
| `response.body()`, `response.json()` | `network show ID --body` | ✅ |
| `page.on('download')`, `'dialog'`, `'popup'` | reported by the action that caused them | ✅ |
| `page.on('websocket')` | `cdp --listen 'Network.webSocket*' --count 0` | 🟡 |
| `page.exposeFunction(name, fn)`, `exposeBinding` | `expose NAME`: calls are logged in `console`; they cannot return a value | 🟡 |
| `page.addInitScript(script)` | `init-script add FILE`, or `init-script add --code 'JS'` | ✅ |
| `page.coverage` | `coverage` | ✅ |
| — | `perf` (load timing, Web Vitals), `mark NAME` + `--since NAME` | |

## Mocking and HAR

`route` rules belong to the browser context (the session's own unless
`--context`): they apply to every tab, popup and frame in it,
including the ones opened later, before the page's first request. The first
rule that matches handles a request.

| Playwright | chrome-cdp | |
|---|---|---|
| `page.route(url, r => r.fulfill({ json }))` | `route URL --json-body '{…}'` | ✅ |
| `fulfill({ status, body, contentType, headers })` | `route URL --status 404 --body 'x' --header 'Content-Type: text/html'` | ✅ |
| `fulfill({ path })` | `route URL --body-file PATH` | ✅ |
| `route.abort('failed')` | `route URL --abort failed` | ✅ |
| `route.continue({ headers, url })` | `route URL --set-header 'Name: v' --remove-header cookie --rewrite-url URL` | ✅ |
| `route.fetch()` + change the JSON + `fulfill` | `route URL --patch-json '{"items": []}'` | ✅ |
| a handler with its own logic (per request) | ordered rules with `--method`, `--resource-type`, `--times N` | 🟡 |
| `route` with a delay | `route URL --delay 2s` | ✅ |
| `page.unroute(url)`, `unrouteAll` | `unroute ID` (the rule's id from `routes`), `unroute --all` | ✅ |
| requests the routes saw | `routes` (hits per rule) | ✅ |
| blocking images, fonts, ads | `block --images --fonts --ads`, `block PATTERN` | ✅ |
| `setExtraHTTPHeaders(h)` | `route '**' --set-header 'Name: v'` | 🟡 |
| `page.routeWebSocket(url, handler)` | a `[[websocket]]` mock (`on_open`, `on_message`), and `mock ws send` ([docs/e2e/beyond-fetch.md](e2e/beyond-fetch.md)) | ✅ |
| Server-Sent Events | a mock's `respond = { sse = [...] }`, streamed to the page's `EventSource` | ✅ |
| server-side fetches (SSR) | `mock serve` (not interceptable in Playwright) | ✅ |
| `recordHar: { path }`, `routeFromHAR(file, { update: true })` | `har start`, then `har stop -o FILE` | ✅ |
| `routeFromHAR(file)` (replay) | `mock import FILE.har` writes a mock set (`--scope '**'` for every request, not only fetch and XHR), then `mock use NAME` | ✅ |
| `context.request`, `APIRequestContext` | `http GET /api/x` (with the context's cookies, `--json-body`, `--header`, `--expect-status`) | ✅ |

## Contexts, storage and emulation

Playwright sets these per context, and so does `chrome-cdp`: the session's
own context, or one from `context new` (`--context c1`). They apply to
every tab of it. `emulate reset` turns everything off.

| Playwright | chrome-cdp | |
|---|---|---|
| `context.storageState({ path })` | `state save FILE` (the same JSON format) | ✅ |
| `newContext({ storageState })`, `test.use({ storageState })` | `state load FILE`, or `open --state FILE` | ✅ |
| `context.cookies()` / `addCookies()` / `clearCookies()` | `cookies` / `cookies set name=value` / `cookies clear` | ✅ |
| `localStorage` via `evaluate` | `storage local` with `list`, `get KEY`, `set KEY VALUE`, `remove KEY` or `clear` (`storage session` for sessionStorage) | ✅ |
| `devices['iPhone 15']` | `emulate 'iPhone 15'`, or `open --device 'iPhone 15'` (Playwright's device list) | ✅ |
| `viewport`, `page.setViewportSize()` | `viewport 1440x900` | ✅ |
| `deviceScaleFactor`, `isMobile`, `hasTouch` | `viewport 390x844 --dpr 3 --mobile --touch` | ✅ |
| `userAgent` | `ua 'Mozilla/5.0 …'` | ✅ |
| `locale` / `timezoneId` | `locale de-DE` / `tz Europe/Berlin` | ✅ |
| `geolocation` + the permission | `geo 59.91,10.75` (grants it too) | ✅ |
| `permissions`, `context.grantPermissions([…])` | `permissions grant notifications camera` | ✅ |
| `colorScheme`, `reducedMotion`, `forcedColors`, `emulateMedia({ media: 'print' })` | `media --color-scheme dark --reduced-motion reduce --forced-colors active --type print` | ✅ |
| `offline`, `context.setOffline(true)` | `offline on` | ✅ |
| network throttling (via CDP) | `throttle slow-3g`, `cpu-throttle 4` | ✅ |
| `httpCredentials` (with `origin`) | `open --http-credentials user:pass [--http-credentials-origin https://app.test]` | ✅ |
| `ignoreHTTPSErrors` | `open --ignore-https-errors` | ✅ |
| `bypassCSP` | `cdp Page.setBypassCSP '{"enabled":true}'` | 🟡 |
| `javaScriptEnabled: false` | `cdp Emulation.setScriptExecutionDisabled '{"value":true}'` | 🟡 |
| `serviceWorkers: 'block'` | `open --bypass-service-worker` | ✅ |
| `page.clock.setFixedTime(t)`, `clock.install({ time })` | `time freeze 2026-01-31T09:30:00Z` | ✅ |
| `clock.fastForward('01:30')` | `time advance 90s` (moves `Date` and `performance.now()`, and runs the timers due by then, each once) | ✅ |
| `clock.runFor`, `pauseAt`, paused timers | — (timers keep running in real time) | ❌ |
| extensions (`--load-extension` in a persistent context) | `open --extension DIR` | ✅ |

## Tracing and debugging

| Playwright | chrome-cdp | |
|---|---|---|
| `context.tracing.start()` / `stop({ path })` (the Trace Viewer) | `trace start` / `trace stop -o trace.json`: a Chrome performance trace for DevTools or Perfetto, not a Playwright trace | 🟡 |
| step-by-step history of a run | each command prints what it did; `run --json` gives one JSON line per step | 🟡 |
| `PWDEBUG=1`, the inspector | `open --headed`, `highlight LOC`, `inspect` | 🟡 |
| `DEBUG=pw:api` | `-v`, `-vv`; the daemon logs to `daemon.log` in the directory `status` prints | ✅ |

## The test runner

`@playwright/test` maps to `chrome-cdp test`: `.cdp` scenario files with
`@test` blocks, mock files instead of `page.route` handlers, a fresh
browser context per test, and a `report.md` per failure.
[e2e/migrating.md](e2e/migrating.md) walks through moving a suite and its
mocks; [e2e/quickstart.md](e2e/quickstart.md) starts from scratch.

| Playwright | chrome-cdp | |
|---|---|---|
| a test file | a scenario file, `e2e/scenarios/login.cdp`, run by `chrome-cdp test` | ✅ |
| `test('name', …)` | `@test "name"` with indented steps | ✅ |
| `test.step('name', …)` | a `# name` comment; each step's line is in the report | 🟡 |
| `beforeEach` / `afterEach`, `beforeAll` / `afterAll` | `@before` / `@after`, `@before-all` / `@after-all` | ✅ |
| fixtures, helpers | step files: `@include ../steps/login.cdp email=ada@example.com` | 🟡 |
| parametrised tests, `process.env` | `@include` parameters, `${env:NAME}`, `${var:name}`, `${data:users.json#/0/email}` | ✅ |
| `baseURL`, `use: { viewport, locale, … }` | `base_url`, `viewport` in `[e2e]`; `@viewport`, `@device` | ✅ |
| isolation between tests | a new browser context per test (cookies, storage, mocks, emulation) | ✅ |
| `webServer` | `[e2e.web_server]` (a command, or `static = "dist"`) | ✅ |
| `retries`, `workers`, sharding, `--repeat-each` | `--retries`, `--jobs`, `--shard 2/4`, `--repeat 20` | ✅ |
| reporters (`html`, `junit`, `json`) | `--reporter text,junit,json,html` | ✅ |
| `test.only`, `skip`, `fixme`, tags, `--grep` | `@only`, `@skip "why"`, `@tags`, `--tags`, `--grep` | ✅ |
| `page.route` mocking in tests | mock files (`@mocks`, `default_mocks`) or `route` lines | ✅ |
| running the same tests against other environments | backends: `--backend staging` | ✅ |
| `projects` (browsers) | Chromium only | ➖ |
| component testing | — | ➖ |
| `codegen` | explore with `snapshot -i`, then `history --as-scenario` | 🟡 |

## Convert a spec file

`chrome-cdp convert` translates a Playwright test file into a `.cdp` script
using the tables above. It is best-effort: it understands the common calls
(`goto`, `getBy…` locators, `click`, `fill`, `check`, `selectOption`,
`press`, `waitFor…`, `route` with `fulfill`/`abort`, and the `expect`
matchers). Anything else becomes a comment such as
`# TODO line 27 (textContent() gives a value: read it with text LOC, or check
it with expect): const heading = …`, which gives the line in the spec, why, and
the original code. Rewrite the TODO lines before you run the script.

```
$ chrome-cdp convert tests/login.spec.ts -o login.cdp
converted tests/login.spec.ts: 2 tests, 14 steps, 1 TODO → /repo/login.cdp
$ chrome-cdp run login.cdp
```

Each `test(…)` becomes a section that starts with a `# test: …` comment.
`beforeEach` and `afterEach` steps are copied into each test, and
`test.use({ … })` becomes emulation commands. The tests run one after the other in the same session.
Locators stored in variables (`const email = page.getByLabel('Email')`) are
tracked, and so is `const p = page.waitForResponse(…)` followed by
`await p`. It does not run TypeScript: loops, conditions, helper functions,
and values read into variables need a person (or an agent) to rewrite
them.

## Where the models differ

- **Text matching.** `text`, `name`, `label`, … match a case-insensitive
  part of the text unless quoted, as in Playwright. But `expect LOC text`
  checks for a part of the text, like `toContainText`; add `--exact` for
  `toHaveText`.
- **One element.** Actions and single-element matchers fail when several
  elements match (`Ambiguous`). The error lists them with refs, so the fix
  is usually `click e14`, or `>> nth=0`.
- **Timeouts.** `expect` waits 5s by default, `wait` and actions 10s, and
  navigation 30s. `--timeout` (or `CHROME_CDP_TIMEOUT`) changes it per
  command.
- **What an action waits for afterwards.** `click` also waits for the
  navigation and requests it caused, and reports them. `--no-wait-after`
  returns at once.
- **Dialogs.** Playwright dismisses dialogs no handler takes. `chrome-cdp`
  accepts an `alert`. A `confirm` or `prompt` waits for `dialog accept` or
  `dialog dismiss`, and the blocked command exits 5.
- **Popups** open as new tabs that do not become active; switch with `tab`.
- **State is per context**: emulation, routes, mocks and init scripts
  apply to every tab of the context (the session's own, or one from
  `context new`), not to one page.
- **No test code.** A `.cdp` script has no variables to assign, loops or
  conditions, by design. Logic that needs them belongs in a shell or
  program that calls `chrome-cdp`, reading `-q` or `--json` output.
