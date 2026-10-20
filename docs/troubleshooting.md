# Troubleshooting

Every chrome-cdp error names its kind, says what happened, and ends with a
`hint:` holding the next command to run. Try the hint first; this page is
for when it is not enough. Errors are grouped by exit code, as
`chrome-cdp help errors` lists them:

| Exit | Meaning | Kinds |
|---|---|---|
| 0 | success | |
| 1 | the action or check failed on the page | [NotFound](#notfound), [Ambiguous](#ambiguous), [Obscured](#obscured), [NotEnabled](#notenabled), [RefStale](#refstale), [RefDetached](#refdetached), [ExpectFailed](#expectfailed), [EvalError](#evalerror), [NavigationFailed](#navigationfailed), [NoSource](#nosource), [HotUpdateFailed](#hotupdatefailed) |
| 2 | the command line is wrong | [BadArgs](#badargs), [BadLocator](#badlocator) |
| 3 | something did not happen in time | [Timeout](#timeout) |
| 4 | the browser, the daemon or the machine | [ChromeNotFound](#chromenotfound), [LaunchFailed](#launchfailed), [BrowserCrashed](#browsercrashed), [PageCrashed](#pagecrashed), [DaemonUnavailable](#daemonunavailable), [ProtocolError](#protocolerror), [InstallFailed](#installfailed) |
| 5 | a JavaScript dialog blocks the page | [DialogOpen](#dialogopen) |
| 6 | a load run's threshold failed | [ThresholdFailed](#thresholdfailed) |

With `--json` the kind is `error.kind`, and `error.details` holds what the
text form only summarises (the candidates, the covering element, what a
wait observed):

```sh
chrome-cdp click e6 --json | jq -r .error.kind
```

Before anything else, two commands answer most "why":

```sh
chrome-cdp doctor      # Chrome, its libraries and sandbox, the session dir
chrome-cdp errors      # the page's own exceptions and failed loads
```

## Exit 1: the page

### NotFound

```
error: no element matches #nothing
  waited: 1s
  hint: chrome-cdp snapshot   (shows the page; use a ref from it)
```

Nothing matched the locator within `--timeout`. Actions wait for the
element to appear, so this is not a race: the locator is wrong, or the
page is not where you think it is.

- Look at the page: `chrome-cdp snapshot -i`, or search it: `chrome-cdp
  find 'sign in'`. Use a ref from the output.
- `text=` matches a part of the visible text, any case; a quoted
  `text="Sign in"` must match all of it. Hidden elements do not match.
- Inside an iframe? Prefix the frame: `frame=payment >> #card`.
- The page may still be loading data: `chrome-cdp wait --idle`, then try
  again, or check `chrome-cdp network --failed`.

### Ambiguous

```
error: 4 elements match .item: e1, e2, e3, e4
  e1: listitem "Apples"
  e2: listitem "Bananas"
  e3: listitem "Cherries" (hidden)
  e4: listitem "Dates"
  hint: chrome-cdp html e1   (the first one), or add >> nth=<index> to .item
```

An action needs exactly one element. The error lists every match with a
new ref and where it is (`e41: button "Delete" (in row "Invoice 12 …")`):
use the ref you meant. To keep a locator instead, narrow it: `'role=row
>> has-text=Invoice 12 >> text=Delete'`, or `>> nth=0`. `count`, `text`
and `wait` take any number of matches.

### Obscured

```
error: #under is covered by another element
  covering: div "We use cookies Privacy policy Accept" [e1]
  waited: 1s
  hint: chrome-cdp click e2   (button "Accept" inside it), or chrome-cdp click '#under' --force
```

Something else would receive the click: a cookie banner, a modal, a
sticky header, a loading overlay. The error names it with a ref and, when
it has one, the button that likely dismisses it. Click that first. If the
cover goes away by itself (an overlay while data loads), `chrome-cdp wait
<cover> --state hidden`. `--force` clicks anyway, where a user could not.

### NotEnabled

```
error: #off is disabled: it has the disabled attribute
  hint: chrome-cdp snapshot   (a control is often disabled until the form is complete)
```

The control is disabled (or `aria-disabled`, or a read-only field for
`fill`). Forms often keep their submit button disabled until every
required field is valid: fill them, then look at `snapshot -i` again.

### RefStale

```
error: ref e1 is from an earlier page of tab t1
  hint: chrome-cdp snapshot   (gives the new document's refs)
```

The tab (or the frame) loaded a new document since the ref was given, and
refs do not survive that. Take a new `snapshot -i` or `find`. An action
that navigates says so in its report (`→ navigated to …`), which is the
cue that old refs are gone. Refs also belong to one tab: pass the same
`--tab`. [locators-and-refs.md](locators-and-refs.md) has the ref rules.

### RefDetached

```
error: element e3 is no longer in the page
  was: button "Delete"
  note: 2 elements have its role and name now; not guessing which
  hint: chrome-cdp snapshot   (shows the refs the page has now)
```

The page is the same, but its framework replaced the element and no one
element of the same role and name took its place (none, or several). Take
a new snapshot. With `--strict-refs`, any replaced element fails this way
instead of moving to its look-alike.

### ExpectFailed

```
error: expect failed: '#status' text Saved
```

An `expect` retried its check until its timeout (5 s by default) and it
never held. The notes say what it saw last (the text, the count, the
URL). The page may be right and the expectation wrong: read the notes
before changing the page. In `chrome-cdp test`, the failed test's
`report.md` has the screenshot, console and network around the failure
([e2e/debugging.md](e2e/debugging.md)).

### EvalError

```
error: the script threw TypeError: Cannot read properties of null (reading 'sum')
  at: explode (/src/app.ts:8:1)
  at: eval:1:1
```

The JavaScript passed to `eval` threw or did not
compile. The stack points at the page's own sources through source maps
on localhost. `eval` runs like a line typed into DevTools' console:
`--in <locator>` binds an element as `el`, `--frame` picks an iframe.

### NavigationFailed

```
error: connection refused on localhost:47
  hint: is your dev server running? start it, then: chrome-cdp goto http://localhost:47/login
```

The page could not load: the server is down, the name does not resolve,
the certificate is bad, or the server wants a login. For a local dev
server, start it and check its port. For a self-signed certificate,
`open --ignore-https-errors`; for HTTP authentication, `open
--http-credentials user:password`. A 4xx or 5xx response is not this
error, only a warning: the page loaded.

### NoSource

```
error: e4 button "Add" was rendered by a production build of react: it keeps no debug info
  hint: open the app from its dev server (a development build keeps where each element came from), then run source again
```

`source` found nothing that says where the element came from. The
message says which of these it is (`details.reason` in `--json`):

- `not_dev_build`: a framework rendered it, but its production build
  keeps no debug info. Open the app from its dev server.
- `before_stacks`: the element was made before chrome-cdp recorded node
  creation stacks (they were turned on while the page was open).
  `chrome-cdp reload`, then run `source` again.
- `stacks_off`: node stacks are off for this page (by default they are
  on for localhost pages only). `chrome-cdp open --node-stacks on`,
  then `reload`.
- `library_only`: only library code (`node_modules/`) made it; ask about
  the element around it that the app renders.

`components` fails with the same kind: `not_dev_build` for a production
build, `no_framework` when the page has no React, Vue or Svelte app, and
`no_component` when no component of a dev build rendered the element
(`source` still finds where it came from).

### HotUpdateFailed

```
error: hot update failed: [PARSE_ERROR] Unexpected token (src/components/CartItem.tsx:23)
  client: vite
  at: src/components/CartItem.tsx:23:8
  hint: chrome-cdp wait --hmr   (after you fix it and save, waits for the next update)
```

`wait --hmr` saw the dev server (Vite, webpack-dev-server, Next.js) fail
to build the edited file, or the page fail to run it: the page still runs
the old code. The message is the compiler's, and the place is on disk
when the file is under the project root. Fix it, save, and run the hint;
it waits for the next update, not this one again. `details.hot_update`
has the update as `--json` shows it.

`wait --hmr` on a page with no hot-reload client fails at once with
`NotFound` (`details.reason` is `no_client`): the page did not come from a
dev server, or its client never connected. Open the app from its dev
server's URL.

## Exit 2: the command line

### BadArgs

A flag or argument is wrong. Typos in commands, flags, device names and
help topics get a "did you mean" and the corrected command in the hint.
`chrome-cdp help <command>` shows every flag with examples; the same text
is in the [command reference](commands/README.md). In a `.cdp` script
the error points at `file.cdp:line:column`, and `chrome-cdp lint` finds
them all without running anything.

### BadLocator

The locator does not parse, such as `role=button[nme=Save]` or a bare word
(`h1` is neither CSS nor text: write `css=h1` or `text=h1`). The hint
suggests the likely meaning; `chrome-cdp help locators` has the grammar.

## Exit 3: time

### Timeout

```
error: waited 500ms for #results to be visible
  observed: no element matches
  page: about:blank ""
  hint: chrome-cdp wait '#results' --timeout 1s   (no element matches the locator yet; check it)
```

Something did not happen within `--timeout` (10 s for actions and `wait`,
30 s for navigation). A timeout always says what it saw instead: how many
elements matched and how many were visible, the requests still in flight,
the URL, the unread console errors. Read that before raising the timeout:

- `observed: no element matches`: the locator is wrong, or the page never
  rendered it (`chrome-cdp errors`, `chrome-cdp network --failed`).
- A navigation that never reached `load`: a slow image or script. `goto
  --wait domcontentloaded` waits for less.
- `wait --idle` or `--settle` that never settles: the page polls or
  streams. Wait for the element or text you need instead.

[waiting.md](waiting.md) explains what each command waits for.

## Exit 4: the browser, the daemon, the machine

`chrome-cdp doctor` checks everything below and prints a fix per problem.

### ChromeNotFound

No Chrome was found. chrome-cdp looks at `--chrome`, `$CHROME_CDP_CHROME`,
its own cache, then a system Google Chrome or Chromium. Install one:
`chrome-cdp install` (Chrome for Testing into the cache; `--with-deps` on
Linux also installs its system libraries), or point `CHROME_CDP_CHROME`
at a binary. `--auto-install` (`CHROME_CDP_AUTO_INSTALL=1`) installs it
the first time one is needed.

### LaunchFailed

Chrome would not start; the notes show the end of its output, and
`chrome-cdp status --logs` the rest.

- `error while loading shared libraries`: Linux is missing a library.
  `chrome-cdp install --with-deps` installs them (it prints the `apt-get`
  or `dnf` line first).
- `No usable sandbox!`: on Ubuntu 23.10+ AppArmor blocks user namespaces
  for binaries without a profile. `chrome-cdp install --with-deps` writes
  the profile for chrome-cdp's own installs; [chrome-flags.md](chrome-flags.md#the-sandbox)
  explains the rule. Do not reach for `--no-sandbox`.
- `the profile … is in use by another Chrome`: another Chrome has that
  `--profile` or `--user-data-dir` open; close it or use another one.

### BrowserCrashed

Chrome exited while the session was using it (killed, out of memory, a
crash). The session's daemon goes with it; `chrome-cdp open` starts a new
one. In containers, a small `/dev/shm` used to crash Chrome; chrome-cdp
passes `--disable-dev-shm-usage` on Linux, so check memory limits first.

### PageCrashed

The tab's renderer crashed ("Aw, Snap"), usually from memory. The tab
fails every command until `chrome-cdp reload` (or `goto`) loads it again;
the other tabs are fine.

### DaemonUnavailable

The CLI could not reach or start the session's daemon, or the session is
not running (`session default is not running` → `chrome-cdp open`). If
starting fails, the error names the daemon's log; `chrome-cdp status`
shows the session dir, which must be yours and mode 0700. `chrome-cdp
sessions` lists what runs; `chrome-cdp close --all` stops everything. A
daemon from an older chrome-cdp is replaced after an upgrade.

### ProtocolError

Chrome rejected a DevTools Protocol call. From `chrome-cdp cdp`, the
method or its parameters are wrong: the message is Chrome's own. From
any other command it is a bug in chrome-cdp or a Chrome too old for it
(`chrome-cdp status` shows the version; the oldest supported is four
majors behind stable); please report it with the command and `-vv`
output. `inspect` in a headless session is this kind too: it needs
`open --headed`.

### InstallFailed

`install` or `upgrade` could not download, verify or unpack. Check the
network, or point `CHROME_CDP_CFT_URL` at a
mirror of the Chrome for Testing index. `upgrade` only replaces a binary
installed by the install script; brew, npm and cargo installs update
through their own tool ([install.md](install.md)).

## Exit 5: dialogs

### DialogOpen

```
error: dialog open in tab t1: confirm "Delete item?"
  hint: chrome-cdp dialog accept, or chrome-cdp dialog dismiss
```

A `confirm` or `prompt` is waiting for an answer, and nothing can happen
in the page until it gets one. Answer it (`dialog accept [text]` or
`dialog dismiss`), then go on. Alerts and "leave the page?" dialogs are
accepted by themselves. `open --dialogs accept|dismiss` answers every
dialog of the session without asking.

## Exit 6: load thresholds

### ThresholdFailed

```
error: 1 of 3 thresholds failed
  failed: p95(GET /api/cart) < 300ms: 412 ms
  hint: the run's numbers are in e2e/.results/load/<run>/report.md
```

`chrome-cdp load` ran, and the site missed a `[thresholds]` limit of the
profile ([e2e/load.md](e2e/load.md#thresholds)). The notes list each
threshold that failed with what was measured; `report.md` and
`report.html` in the run's folder have every endpoint, step and Web
Vital. A threshold whose group never had a value (an endpoint no request
had, an INP without interactions) fails too, and says the closest groups
that had one: check its name. `--compare` with an earlier run shows what
changed.

## Not an error, but not right either

- **The click worked but nothing happened.** Read the action's report: it
  lists the console errors and failed requests the click caused. `chrome-cdp
  errors` and `chrome-cdp network --failed` show the rest.
- **The output was cut.** Every command bounds its output and says how to
  get more (`--max-chars`, `--limit`, `--scope`).
- **Different results headed and headless.** Headless pages get a fixed
  1280x800 viewport; `viewport` or `emulate` sets one for both.
- **A command is slow the first time.** The first command of a session
  starts the daemon and Chrome; later ones reuse them.
