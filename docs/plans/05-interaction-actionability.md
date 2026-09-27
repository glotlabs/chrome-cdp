# Plan 05 — Interaction & actionability

Status: IN PROGRESS (5.1 done)
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
- [ ] `fill` on the React, Vue and Svelte fixture inputs updates framework
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
