# Plan 14 — Parity gaps and small commands

Status: DONE (14.1-14.7)
Depends on: v1 (plans 01–11). Track 3 of [plan 12](12-after-v1.md). Plan 13
(every frame, every worker) is not required, but where it has landed, use its
target graph instead of adding per-session code.

## Goal

Remove "chrome-cdp can't do X" from Playwright migrations. This plan closes
the three ❌ rows of `docs/playwright-parity.md`, the 🟡 rows that agents or
converted suites hit, and the small commands the backlog asked for. Each
item is small; the milestones group them by the code they touch.

When this plan is done:

- `docs/playwright-parity.md` has no ❌ row.
- The 🟡 rows for `keyboard.down`/`up`, `exposeFunction`, WebSocket frames,
  `toMatchAriaSnapshot` and `browser_generate_locator` are ✅.
- Every backlog note filed under track 3 in plan 12 §6 is done and its file
  in `.looper/tasks/` is deleted.

## Rules for every milestone

- v1's command line and `--json` output are stable (`docs/output.md`). Add
  commands, flags and JSON fields; never remove or rename one, and never
  change a field's type. `cargo xtask release check` must keep passing.
- Every new command or flag gets help text with an example (plan 08 §8.3),
  a regenerated `docs/commands/` page (`cargo xtask docs`; those files say
  "do not edit"), golden output in `crates/chrome-cdp/tests/golden/`, and a
  line in `chrome-cdp skill` only when an agent should reach for it.
- Errors follow plan 08 §3: what happened, why, and the next command.
- When a parity row changes, update both `docs/playwright-parity.md` (its
  row and the "Today:" line; `crates/chrome-cdp/tests/parity.rs` checks the
  counts) and the matching row of `docs/migrating-from-playwright.md`.
- Engine tests go in `crates/engine/tests/`, CLI tests in
  `crates/chrome-cdp/tests/`. Fixture pages go in `testdata/site/`.

## Milestone 14.1 — Locators

Backlog: `locator-has-filter`, `locator-for-ref`.

### Grammar additions

The grammar lives in `crates/engine/src/locator/parse.rs` (steps, `KINDS`,
error texts) and is evaluated by `query()` in
`crates/engine/src/injected/src/locator.ts`. Add five step kinds. Each is a
filter or combinator of what the steps before it found, like `nth=`,
`visible=` and `has-text=`, so none may be the first step.

| Step | Keeps | Playwright |
|---|---|---|
| `has=<locator>` | elements with at least one descendant the inner locator matches (searched with the element as root) | `filter({ has })` |
| `has-not=<locator>` | elements with no such descendant | `filter({ hasNot })` |
| `has-not-text=<text>` | elements whose text does not match (same text rules as `has-text=`) | `filter({ hasNotText })` |
| `and=<locator>` | elements the inner locator also matches, searched in the same scopes as the last finder step | `a.and(b)` |
| `or=<locator>` | the elements so far plus those the inner locator matches in the same scopes, in document order, without duplicates | `a.or(b)` |

- **Inner locators.** The value is a full locator. It needs double quotes
  when it contains `>>` or spaces at its ends:
  `.row >> has="role=button[name=Delete] >> visible=true"`. Inside the
  quotes, `\"` and `\\` escape as in text values. An unquoted value runs to
  the next top-level `>>`.
- **Limits.** An inner locator may not start with a ref or `frame=`: error
  `` `has=e12`: an inner locator cannot be a ref or frame= `` with the hint
  "put the ref first: e12 >> …". Nesting is allowed up to depth 4; deeper is
  an error naming the step.
- **Scopes for `and=`/`or=`.** `query()` keeps the list of scopes the last
  finder step searched. `role=row >> role=button >> or=role=link` is the
  buttons or links inside each row. With no finder before them (`e12 >>
  or=…`), the scope is the chain's root.
- **Serialization.** `Step` gets `Has { steps }`, `HasNot { steps }`,
  `HasNotText { text }`, `And { steps }`, `Or { steps }`. `is_filter()` is
  true for all five. The JSON the bundle receives nests the inner steps, and
  `parse.rs`'s tests pin the JSON as they do for `has-text=`.
- **Errors in resolution** keep today's messages (ambiguous lists
  candidates, not found says which step matched nothing). A not-found whose
  last step is `has=` says so: "`.row` matched 12 elements; none has
  `role=button[name=Delete]`".

Update `docs/locators-and-refs.md`, `crates/chrome-cdp/src/help/locators.txt`
and the grammar doc comment at the top of `parse.rs`.

### `locator <loc>`: a ref's stable locator

Prints the locator a test should use for one element: what
`history --as-scenario` writes for a ref. The code is in
`crates/engine/src/locator/stable.rs` and `injected/src/stable.ts`.

```text
chrome-cdp locator <LOCATOR> [--all] [--tab T] [--strict-refs]
```

- Default output: one line, the most robust locator that matches the
  element alone, e.g. `role=button[name="Delete"] >> nth=1`. `-q` prints
  the same.
- `--all`: every candidate `stable.ts` tried, best first, each with how many
  elements it matches (`1` for a usable one):

  ```text
  testid=delete-12                          1
  role=button[name="Delete"] >> nth=1       1
  text=Delete                               3
  ```

- JSON: `{ "locator": "...", "candidates": [{ "locator", "count" }] }`
  (`candidates` only with `--all`).
- Honors the effective `testid_attribute` (config or `--testid-attribute`).
- Errors: the argument matches no element (NotFound) or several
  (Ambiguous, with candidates) as for actions. An element in an iframe:
  prefix a `frame=` locator when plan 13's `history-frame-locators` has
  landed; otherwise fail with "it is in a frame" and the hint `find`.
- New stable candidates may now use `has=` (e.g. `role=row >>
  has="text=Invoice 12" >> role=button[name=Delete]`) where that is the
  only unique form; add it at the end of `stable.ts`'s preference order.

### `convert`

`crates/chrome-cdp/src/convert/translate.rs` (`filter()`, around line 2328)
today turns `filter({ has })` into a TODO. Translate:

- `filter({ has: X })` → `>> has="X"`, `hasNot` → `has-not=`,
  `hasNotText` → `has-not-text=`, `hasText` stays `has-text=`.
- `a.or(b)` → `A >> or="B"`, `a.and(b)` → `A >> and="B"`, when `b` is a
  locator built from the same page or the same parent locator. Otherwise
  keep the TODO with a reason.

### Tests

- `parse.rs` unit tests: each kind parses, quoting, escapes, nesting depth,
  the ref/`frame=`-inside error, as the first step error.
- An engine test page `testdata/site/locators/filters.html`: a table of rows
  with and without Delete buttons, links and buttons mixed, nested lists.
  One test per kind against known counts, plus `and=`/`or=` scopes inside
  rows.
- CLI: `locator e12`, `locator --all`, `--json`; golden output.
- `convert`: snippets with each Playwright form, round-tripped through
  `e2e::scenario::load`.
- Parity: the two ❌ locator rows and the `browser_generate_locator` row
  become ✅.

## Milestone 14.2 — Paused timers

Backlog: `clock-fake-timers`.

Today (`crates/engine/src/emulate/clock.js`, `emulate/commands.rs`): `time
freeze` fakes `Date` and `Intl.DateTimeFormat`; timers and animations run in
real time; `time advance` moves `Date` and `performance.now()` and fires the
due `setTimeout`/`setInterval` callbacks once each. The timers are wrapped
by `wrapTimers` when the clock is first installed in a document.

Add Playwright's paused clock without changing `freeze`/`advance`:

```text
chrome-cdp time pause [TIME]     # freeze Date at TIME (default: now) and stop timers
chrome-cdp time run-for <DUR>    # move on by DUR, firing every due callback in order
chrome-cdp time pause-at <TIME>  # run forward to TIME, firing callbacks, then stay paused
chrome-cdp time resume           # timers run in real time again, from the paused time
```

- **Paused** means: `setTimeout`, `setInterval` and `requestAnimationFrame`
  callbacks are queued, not run. `performance.now()` stays still.
- **`run-for`** fires callbacks in due order, each interval tick
  separately (an interval of 100ms over `run-for 1s` fires 10 times), and
  `requestAnimationFrame` callbacks at 16ms steps. A callback that schedules
  another within the window runs too. It stops after 10,000 callbacks with
  a warning (a runaway loop), leaving the rest queued.
- **`pause-at`** is `run-for` up to TIME. A TIME in the past is an error.
- **`resume`** turns the paused clock back into a frozen one whose timers
  run in real time (as after `freeze`). `time reset` still removes it all.
- `time advance` while paused behaves like `run-for` but fires each due
  callback once (its v1 meaning); document the difference.
- `rAF` is wrapped only while paused, so animations are unaffected
  otherwise.
- Timers a page set before the clock was installed in that document cannot
  be paused. `time pause` on an already loaded page says so in a warning:
  "timers set before now keep running; reload for full control".
- Output: `time paused at 2026-01-31T09:30:00Z (timers stopped)`;
  `run-for` adds `ran 10 timers, 60 frames`. The session status line
  (`time_line`) shows `paused`. JSON adds `paused: bool` and, for
  `run-for`, `fired: { timers, frames }`.
- Errors: `run-for`/`pause-at` when not paused: FailedPrecondition with the
  hint `time pause`. A bad duration or time: the existing parse errors.
- The test runner's determinism (plan 11.6) is unchanged.

Tests: a fixture page with a countdown (interval), a debounce (timeout)
and a rAF animation counter. Engine tests for each command, interval
counts, nested timers, the runaway cap, `resume`, and a new tab opened
while paused. Parity: the ❌ `clock.runFor, pauseAt` row becomes ✅.

## Milestone 14.3 — Input and files

Backlog: `clipboard-rich-content`, `upload-directory-inputs`,
`downloads-dir-option`. Parity row: `keyboard.down`/`up`.

### `keyboard down|up`

The CLI has `press` (chords and sequences) and `mouse down|up|move|wheel`,
but no way to hold a key across commands.

```text
chrome-cdp keyboard down <KEY>     # e.g. Shift, Control, Mod, a
chrome-cdp keyboard up <KEY>
chrome-cdp keyboard up --all
```

- Key names come from the key table (`crates/engine/src/input/keys/`);
  unknown names get "did you mean" (`protocol::suggest::similar`).
- The daemon keeps the held keys per tab. Held modifiers apply to later
  `click`, `dblclick`, `press`, `type`, `mouse` and `drag` events (the
  `modifiers` bitmask of `Input.dispatch*Event`), as Playwright does.
- `keyboard up` of a key not held: a warning, not an error.
- Output: `holding Shift` / `released Shift (holding nothing)`. JSON:
  `{ "held": ["Shift"] }`. `status` lists held keys when there are any.
- Held keys stay held across navigations, as in Playwright, and are
  dropped when the tab closes. Document it.

### Clipboard: HTML and images

`clipboard read|write` handles text through `navigator.clipboard`
(`crates/engine/src/clipboard.rs`). Add:

```text
chrome-cdp clipboard read [--type text/plain|text/html|image/png] [-o FILE]
chrome-cdp clipboard write [TEXT] [--html HTML] [--file IMAGE.png]
```

- `read --type text/html` prints the HTML. `--type image/png` needs `-o`
  (a file, or `-` for stdout); without it, error "an image needs -o".
- `write --html` writes a `ClipboardItem` with `text/html` and, from TEXT
  or the HTML's text, `text/plain`. `--file` writes `image/png` (only PNG:
  what Chrome's async clipboard accepts; other extensions are an error).
- Permissions: grant `clipboardReadWrite` and `clipboardSanitizedWrite`
  (`Browser.grantPermissions`, both in the vendored protocol) as text does.
- A type that is not on the clipboard: NotFound listing the types that
  are ("the clipboard has text/plain, text/html").
- JSON: `read` adds `type` and, for images, `path` and `bytes`.

### Directory upload

`upload` rejects directories (`crates/engine/src/input/upload.rs:142`).

- If the input has `webkitdirectory` (check it in the page probe), accept
  one directory: its files, recursively, sorted, not following symlinks,
  are passed with `DOM.setFileInputFiles`. Over 10,000 files is an error.
- Check in the test that the page sees `webkitRelativePath` as
  `dir/sub/file.txt`. If Chrome does not set it through
  `setFileInputFiles`, record that in the decisions log and document it.
- A directory for an input without `webkitdirectory` keeps today's error,
  with the hint "this input takes files; pass the files". Files for a
  directory input: error "this input takes a directory".

### Downloads directory

- `open --downloads-dir <PATH>`: a session setting, like `--dialogs`. It
  sets `Settings::downloads_dir` (`crates/engine/src/tabs/mod.rs`) and
  calls `Browser.setDownloadBehavior` at once. Relative paths resolve
  against the CLI's working directory; the directory is created. It is kept
  when a CLI upgrade replaces the daemon (`keep_previous_options` in
  `crates/chrome-cdp/src/session.rs`).
- `downloads --clear`: deletes the files of finished downloads in the list
  and empties it. Downloads in progress are kept, with a warning naming
  them. Files chrome-cdp did not save are never touched.
- A daemon started by an upgrade starts with an empty list: `downloads`
  says "the list starts at <time> (the session's daemon was replaced)".

### Tests

Clipboard HTML and image round trips (write then read, and a page that
copies a `ClipboardItem`); `keyboard down Shift` then `click` on a fixture
that logs `shiftKey`, and `down a`/`up a` on a keydown/keyup logger; a
`webkitdirectory` fixture that lists `webkitRelativePath`;
`open --downloads-dir`, a download landing there, and `downloads --clear`.

## Milestone 14.4 — Extensions

Backlog: `extensions-command`.

### `extensions`

`open --extension` loads unpacked extensions at launch
(`crates/engine/src/extensions.rs`, flag `EXTENSIONS_FLAG` in
`crates/browser/src/launch/flags.rs`). Add the domain commands
`triggerAction`, `getStorageItems`, `setStorageItems`,
`removeStorageItems` and `clearStorageItems` to the `Extensions:` line of
`crates/cdp/protocol/domains.txt` and run `cargo xtask gen-cdp`.

```text
chrome-cdp extensions                          # list
chrome-cdp extensions load <DIR>
chrome-cdp extensions reload <ID|NAME>
chrome-cdp extensions uninstall <ID|NAME>
chrome-cdp extensions popup <ID|NAME>          # open its action popup
chrome-cdp extensions storage <ID|NAME> [--area local|sync|session|managed]
                              [get [KEY...] | set <JSON> | remove <KEY...> | clear]
```

- List: one line per extension: id, name, version, path. JSON: the existing
  `ExtensionInfo` list.
- `reload` is uninstall + `loadUnpacked` of the same path (an unpacked
  extension's id comes from its path, so it stays the same).
- `popup` calls `Extensions.triggerAction` on the active tab; the popup
  appears as a new tab or target, listed by `tabs`.
- `storage get` prints the items as JSON; `set` merges; `--area` defaults
  to `local`. `managed` is read-only: `set` on it is an error.
- A session launched without the extensions flag (no `--extension`):
  `load` fails with "this browser was started without extension
  debugging" and the hint `open --restart --extension DIR`. A connected
  browser (`connect`) gets the same error if Chrome refuses.
- An extension's service worker and pages: check that their console
  reaches `console`/`errors`. If plan 13 has landed, they are targets of
  the graph; if not, record the gap in a `.looper/tasks/` note.

### Tests

A fixture extension in `testdata/extensions/` with an action popup and a
storage write; list, reload after editing a file (a changed value shows), popup,
storage get/set/clear, uninstall.

## Milestone 14.5 — Sessions and waits

Backlog: `open-init-script`, `eval-in-auto-wait`, `wait-for-binding-call`,
`no-failed-requests-all-tabs`, `cdp-method-did-you-mean`.

- **`open --init-script <FILE>`** (repeatable) and
  **`--init-script-code <JS>`** (repeatable): added through
  `Settings::injections` before the first URL loads, as `open --device`
  does, and listed by `init-script list`. Kept across daemon upgrades
  like `--device`. A missing file fails before the browser starts.
- **Read commands wait for their element.** `eval --in`, `text`, `html`,
  `attr`, `value`, `box` and `count`'s single-element forms wait for the
  locator to match within `--timeout` (as actions and `screenshot <loc>`
  do), instead of failing at once. Decide once for all of them so they stay
  consistent; `count` without `--min` stays immediate (0 is an answer).
  The default timeout for reads is the same as for actions. Record the
  decision.
- **`wait --console <REGEX>`**: returns as soon as a console entry
  (any level, any source, including `binding` entries from `expose`)
  matches, logged after the command starts. **`wait --binding <NAME>`** is
  sugar for the `binding NAME(` entry. Output: the entry. With `--json`,
  the entry as `console --json` shows it. Timeouts list the last 5
  entries seen.
- **`expect --no-failed-requests --all-tabs`**: every tab of the session
  or context, each since it opened, sharing the code of
  `no_errors_in_all_tabs` in `crates/engine/src/expect/page.rs`.
- **`cdp` "did you mean".** Expose the list of generated domain and method
  names from the `cdp` crate. When Chrome answers "wasn't found"
  (code -32601, `crates/engine/src/raw.rs`, `cdp_error`), add
  "did you mean: Page.navigate" and a corrected hint using
  `protocol::suggest::similar`. Only suggest; never retry.

Tests: `open --init-script` with a script visible on the first page;
`text '#late'` on an element added after 500ms; `wait --binding` with a
page calling an exposed function after a click; `--all-tabs` with a
failed request in a second tab; `cdp Page.navigte`.

## Milestone 14.6 — Output files

Backlog: `har-pages-per-navigation`, `pdf-more-options` (templates and
scale only; streaming belongs to track 2), `screenshot-annotate-top-layer`.

- **HAR pages per navigation.** `crates/engine/src/observe/har.rs` makes one
  page per tab. Record each main-frame document request as a navigation
  boundary, with DOMContentLoaded and load times from the tab's lifecycle
  events, so a tab that navigated three times gives three pages (ids
  `t1_1`, `t1_2`, …; keep `t1` for a tab that never navigated so existing
  files look the same). `pageTimings.onContentLoad` and `onLoad` are ms
  since the page's start, `-1` when unknown. Each entry's `pageref` points
  at the page it belongs to. `har stop`'s output lists the pages.
- **PDF header, footer and scale.** `pdf --header <HTML|@FILE>`,
  `--footer <HTML|@FILE>` (turn on `displayHeaderFooter`; Chrome's
  classes `pageNumber`, `totalPages`, `date`, `title`, `url` work), and
  `--scale <0.1-2>`. Document that the templates need their own font size
  (Chrome's default is tiny) with an example.
- **`--annotate` above the top layer.** In
  `crates/engine/src/observe/screenshot.rs` and its overlay script: when the
  page has top-layer content (an open modal `<dialog>`, a popover, a
  fullscreen element), show the overlay as a manual popover
  (`popover="manual"`, `showPopover()`) so it sits above it. Nudge labels
  that overlap another label (down, then right, at most a label's size).
  Remove the overlay afterwards, as today.

Tests: a fixture that navigates twice then loads a resource; the HAR has
three pages with timings and the entries' `pageref`s. A PDF with a footer
containing the page number (check the text with the existing page counter
or by extracting it). `--annotate` on a page with an open modal: the
labels are in the image (compare a crop with the pixels helper in
`crates/engine/src/expect/pixels.rs`).

## Milestone 14.7 — The 🟡 rows

- **`expose` functions that return a value.** Today an exposed function
  logs its call and returns `Promise.resolve()`
  (`crates/engine/src/inject/expose.js`). Add `expose NAME --returns
  <JSON>` for a fixed answer and `--returns-seq <JSON-ARRAY>` for answers
  in call order (the last repeats). The answer is data, not code: logic
  that runs in the page is what `eval` and `init-script` are for. The binding's reply goes back through
  `Runtime.evaluate` in the calling context resolving the call's promise
  (keep a call id). Calls are still logged. Parity row: ✅ with a note that
  the answer is data, not a Node function.
- **WebSocket frames in `network`.** The network log already keeps a
  socket's last 20 frames and `network show ID` lists them
  (`crates/engine/src/observe/netlog.rs`, `network.rs`). Add
  `network frames <ID> [--follow] [--grep REGEX] [--dir sent|received]
  [--limit N]`: every kept frame, and new ones with `--follow`. Keep up to
  500 frames per socket (text cut at the current length), dropping the
  oldest with a count. `wait --ws <URL-PATTERN> [--frame REGEX]` waits for
  a frame on a matching socket. Update the stale parity rows (they say
  frames are only reachable through `cdp --listen`).
- **`expect-snapshot` templates.** A baseline line may use:
  - `*` as a whole name or text value: any value
    (`button "*"`);
  - `/regex/` as a name or text value (`heading /Order #\d+/`);
  - a line `...` : any number of sibling lines at that depth.

  Matching is line by line on the rendered `snapshot -i` text, so the
  template is still readable and diffable. `--update` never writes
  templates; it keeps `*`, `/…/` and `...` lines of the existing baseline
  that still match and rewrites the rest. A failure's diff shows the
  template's lines. Document it in `expect-snapshot --help` and
  `docs/guide.md`. Parity: `toMatchAriaSnapshot` becomes ✅.

Tests: an exposed function returning a fixed value and a sequence to page
code that awaits it; `network frames` on the WebSocket fixture with
`--grep` and `--follow --timeout`; `wait --ws`; template baselines that
match with changed dynamic text, fail on a missing button, and survive
`--update`.

## Acceptance criteria

- [x] `docs/playwright-parity.md` has no ❌ row; its "Today:" line and
      `tests/parity.rs` agree.
- [x] The 🟡 rows for `keyboard.down`/`up`, `exposeFunction`, WebSocket
      frames, `toMatchAriaSnapshot` and `browser_generate_locator` are ✅,
      in both parity docs.
- [x] `convert` translates `filter({ has, hasNot, hasNotText })`, `or()`
      and `and()` on the same-parent forms, and its tests load the output.
- [x] Every new command and flag has help with an example, a
      `docs/commands/` page, golden output and a `--json` form that only
      adds fields.
- [x] `cargo xtask release check` passes.
- [x] The track-3 backlog notes of plan 12 §6 are done and deleted from
      `.looper/tasks/`.

## Tests

| Area | Where |
|---|---|
| Locator grammar | `crates/engine/src/locator/parse.rs` unit tests; `crates/engine/tests/locator*.rs` with `testdata/site/locators/filters.html` |
| `locator` command, `convert` | `crates/chrome-cdp/tests/cli.rs`, `convert/tests.rs`, golden |
| Paused timers | engine test with a countdown, debounce and rAF fixture |
| Clipboard, keyboard, upload, downloads | engine tests; CLI tests for `open --downloads-dir` and `downloads --clear` |
| Extensions | `crates/chrome-cdp/tests/extensions.rs`, golden; `testdata/extensions/` fixture extension; tab routing unit tests |
| Sessions and waits | `crates/chrome-cdp/tests/wait.rs`, `session.rs`, `cli.rs` |
| HAR, PDF, annotate | `crates/chrome-cdp/tests/network.rs`, `screenshot.rs` |
| 🟡 rows | engine and CLI tests per item; `tests/parity.rs` for the counts |

## Decisions log

- **14.1, inner quoting.** Inside a quoted inner locator only `\"` and
  `\\` are unescaped; other backslashes stay, so an inner regex
  (`has="text=/a\s+b/"`) needs no doubling. Text values keep their rule.
- **14.1, `has=` candidates.** `stable.ts` writes them unquoted,
  `role=row >> has=text="Invoice 12" >> …`, using the first element in
  the container that holds its whole text (outside the target). They come
  after the scoped `has-text=` forms and before `nth=`.
- **14.1, `locator --all`.** It lists the element's own candidates (role,
  label, test id, placeholder, text) that find it, with counts, and the
  scoped, `has=` and `nth=` forms only when none of its own is usable:
  those are what `stable` tries. Listing every scoped combination on a
  page with a unique button was noise.
- **14.1, no stable locator.** `locator` on an element with nothing to
  find it by (or in an iframe no `frame=` can name alone) is `NotFound`
  with the reason, and the candidates in `details`. There is no
  FailedPrecondition kind, and adding a kind would change exit codes.
- **14.1, no auto-wait.** `locator` resolves its argument at once, as the
  other read commands do today; 14.5 decides whether reads wait, and
  `locator` should follow that decision.
- **14.1, `has=` not found.** The "`.row` matched 12 elements; none has
  …" message comes from the bundle counting the steps before a last
  `has=` when nothing matched; the count is also `details.before_has`.
  When those steps match nothing either, the usual "no element matches"
  message stays.
- **14.1, `convert` and `or()`/`and()`.** The parent is found on the
  translated locators: `a`'s steps before its last finder step. `b` must
  start with the same steps, and the rest becomes the inner locator.
  `page.getByRole('button').or(page.locator('.row').getByRole('link'))`
  is therefore `role=button >> or=".row >> role=link"`, while
  `row.getByRole('button').or(page.getByRole('link'))` stays a TODO.
- **14.2, errors.** `run-for`/`pause-at` on a clock that is not paused,
  and `pause-at` to an earlier time, are `BadArgs` (exit 2) with the hint
  `time pause`: there is no FailedPrecondition kind, and `time advance` on
  an unfrozen clock is `BadArgs` already.
- **14.2, `time pause` without TIME** keeps the frozen time when there is
  one (now otherwise), so `freeze T` then `pause` stops the clock at T.
  `time freeze` on a paused clock sets the time and lets timers run again.
- **14.2, `fired`** sums what each page, out-of-process iframe and worker
  session's main document fired. A same-process iframe runs its timers
  but is not counted: reading every frame's count costs a call per frame.
- **14.2, frames** run at 16ms steps from where the document paused, so a
  1s `run-for` fires 62 or 63 of them. An interval of 0 steps 1ms at a
  time while running, so it cannot stall the loop at one instant.
- **14.2, the "timers set before now" warning** comes from the clock
  marking a document it reached after it started loading (not
  `about:blank`), so it also shows for a page frozen after it loaded.
- **14.2, `resume`** hands queued animation frames to the browser, which
  gives them new ids; `cancelAnimationFrame` stays wrapped, mapping the
  old ids, until those frames have run.
- **14.2, `convert`.** Playwright's `clock.pauseAt(t)` also jumps a
  running clock there, while `time pause-at` needs a paused one, so it
  becomes `time pause T`; `runFor` becomes `time run-for`, `resume` `time
  resume`.
- **14.2, skill.** `chrome-cdp skill` has no `time` line, and paused
  timers are for tests of countdowns and animations, so it gets none.
- **14.3, clipboard permissions.** `clipboard` keeps granting
  `clipboard-read` and `clipboard-write` with `Browser.setPermission`, as
  for text, instead of `grantPermissions` with `clipboardReadWrite` and
  `clipboardSanitizedWrite`: the effect is the same, and `permissions`
  lists them. HTML and PNG are types Chrome sanitizes, so no unsanitized
  write is needed; `read --type text/html` gets Chrome's sanitized HTML.
- **14.3, the clipboard needs the front tab.** A page behind another
  reads an empty clipboard even with focus emulated (headless included),
  so `clipboard` brings its tab to the front for the call, and the active
  tab back after a `--tab` of another one. This fixed a v1 read that came
  back empty on a freshly opened tab.
- **14.3, clipboard output.** `write` of text alone keeps its v1 line;
  with `--html` or `--file` it names each type written. JSON `write`
  adds `types`; `read` adds `type`, and `path` and `bytes` for an image.
  `-o` with a text type is BadArgs (text is printed); an image without
  `-o` is BadArgs, as planned. The `-o -` path follows `screenshot`'s.
- **14.3, keyboard.** Held modifiers go with key, mouse, touch and drag
  events (`press` chords start with them, so `press a` with Shift held
  types `A`); `fill` ignores them, as Playwright's does. `type` types its
  text as given: its keys carry the held modifier bits, and type nothing
  with a modifier other than Shift held. `keyboard down` of a held key is
  a warning, not an auto-repeat. A chord (`Shift+a`) is BadArgs with the
  hint `press`. A click's `modifiers` in the output include held ones.
- **14.3, directory upload.** Chrome sets `webkitRelativePath` only when
  `DOM.setFileInputFiles` gets the directory itself (it lists it); given
  the files, a `webkitdirectory` input keeps what it had. So `upload`
  passes the directory, and walks it (sorted, no symlinks, at most 10,000
  files) only to check it and report the files, each with a new
  `relative_path` (`tree/sub/b.txt`, what the page sees); `data.directory`
  names it. Chrome's own listing may differ for symlinks. A directory with
  `--drop`, several paths with a directory, or an empty directory are
  BadArgs.
- **14.3, downloads.** `open --downloads-dir` prints `downloads: DIR`
  (JSON `downloads_dir`). `downloads --clear` deletes the file of every
  completed download in the list, wherever it was saved (an earlier
  `--downloads-dir` too), and forgets canceled ones. The "list starts at"
  line after a daemon upgrade is a warning (with the directory where the
  earlier files still are), and JSON has `since`; it shows only when the
  replaced daemon had a browser. Downloads kept in progress by `--clear`
  are not covered by a test: the fixture server cannot hold a response
  body open.
- **14.3, golden runs.** `GOLDEN_CASES=word,word` runs only the golden
  cases whose names hold one of the words, for iterating on a few.
- **14.4, reload.** `reload` calls `Extensions.loadUnpacked` on the
  extension's directory again instead of uninstall + load: Chrome reloads
  it with the same id and keeps its `chrome.storage`, as the Reload button
  does, where uninstalling deletes the storage.
- **14.4, extension debugging.** Chrome for Testing 154 answers
  `Extensions.*` without `--enable-unsafe-extension-debugging`, over the
  pipe and over a WebSocket, so `extensions load` works in any session,
  `--extension` or not. The planned "started without extension
  debugging" error (hint `open --restart --extension DIR`, or for
  `connect` the flag) is kept for a Chrome that refuses (method not found
  or "not available"); no Chrome at hand refuses, so it is not tested.
- **14.4, popup.** `Extensions.triggerAction` wants the tab's `tab`
  target, which Chrome does not link to its page target: `popup` attaches
  to each tab target in turn (the page's URL first) and auto-attaches to
  find the page. An extension id Chrome does not know crashes Chrome 154
  there, so the id always comes from `getExtensions`. The popup attaches
  as an `other` target that becomes a page; the tab manager attaches to
  such a target again, so it becomes a tab (what its scripts logged
  before that is lost). It is not made active: the output names it and
  `tab t2` switches. `popup` waits the whole `--timeout` for a manifest
  with a `default_popup`, 300ms without one, and 2s when the manifest
  cannot be read (a connected browser's files).
- **14.4, storage.** Chrome answers the storage commands only in a
  session of one of the extension's own targets ("Extension not found" on
  a page, "No associated browser context" on the browser). `storage` uses
  its service worker (kept running while chrome-cdp is attached), else a
  tab showing one of its pages, else a tab opened on its `manifest.json`
  for the call and closed after (it uses up a tab number). Every
  subcommand's JSON has the area's items after it (`get KEY…` only
  those); `set`, `remove` and `clear` print a line, `get` the JSON.
- **14.4, console.** Plan 13's graph attached extension service workers
  but, having no web origin, their console reached no tab. An extension's
  worker now belongs to `chrome-extension://<id>`: its console goes to
  every tab of its scope (an extension works on every page) and its
  requests only to the tabs of its own pages, so they hold no web page's
  `--idle` wait. Its pages (popup, options) are tabs with their own
  console.
- **14.4, status.** `open` and `status` keep listing what `--extension`
  loaded (`browser.extensions`), and a daemon upgrade relaunches with
  those only; `extensions` lists what the browser has loaded now.
- **14.4, `--tab`** is used by `popup` only; the other subcommands warn
  that it is ignored, as `permissions` does. `chrome-cdp skill` gets no
  line: extensions matter only to agents testing one.
- **14.4, tests** are CLI tests (`tests/extensions.rs`) and golden cases
  rather than engine tests: they go through the argument parsing (JSON,
  `--area`) and the session sandbox the extension fixture already uses.
  Golden ids depend on the fixture's path, so they read `<ext-id>`.
- **14.5, `open --init-script`.** The CLI reads the files (a missing one
  fails before a browser starts) and sends their sources, files first,
  then `--init-script-code`s. `open` adds only the scripts the session
  does not have yet (same source and file), so running it again adds
  nothing; its output lists them all (`init script: s1 main …`,
  `init_scripts` in JSON). The daemon's launch record merges them, so a
  daemon upgrade adds them again; it now also keeps an `open --device`
  given to a running browser, which it dropped before.
- **14.5, read waits.** Decided once for all of them: `text`, `html`,
  `attr`, `value`, `box` and `markdown` with a locator, `eval --in` and
  `locator` try again (every 50ms) while their locator matches nothing or
  names a frame not there yet, as actions do (`locator::until_found`);
  a timeout is their NotFound with `waited: 10s`. `count` answers at
  once. The read commands and `locator` default to the actions' 10s
  (they had 30s); `eval` keeps 30s, since its timeout bounds the script
  too. Each try bounds its calls by the whole timeout: a last try with a
  few milliseconds left would time out instead of finding nothing.
- **14.5, `wait --console`** counts entries logged since the previous
  command began, not since `wait` started: an agent runs `click` and
  then `wait`, and a call the click caused (while it settled) would be
  missed; `--request` and `--download` count the same way. It matches an
  entry's text or URL, as `console --grep` does; `--binding NAME` an
  entry of source `binding` and kind NAME. It takes no tab lock (an
  action can run meanwhile) and a dialog does not stop it. The text
  output is the entry as `console` prints it; JSON adds `console` to
  `wait`'s data.
- **14.5, `--no-failed-requests --all-tabs`.** `--origin self` is each
  tab's own page's origin. A worker's request is in each tab of its
  origin, so it counts once (by session and request id). `--all-tabs` no
  longer `requires` `--no-errors` in clap; `expect` checks that it comes
  with one of the two.
- **14.5, `cdp` did you mean.** The generated modules cover only
  `domains.txt`, so `cdp-gen` also writes `domains/names.rs`: every
  command of the protocol files. The hint adds `--target page` unless the
  suggestion is a `Browser.` or `Target.` method, and `…` when params were
  given.
- **14.5, `cdp --listen` did you mean (close-out).** `names.rs` also
  lists every event; a `--listen` name that is none and got no event is
  a warning (a `did you mean` note on a timeout), not an error: a newer
  Chrome may send events the pinned protocol lacks.
- **14.6, HAR page ids.** A tab with one page keeps `t1`, even when a
  navigation started it (`har start`, `goto`, `har stop`), so files of a
  single load look as before; `t1_1`, `t1_2`, … only when there are
  several. Requests of the document loaded before `har start` are a page
  of their own, first, named by that document's URL when the log still
  has its request, with timings -1. A navigation is a main-frame request
  of type Document (the netlog knows the page's target id, which is the
  main frame's id); a redirect's hops share its loader and stay one page.
  The times come from `Page.lifecycleEvent`, on the same monotonic clock
  as the request's `timestamp`. A back/forward-cache restore sends no
  request, so it starts no page.
- **14.6, `har stop` output.** The "from t1, t2" line stays; when a tab
  has several pages each gets a line (`t1_2 URL: 3 requests,
  DOMContentLoaded 40ms, load 52ms`). JSON adds `page_details` (id, tab,
  url, entries, `dom_content_loaded_ms`, `load_ms`) next to `pages`.
- **14.6, PDF templates.** Either of `--header`/`--footer` turns on
  `displayHeaderFooter` and the other is `<span></span>`: Chrome's default
  one (date and title, URL and page number) would surprise. A value
  starting with `@` is a file, read by the CLI. `--scale` is checked by
  clap (0.1 to 2); the text output says `scale 0.5` when it is not 1. The
  test reads the footer's text out of the PDF (Flate streams and the
  fonts' ToUnicode maps) rather than trusting the page count. `convert`
  translates `headerTemplate`/`footerTemplate` only with
  `displayHeaderFooter: true` (as Playwright prints them), and that flag
  alone is a TODO; it also adds `--margin` (Playwright's margins are 0
  unless given, `pdf`'s 1cm).
- **14.6, `--annotate` and the top layer.** The overlay is a manual
  popover only when the document has `:modal`, `:popover-open` or
  `:fullscreen` content (found with `querySelector`, so not inside shadow
  roots); a popover everywhere would show in page code's
  `:popover-open` queries for nothing. Labels are nudged within each
  frame's overlay: down by a label's height, then right by its width,
  then both, else left where they were.
- **14.6, `screenshot LOC` timeout.** The v1 backlog note: `screenshot`
  with a locator waits for its element, so it now defaults to the 10s of
  actions and reads; a whole-page capture keeps 30s.
- **14.7, expose answers.** A function exposed with answers numbers its
  calls in the page and sends `{"call":N,"args":[…]}`; the daemon
  evaluates the answer in the calling context, which resolves that
  call's promise. One without answers still resolves at once, so a
  daemon that misses a call cannot leave a page waiting. `--returns X`
  is the sequence `[X]`; the sequence is counted per function across the
  session's tabs and reloads, and `expose NAME` with other answers (or
  none) is a new binding that starts over. Workers still get no exposed
  functions (backlog note `expose-in-workers`). `convert` turns an arrow
  whose body is a JSON literal into `--returns`; other functions stay
  `expose NAME`.
- **14.7, frames.** Each frame has its number on the socket (`n`, a new
  field of `WebSocketFrame`) and the log's change count when it came,
  which `wait --ws` compares with a new per-command mark, as `--console`
  does. `network frames` defaults to `--limit 100`; `network show` keeps
  listing the last 20 frames (10 in text) and adds `earlier_frames` and
  a line naming `network frames ID`. `--follow` ends when the socket
  closes. A request that is no WebSocket is BadArgs with the hint
  `network --type websocket --since 0`, and so is the hint of a `wait
  --ws` timeout that saw no matching socket. `--frame` without `--ws` is
  BadArgs, as `--status` without `--response` is. HAR exports now carry
  up to 500 messages per socket.
- **14.7, binary frames (close-out).** A binary frame keeps its first 32
  bytes in hex (`hex`, a new optional field of `WebSocketFrame`), shown
  where a text frame's text is and matched by `network frames --grep` and
  `wait --ws --frame`. HAR exports still write binary messages with no
  data: 32 bytes are not the message.
- **14.7, templates.** A regex value is `/…/` or `/…/i` in place of the
  quoted value, found anywhere in it (as Playwright's are); `"*"` must
  still be a value (a line without a name does not match). One
  alignment (fewest template lines left out, page lines added, and lines
  changed at the same depth) decides the match and writes the page in
  the template's terms; the failure diff is from the template to that,
  and `--update` stores it. A changed line counts as one change, so a
  `...` does not swallow the page's version of a line the template no
  longer matches. A regex that does not compile is BadArgs naming the
  line. `convert` keeps `toMatchAriaSnapshot` a TODO: Playwright's YAML
  (`[level=1]`, `- text: …`) is not the `snapshot -i` format.
- **14.7, template values (close-out).** A field's `value=` takes `"*"`
  and `/regex/` as the name does; a link's href takes only `→ "*"`: an
  href is not quoted, and `→ /docs/` is a path, not a regex.
- **14.7, skill.** `chrome-cdp skill` gets no line for these: its list
  is the hot path, and WebSocket frames, exposed answers and templates
  are for tests that need them.
