---
name: chrome-cdp
description: Drive a real Chrome browser from the shell with chrome-cdp. Open pages, read them as an accessibility tree with refs, click, fill and type, check console errors and network requests, find the source line behind an element, assert what the page shows, take screenshots, mock requests, and write and run E2E tests against a mocked or real backend. Use it to look at, operate, test or debug a web page or web app.
---

# chrome-cdp

Every step is one shell command. A background daemon per session keeps the
browser, tabs and refs between commands. Output is compact text; add
`--json` for one JSON object, or `-q` for the bare result.

## Workflow

1. **Open**: `chrome-cdp open http://localhost:3000` starts headless
   Chrome. Then `chrome-cdp goto <url>`.
2. **Look**: `chrome-cdp snapshot -i` lists what you can act on, with refs:
   ```
   page t1 "Login" http://localhost:3000/login
   - textbox "Email" [e6]
   - textbox "Password" [e7] (password)
   - button "Sign in" [e8]
   ```
   Drop `-i` for the whole tree; `text` and `markdown` read the content,
   iframes included.
3. **Act by ref**: `chrome-cdp fill e6 ada@example.com`, `chrome-cdp click e8`.
   Actions wait until the element is visible, enabled and not covered,
   scroll it into view, and act like a real user.
4. **Read the report**: each action prints what it did and what followed
   (a navigation, a new tab, a dialog, a console error, a failed request):
   ```
   clicked e8 button "Sign in"
   → http://localhost:3000/dashboard "Dashboard" (412ms)
   ```
   Add `--snap` to also get what changed in the tree (`snapshot --diff`).
5. **After a navigation, refs are stale**: run `snapshot -i` again.
6. **When stuck**: read the error's `hint:` line (it names the next
   command), then `chrome-cdp errors`, `console`, `network --failed`, or
   `screenshot` (prints the PNG path; look at it). `why e8` says why an
   action on e8 would wait.
7. **Verify**: `chrome-cdp expect 'role=alert' text Saved`, `expect e6
   visible`, `expect '.row' count 3`, `expect --no-errors`. It retries
   for 5s, and a failure (exit 1) says what it saw instead. To guard a
   whole page, `expect-snapshot login` stores a baseline the first time,
   then prints a diff when the page differs (`--update` accepts it).
8. **Done**: `chrome-cdp close`.

## Locators

Commands that take an element take a ref (`e12`) or a locator. Quote
locators in single quotes.

- `'role=button[name="Save"]'`, `label=Email`, `placeholder=Search`,
  `testid=submit`, `'text=Sign in'`
- CSS: `#id`, `.class`, `'css=form button'`; XPath: `xpath=//a`
- `'#list >> text=Delete >> nth=-1'` chains; `'frame=payment >> #card'`
  looks inside an iframe (`chrome-cdp frames` lists them, and workers).
- `'role=row >> has="text=Invoice 12" >> role=button'` keeps the rows
  with that text inside; also `has-not=`, `and=`, `or=`.

## Pitfalls

- Never `sleep`. Actions, reads (`text <loc>`, …) and `goto` wait by
  themselves. For later changes: `wait --text 'Saved'`, `wait '#results'`,
  `wait --url /done`, `wait .spinner --state hidden`, `wait --idle`.
- Don't click with `eval "el.click()"` or coordinates: that skips the
  checks and the report. Use `click`, `fill`, `select`, `check`, `press`.
- `Obscured`: something covers the element (a cookie banner, a modal). The
  hint names a button to close it; run that first.
- `Ambiguous`: the locator matched several; the error lists them.
- A popup opens a new tab but stays in the background: `chrome-cdp tabs`,
  then `chrome-cdp tab t2`.
- An `alert`/`confirm` blocks the page (exit 5): `chrome-cdp dialog accept`.
- Long output is cut, with a line saying how to get more. Prefer
  `snapshot -i`, `--scope e12`, `find`, or `text <loc>`; for a big page,
  `snapshot --budget 2000` (`--expand r4` opens a folded part).
- Exit codes: 0 ok, 1 action failed, 2 usage, 3 timeout, 4 browser, 5 dialog.

## Find the code behind it

On a local dev server the page knows its sources; don't grep for them:

- `chrome-cdp source e12`: the file:line that rendered it, and its
  components (`components e12`: their props and state).
- `chrome-cdp debug log src/cart.ts:42 total item.qty`: log values each
  time that line runs, no code edits; the next action prints them.
- `chrome-cdp errors --locals`: the variables where an error was thrown.
- After an edit, `chrome-cdp wait --hmr` until the page has it (or the
  compile error).

## E2E tests with mocks

A project with `chrome-cdp.toml` (`chrome-cdp init e2e` writes one) keeps
tests in `e2e/scenarios/*.cdp` and fake API answers in
`e2e/mocks/*.toml`. To add a test:

1. Explore with the mocks on: `chrome-cdp mock use`, then the usual
   commands. `mock explain GET /api/items` says which mock answers.
2. Save what worked: `chrome-cdp history --as-scenario "adds an item"
   >> e2e/scenarios/items.cdp` (refs become stable locators).
3. Mock what the app fetches, in `e2e/mocks/base.toml`: a `[[mock]]` with
   `match = { method = "GET", path = "/api/items/:id" }` and
   `respond = { json_file = "data/items/{{params.id}}.json" }`, or a
   `[[collection]]` for data the app changes. `chrome-cdp schema mocks`
   has every key.
4. `chrome-cdp lint`, then `chrome-cdp test` (`file.cdp:12` runs one
   test). Each test gets a fresh context, frozen time and seeded random.
5. A failure prints a `report.md` path: the step, the error, the page,
   console errors, requests. Read it, fix the app or the test, rerun.
   Locators a redesign broke: `test --heal --write`. Before committing a
   test, `test file.cdp --repeat 20`: a flaky one names its likely cause.
   What to test next: `test --ui-coverage` lists, per page, the controls
   no test acted on and the mocks no test used.

```
@mocks base checkout

@test "places an order"
  goto /checkout
  click 'role=button[name="Place order"]'
  expect request POST /api/orders --json-body '{"qty":1}'
  expect 'role=heading[name="Thank you!"]' visible
```

A request in the mocks' scope that no mock answers fails the test and
names the closest mock. `@backend mock` marks tests that only work
against mocks; `chrome-cdp test --backend NAME` runs the same files
against a real API.

## Cheat sheet

```
open [url] [--headed]          start the browser; --session NAME for another
goto <url> | back | reload     navigate (goto /path with base_url)
snapshot -i | --diff           refs to act on | what changed
find <words> [--role button]   search elements, prints refs
locator e12 [--all]            the stable locator to put in a script or test
text [loc] | markdown | html   read the page or an element
value <loc> | attr <loc> <a>   a field's value | an attribute
click | dblclick | hover <loc>
fill <loc> <text>              set a field (type <text> types key by key)
press Enter | 'Control+A'      keys and shortcuts
keyboard down Shift           hold a key; keyboard up --all
select <loc> <v> | check <loc>  an option; a checkbox or radio (uncheck)
fill-form --values '{"Email":"a@b"}'  fill a whole form; see forms
upload <loc> <file...> [--drop] file inputs; --drop for drop zones
wait <loc> | --text t | --url u | --idle
expect <loc> visible|text t|value v|count n [--not]   assert, retrying
expect --url u | --no-errors | --no-issues | --no-failed-requests | --a11y | --perf good | --audit
expect-snapshot <name>         snapshot -i vs a stored baseline
screenshot [loc] [--full [loc]] [-o f.png]   --full loc: all of a scrolling container
errors | console | issues | network [--failed] | audit
eval '<js>'                    run JavaScript, print the value (elements with refs)
tabs | tab <id> | tab new <url> | tab close
frames                         the tab's iframes (cross-site too) and workers
dialog accept|dismiss          answer alert/confirm/prompt
route '<url glob>' --json-body '{...}'   mock a request
viewport 390x844 | emulate 'iPhone 15'   devices
state save f.json | state load f.json    keep a login
batch                          many commands from stdin, one per line
test [file.cdp:12]             run the E2E scenarios (see above)
```

More: `chrome-cdp help <command>` (every command's help has examples),
`chrome-cdp help locators|refs|waiting|errors|config|scripts`.
