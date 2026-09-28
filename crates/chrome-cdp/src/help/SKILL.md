---
name: chrome-cdp
description: Drive a real Chrome browser from the shell with chrome-cdp. Open pages, read them as an accessibility tree with refs, click, fill and type, check console errors and network requests, assert what the page shows, take screenshots, mock requests. Use it to look at, operate, test or debug a web page or web app.
---

# chrome-cdp

Every step is one shell command. A background daemon per session keeps the
browser, tabs and refs between commands. Output is compact text; add
`--json` for one JSON object, or `-q` for the bare result.

## Workflow

1. **Open**: `chrome-cdp open http://localhost:3000` starts headless Chrome
   (a no-op if it runs already). Then use `chrome-cdp goto <url>`.
2. **Look**: `chrome-cdp snapshot -i` lists what you can act on, with refs:
   ```
   page t1 "Login" http://localhost:3000/login
   - textbox "Email" [e6]
   - textbox "Password" [e7] (password)
   - button "Sign in" [e8]
   ```
   Drop `-i` for the whole tree with text. `find <words>` searches for
   elements; `text` and `markdown` read the content.
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
   `screenshot` (prints the PNG path; look at the image).
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
  looks inside an iframe.

## Pitfalls

- Never `sleep`. Actions and `goto` wait by themselves. For later changes:
  `wait --text 'Saved'`, `wait '#results'`, `wait --url /done`,
  `wait .spinner --state hidden`, `wait --idle`.
- Don't click with `eval "el.click()"` or coordinates: that skips the
  checks and the report. Use `click`, `fill`, `select`, `check`, `press`.
- `Obscured`: something covers the element (a cookie banner, a modal). The
  hint names a button to close it; run that first.
- `Ambiguous`: the locator matched several; the error lists their refs.
- A popup opens a new tab but stays in the background: `chrome-cdp tabs`,
  then `chrome-cdp tab t2`.
- An `alert`/`confirm` blocks the page (exit 5): `chrome-cdp dialog accept`.
- Long output is cut, with a line saying how to get more. Prefer
  `snapshot -i`, `--scope e12`, `find`, or `text <loc>`.
- Exit codes: 0 ok, 1 action failed, 2 usage, 3 timeout, 4 browser, 5 dialog.

## Cheat sheet

```
open [url] [--headed]          start the browser; --session NAME for another
goto <url> | back | reload     navigate (goto /path with base_url)
snapshot -i | --diff           refs to act on | what changed
find <words> [--role button]   search elements, prints refs
text [loc] | markdown | html   read the page or an element
value <loc> | attr <loc> <a>   a field's value | an attribute
click | dblclick | hover <loc>
fill <loc> <text>              set a field (type <text> types key by key)
press Enter | 'Control+A'      keys and shortcuts
select <loc> <value>           a <select> option
check | uncheck <loc>          checkboxes and radios
upload <loc> <file...>         file inputs
wait <loc> | --text t | --url u | --idle
expect <loc> visible|text t|value v|count n [--not]   assert, retrying
expect --url u | --no-errors | --no-failed-requests | --a11y
expect-snapshot <name>         snapshot -i vs a stored baseline; a diff if not
screenshot [loc] [--full] [-o f.png]
errors | console | network [--failed]
eval '<js>'                    run JavaScript, print the value
tabs | tab <id> | tab new <url> | tab close
dialog accept|dismiss          answer alert/confirm/prompt
route '<url glob>' --json-body '{...}'   mock a request
viewport 390x844 | emulate 'iPhone 15'   devices
state save f.json | state load f.json    keep a login
batch                          many commands from stdin, one per line
```

More: `chrome-cdp help <command>` (every command's help has examples),
`chrome-cdp help locators|refs|waiting|errors|config|scripts`.
