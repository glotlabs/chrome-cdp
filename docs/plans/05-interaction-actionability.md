# Plan 05 — Interaction & actionability

Status: DONE (5.1-5.5; the 50-run flakiness gate waits for CI runners, `press Mod+A` is only verified on Linux, and the obscured ref has a known gap when a frame loads twice)
Depends on: 04

## Goal

Every input action behaves like a real user's: trusted events, the correct
sequence, and auto-waiting until the element is actually actionable. Each
action gives a short report of what changed, so the agent rarely needs to
re-snapshot.

## Milestone 5.1 — Actionability pipeline

For `<loc>`, run the pipeline below. Retry the whole pipeline until it
succeeds or the timeout (default 10 s) expires. The final error names the
failing check and the evidence.

1. **Resolve** the locator (plan 04) → exactly one node. Refs resolve
   immediately. Other locators wait for attachment.
2. **Attached & visible:** a non-empty box, not `visibility:hidden`, and
   opacity > 0 (except for inputs styled invisibly behind labels; see 5.3).
3. **Stable:** the bounding box is unchanged across two consecutive animation
   frames.
4. **Enabled:** not `disabled` or `aria-disabled`, and not inside a disabled
   fieldset. Checked only for actions that need it.
5. **Editable:** only for `fill`/`type`. Not `readonly`.
6. **Scroll into view:** `DOM.scrollIntoViewIfNeeded`, then re-measure with
   `DOM.getContentQuads`. Pick the centre of the first quad that is within the
   viewport.
7. **Hit test:** `DOM.getNodeForLocation(x, y)` must be the target or a
   descendant (shadow-inclusive). If another element covers the point, report
   `Obscured by <div class="cookie-banner"> [e88]`. The error gives a ref to the
   obstruction, so the agent can dismiss it in one step. `--force` skips checks
   2–7.

## Milestone 5.2 — Mouse & pointer

- `click <loc> [--button left|right|middle] [--count 2] [--modifiers Shift,Meta]
  [--position x,y] [--force] [--no-wait-after]`. It dispatches
  `Input.dispatchMouseEvent` moved → pressed → released at the hit point,
  with a correct `clickCount` and `buttons` bitmask. `dblclick` is sugar for it.
- `hover <loc>`, and `mouse move|down|up|wheel x y` (low-level escape hatch).
- `drag <from-loc> <to-loc> [--steps 10]`:
  1. Try pointer events with intermediate moves.
  2. If the page uses HTML5 DnD (`draggable=true`), use
     `Input.setInterceptDrags` + `Input.dispatchDragEvent` with the intercepted
     `dragData`. This is the only reliable way under CDP.
- `scroll [loc] [--by dx,dy] [--to top|bottom|x,y]` via
  `Input.dispatchMouseEvent type=mouseWheel` at the element (a real wheel, so
  infinite scroll triggers). Reports the new scroll position and whether the
  end was reached.
- `tap <loc>` (touch) uses `Input.dispatchTouchEvent` when touch emulation is
  on (plan 07).

## Milestone 5.3 — Keyboard & text

- A key table for the US layout: `key`, `code`, `keyCode`, `text`, and
  `location` for every named key (`Enter`, `Tab`, `ArrowDown`, `F1`–`F12`,
  numpad, …). Generate it from Playwright's (Apache-2.0) `USKeyboardLayout`
  data and credit it in `NOTICE`.
- `press <keys>`: `Enter`, `Control+A`, `Shift+Tab`, a sequence
  `"Control+A Backspace"`. `Mod` means Meta on macOS targets and Control
  elsewhere; this is resolved by the *browser* platform, not the host.
  The edit commands (`selectAll`, `copy`, `paste`) are sent via the
  `commands` field, because macOS headless ignores synthetic Meta shortcuts.
- `type <text> [--delay ms] [--in <loc>]`: per-character `keyDown`/`char`/`keyUp`
  for layout characters, and `Input.insertText` for others (emoji, CJK).
  Triggers every key listener.
- `fill <loc> <text>`: the fast path, which is the one agents should use.
  1. Focus the element.
  2. Select the existing content: `select()` for inputs/textarea, a Range for
     contenteditable.
  3. Call `Input.insertText` (one trusted `input` event).
  4. Dispatch `change` on blur, only if the value changed.

  Special input types: `date`/`time`/`color`/`range` set `.value` and dispatch
  `input` + `change`. `fill <loc> ""` clears.
  - `select <loc> <value|label>...`: native `<select>` by value, then by label.
    For ARIA comboboxes/listboxes, return a helpful error pointing to
    `click` + `click option`.
  - `check`/`uncheck <loc>`: click, only if the state differs, then verify
    the state changed. Handles custom checkboxes (`role=checkbox`) and visually
    hidden native inputs by clicking the associated `<label>`.
- `focus <loc>`, `blur <loc>`.

## Milestone 5.4 — Files & clipboard

- `upload <loc> <file...>`: `DOM.setFileInputFiles` with absolute paths. If
  `<loc>` is a button that opens a file chooser (not an `<input>`), use
  `Page.setInterceptFileChooserDialog`, click, then fulfill the chooser.
  `--clear` empties the input.
- `clipboard read|write <text>`: grant permissions (plan 07) and use
  `navigator.clipboard` in the page.

## Milestone 5.5 — Post-action report

After each action (unless `--no-wait-after`):

1. Wait for the consequences. If the action caused a navigation, wait for
   `load` + settle (plan 03). Otherwise wait for a brief settle (network idle
   or DOM quiet, max 1.5 s).
2. Print a compact report:

   ```
   clicked e6 button "Sign in"
   → navigated to https://acme.test/dashboard "Dashboard" (200)
   ! console error: TypeError: x is undefined (app.js:12)
   ! opened tab t2 https://acme.test/help
   ```

   Lines appear only when something actually happened: navigation, a new tab,
   a dialog, a download, a new console error, failed requests (≥ 400 or
   network error) on the page's own origin, or a focused element changing to a
   different form.
3. `--snap` appends `snapshot --diff -i` (the changed interactive elements
   only). The session default is configurable
   (`chrome-cdp config set snap-after-action diff`). Plan 10's benchmark decides
   the recommended default.

## Tests

The fixture pages cover: a button under a cookie banner (obscured error), a
button that moves during animation (stability), a button disabled until a
timer, an element scrolled far off screen, a nested scroll container, shadow
DOM buttons, an iframe button, HTML5 DnD and pointer-based DnD (a sortable
list), every input type, a React controlled input (`fill` must update React
state; assert via the rendered echo), contenteditable (a rich-text editor), a
native select, a custom listbox, a custom checkbox, a hidden file input behind
a button, keyboard shortcut listeners (assert `key`/`code`/modifiers), an IME-
style `insertText` receiver, infinite scroll, and double-click and
right-click handlers.

- Each action has: a success test, a test per failed actionability check (with
  golden-file error messages), and a `--force` test.
- Post-action report tests cover each line type.

## Acceptance criteria

- [ ] Every fixture interaction passes 50 consecutive runs on each CI runner
      (flakiness gate).
- [x] `fill` on the React, Vue and Svelte fixture inputs updates framework
      state.
- [ ] The obscured-element error always includes the obstructing ref.
- [ ] `press Mod+A` behaves correctly on both macOS and Linux runners.

## Decisions log

_(append here)_
- 5.1: The pipeline lives in `engine::input::actionability`: `Engine::actionable` (for 5.2's actions, with a `retry` closure for hints) and the public `Engine::action_point`, which only tests use until 5.2's `click` exists. So the golden error files are engine-level for now (`crates/engine/tests/actionability/`), and the tests check each point by pressing the mouse there with raw `Input.dispatchMouseEvent` calls.
- 5.1: Retries wait 0, 20, 100, 100, then every 500 ms (Playwright's schedule). A ref that is gone, `Ambiguous`, and an element that can never take text (a checkbox, a `<div>`) fail at once; only "no match yet" and failed checks are retried.
- 5.1: Error kinds: disabled and read-only are `NotEnabled`, covered is `Obscured` (both exit 1); not visible, not stable and outside the viewport are `Timeout` (exit 3) naming the check in the message and `details.check`; a locator that never matched keeps its `NotFound` error. The `waited:` note shows the timeout, not the elapsed time, so messages are deterministic.
- 5.1: The stable check (the same box on two consecutive animation frames, with a 100 ms fallback where frames do not run) runs in the page before scrolling, in the plan's order. `DOM.scrollIntoViewIfNeeded` scrolls instantly even under `scroll-behavior: smooth` (the scroll fixture uses it), so the point measured after it holds.
- 5.1: `DOM.getNodeForLocation` takes document coordinates, not viewport ones: the hit test adds the local root's scroll offset (found by the far-below-the-fold fixture).
- 5.1: Quads and hit tests are in the element's local root (its out-of-process iframe, or the main frame); Chrome hit-tests through same-process iframes itself. At each out-of-process boundary the point moves by the `<iframe>`'s content box, and the hit test in the parent must find that `<iframe>`.
- 5.1: Scrolling inside an out-of-process iframe scrolls its parent later, asynchronously, and the browser routes input into the iframe by the last frame it drew. So the pipeline scrolls each `<iframe>` to the element's box in its parent itself, and waits for two animation frames in the tab before it returns the point; without that, clicks into a scrolled cross-origin iframe missed 3 times in 4.
- 5.1: `--force` skips checks 2-5 and the hit test, but still scrolls the element into view and aims at its first quad (the plan said to skip 2-7): a click at an off-screen point lands on nothing. A `display: none` element has no point even with `--force`.
- 5.1: The obscured error names the highest ancestor of the hit element that does not contain the target (the cookie banner, not the `<p>` in it), with a ref, and a button in it whose name looks like a dismissal (Accept, OK, Close, ...; else its first button) as the hint's `click`. When the target itself does not take pointer events (`pointer-events: none`), the error says so and names the element the click lands on instead.
- 5.1: The opacity exception for inputs styled invisibly behind labels is left to 5.3, which clicks the `<label>` for those.
- 5.2: Every action goes through one daemon method (`act`) and `Engine::act`, with an `Action` enum (`click`, `hover`, `tap`, `mouse`, `drag`, `scroll`); 5.3-5.5 add variants. Actions wait 10 s by default, like `wait`, and one deadline covers everything an action waits for (a drag aims four times).
- 5.2: `--no-wait-after` is not added yet: 5.5 owns the post-action wait and report, and a flag that does nothing would mislead (as plan 08 decided for global flags). Actions return once their events are sent, with the tabs, dialogs and downloads `report_events` already lists.
- 5.2: Each action prints one line naming its element as the snapshot does, giving it a ref if it had none (`clicked e6 button "Sign in"`); `-q` prints the ref. `hover` skips the enabled check (a disabled control still shows its tooltip); `drag` and `scroll` use the same checks as `hover`.
- 5.2: `--modifiers` only sets the events' `modifiers` bits (`shiftKey`, … and link dispositions); no key events are sent for them. `Mod` is not accepted yet: 5.3 resolves it by the browser's platform.
- 5.2: `--position x,y` counts from the top-left corner of the element's first quad (its border box), and the point must be in the viewport; the hit test runs there.
- 5.2: `mouse down|up` take an optional `x y` to move to first, `mouse wheel dx dy` turns the wheel where the mouse is, and `mouse up` without `--button` releases the button held. Each tab keeps where chrome-cdp put the mouse and the buttons it holds, so moves start there and a press can span commands.
- 5.2: `drag` is one flow, not "pointer first, then HTML5": drags are intercepted (`Input.setInterceptDrags`) while the mouse presses on `from` and moves to `to` in `--steps`. If the page starts an HTML5 drag, its `dragData` comes back and the drop is finished with `Input.dispatchDragEvent` (enter, over, drop), then the mouse is released; otherwise the release ends a pointer drag. It waits up to 1 s for drag data only when `from` is draggable. Both elements are aimed at twice; if aiming at one moved the other, they do not fit in the viewport together and the drag fails (`Obscured`, the closest kind) instead of dropping in the wrong place.
- 5.2: `scroll` reports on the container a wheel over the element scrolls first (the element or its nearest scrollable ancestor, else the page). Without a locator the wheel turns in the middle of the viewport. `--by` is one wheel event; `--to` sends wheels until it gets there, stops moving, or after 10 wheels. After each wheel it waits in the page until the offset moved and held for two frames, or did not move for 10 frames. `scroll <loc>` alone scrolls the element into view (the pipeline does that when it aims).
- 5.2: `tap` needs touch emulation (`navigator.maxTouchPoints > 0`), else it fails with `BadArgs` and hints at `click`, or at `cdp --target page Emulation.setTouchEmulationEnabled` until plan 07's `emulate` exists. It sends a touch start and end; the browser makes the click, which a cancelled `touchend` stops.
- 5.2: Fixture logs sit below the elements they log for: a log that grows above a button moves it between a touch and the click the browser makes of it, which looked like taps not clicking in headless Chrome.
- 5.3: The key table is generated from playwright-core 1.64's bundled `usKeyboardLayout` and `macEditingCommands` by `bench/playwright/gen-keys.cjs` into `engine/src/input/keys/table.rs` (credited in `NOTICE`). Keys are named by code, by the character they type, or by name; multi-letter names also match in any case, with synonyms agents type (`Esc`, `Ctrl`, `Cmd`, `Up`, `PgDn`). Unlike Playwright, Shift with a character key types its shifted character (`Shift+1` is `!`), and a shifted numpad key sends its shifted key code.
- 5.3: `Mod` is resolved by the browser's user agent (`Browser.getVersion`, "Macintosh"), asked once per browser, not by the page's `navigator.platform` (emulation can change that; the editing behaviour follows the real OS). `click --modifiers Mod` resolves it the same way. On macOS every key down carries Playwright's whole editing-command table (not only selectAll, copy and paste; synthetic Backspace and arrows need theirs too); elsewhere none are sent, so `Control+V` does not paste twice. Only verified on Linux: no macOS runner here.
- 5.3: `press` and `type` take `--in <loc>`: the pipeline (enabled; editable for `type`) without the hit test, then `DOM.focus`, which works in out-of-process iframes where a page's `focus()` may not. An element that cannot take focus fails (`NotEnabled`) instead of sending keys elsewhere. Without `--in`, keys go where focus is and the output names that element (its `<iframe>` when focus is in one).
- 5.3: `type` presses a key for each character the layout has and inserts runs of the others with one `Input.insertText` (an emoji sequence stays whole); with `--delay` each character is its own event. `type --in` on an element it had to focus puts the caret at the end, as a click after the text would. A `--delay` whose total outlasts the timeout fails at once (`BadArgs`, with a `--timeout` that fits).
- 5.3: Keyboard and form actions skip the hit test (`Checks::no_hit_test`), as Playwright's `fill` does: a floating label over a field does not stop typing into it. `check` keeps it (it clicks), and the hit test now counts a hit on a `<label>` of the element as reaching it (Playwright's follow-label).
- 5.3: `fill` focuses, selects the content (`select()`, or a Range for contenteditable) and inserts the text; `""` presses Delete on the selection. Step 4 needs no code: the browser fires `change` on blur after a trusted edit (tested). Date, time, datetime-local, month, week, colour and range inputs get their value set with synthetic `input` and `change`; a value they refuse fails with `BadArgs` and an example to fill. When the field holds something else afterwards (a mask, `maxlength`), a warning says what (only the length for passwords, whose value `--json` leaves out).
- 5.3: `fill` on a `<select>` fails at once (the editable check) and says to use `select`; with `--force` the error's hint runs it.
- 5.3: `select` matches each value by option value, then label, then label in any case, and waits until they match (options may load late); a disabled option is waited for, then `NotEnabled`. It sets the options and fires `input` and `change` like Playwright. A non-`<select>` fails with `NotEnabled` (as 5.1's "cannot take text" does); for a combobox or listbox the hint clicks it, then its option.
- 5.3: `check`/`uncheck` read the state (native, `aria-checked`, or the control of a `<label>` locator), do nothing and say so when it already is, and refuse to uncheck a radio (`BadArgs`). A native input that is visually hidden (transparent, clipped or at most 1px, as with `.sr-only`) is clicked through its first visible label, which gets a ref; that settles 5.1's opacity exception without relaxing the visible check. After the click the state must change within 1 s (frameworks render late), else `NotEnabled` ("did not check it", and "it is disabled" when so).
- 5.3: `focus` and `blur` run the pipeline without the enabled check and the hit test. `blur` on an element without focus is a no-op that says so.
- 5.3: The framework fixture (`testdata/site/actions/frameworks.html`) uses vendored builds (`actions/vendor/`): React 18.3.1 (the last with UMD builds), Vue 3.5.43, and a Svelte 5.57.1 component compiled and bundled with esbuild; `fill`, `check` and `select` update their state.
- 5.4: `upload` sends a file input (or a `<label>`'s control) its files with `DOM.setFileInputFiles` without the visible check, since upload widgets hide their input; it waits while the input is disabled (`NotEnabled` after the timeout; `--force` sets it anyway). Anything else goes through the pipeline like `click`, is clicked with `Page.setInterceptFileChooserDialog` on in its frame's session (so choosers in out-of-process iframes work), and the chooser's input gets the files. A click that opens no chooser within 3 s fails (`NotEnabled`) and hints at `upload 'css=input[type=file]'`; a chooser without an input (`showOpenFilePicker`) cannot be filled over CDP and says so.
- 5.4: More files than a single-file input or chooser takes is `BadArgs`. A file the input's `accept` rules out (by extension, or by MIME type for common extensions) is uploaded with a warning: the page may drop it silently, and the browser does not check. The CLI makes paths absolute (the daemon has another working directory) and checks each is an existing file before it starts anything.
- 5.4: `DOM.setFileInputFiles` ignores an empty list (Chrome 154), so `--clear` sets an empty `FileList` in the page and fires `input` and `change`, as Playwright does; clearing an input with no files says so and fires nothing. Setting the files an input already holds fires no events, as a user's same choice does not.
- 5.4: `clipboard` is its own daemon method, not an `Action`: it takes no element. It runs `navigator.clipboard` in the isolated world of the tab's main frame, after `Browser.setPermission` grants `clipboard-read` and `clipboard-write` to the page's origin (kept for the session, so the page's own paste button works afterwards; `setPermission` rather than `grantPermissions`, which would reset the origin's other permissions) and with `Emulation.setFocusEmulationEnabled` on for the call only (headless pages are not focused, and the API refuses unfocused ones). A page without an origin (about:blank) or not in a secure context fails with `NotEnabled`. Plan 07's permissions and emulation should reuse and not undo these.
- 5.4: `clipboard read` prints the text as is (bounded by `--max-chars`), with a warning when the clipboard holds none; `clipboard write` takes `-` for stdin and drops one trailing newline, like `fill`. Only text: images and HTML are left for later.
- 5.5: The report is built in `engine::input::report` inside `Engine::act`, while the tab's lock is held: before the input it watches the main frame (`navigate::watch::Watch`) and marks the tab's network and console logs and where focus is; after it, it waits and adds the lines. So every action (and `mouse`) reports the same way, and `--json` gets the lines as `events` (new kinds `navigated`, `console_error`, `request_failed`, `focus_moved`, next to `tab_opened`, `dialog` and `download`). The lines go on stdout after the action's own; the warnings `report_events` gave actions before (popups, dialogs, downloads) became `!` lines there.
- 5.5: "Network idle or DOM quiet" is read as both, as the navigation settle does: a click that sends a request has a quiet DOM until the answer comes. The brief settle counts only requests started since the input (a page's polling from before is not the action's) and wants 150 ms of quiet (not the 500 ms of `wait --idle`, which every click would pay), capped at 1.5 s.
- 5.5: A navigation is followed when the main frame starts to leave its document (`Page.frameStartedNavigating` other than same-document) during the action or its brief settle: to the load event (up to `--timeout`, at most 20 s from the input's end, so the CLI's wait for the daemon still covers it), then the settle `goto` runs. It is not an error when the page does not load in time: the line says `(still loading)` and a warning points at `wait --load`. A navigation that stops without committing (a download, a 204) reports nothing. `pushState` and `#fragment` navigations are reported with `(same document)`, since single-page apps route that way.
- 5.5: Console errors and failed requests needed logs before plan 06: each tab keeps its last 100 `console.error`/failed `console.assert` calls and uncaught exceptions (`engine::console`, fed by the `Runtime` events every session already gets) and its last 256 failed requests (status 400 or more, or `loadingFailed` unless canceled) in `network::Network`. Plan 06's buffers can grow from the same events. The report lists five of each and counts the rest. A failed request counts when its origin is the page's before or after the action; the document of a reported navigation is left out (its status is on the `→` line).
- 5.5: "A focused element changing to a different form" is taken literally: focus moved to an element whose form (its form owner, or the `<form>` around it; none counts as one) differs from where focus was before, and the action did not act on it (it, inside it, or the control of a `<label>` it clicked; `__ccdp.acted` records what the action named). So `press Tab` inside a form, a click that focuses its own field, and a `fill` report nothing, while a step that moves focus to the next form does. Focus in an iframe is not followed, and not after a new document.
- 5.5: `--snap`, `--no-snap` and `--no-wait-after` are flags of the action commands (with `CHROME_CDP_SNAP` and `CHROME_CDP_NO_WAIT_AFTER`), not of every command as plan 08 §4 lists, which would add flags that do nothing elsewhere. The session default is the config key `snap_after_action = "diff" | "off"` (plan 08's config files), not a `config set` command, which plan 08 does not have. `--snap` is `snapshot --diff -i` of the tab (the `snapshot` command's read, while the action still holds the tab), its data under `data.snapshot` (the `Acted<T>` schema wrapper). With it, action output can grow with the page, so actions took `--max-chars` (20,000 by default, like `snapshot`) and left the fixed-size list.
- 5.5: `--no-wait-after` skips the waits but still reports what already happened (a navigation that started, as `(still loading)`), so it only makes the output less complete, never wrong. A dialog that waits for an answer, opened during the waits, still fails the action with `DialogOpen` (exit 5), as one opened by the input itself does.
