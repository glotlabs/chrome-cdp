# Recording a test

`record` turns what happens in the page into `.cdp` lines, whoever does
it: a person clicking in a headed window, an agent running chrome-cdp
commands, or anything else that sends the page input. It listens to the
page itself, not to chrome-cdp's commands, so a raw `cdp Input.*` event
records like a real click.

```
chrome-cdp open --headed http://localhost:5173/signup
chrome-cdp record start --name "signs up"
# … fill the form in the window, choose a plan, press Sign up …
chrome-cdp record stop -o e2e/scenarios/signup.cdp
```

```
@test "signs up"
  goto /signup
  fill 'role=textbox[name="Name"]' Ada
  press Tab
  fill 'role=textbox[name="Email"]' ada@example.com
  # a password field: the test reads it from the PASSWORD secret, never the value
  fill 'role=textbox[name="Password"]' "${secret:PASSWORD}"
  select 'role=combobox[name="Size"]' m
  check 'role=radio[name="Pro"]'
  fill 'frame=card >> role=textbox[name="Card number"]' 4242
  click 'frame=card >> role=button[name="Pay"]'
  click 'role=button[name="Sign up"]'
  # add expect steps: the recorder does not guess what to check
```

The recorder guesses no assertions: add the `expect` steps that say what
the test is for, then run it with `chrome-cdp test`.

## Start, status and stop

`record start [--name NAME]` starts recording in every tab and iframe of
the session (or of `--context`), from the active tab. Its page is where
the test starts: the first step is a `goto` to it. Starting again drops
the earlier recording. `record status` says how many steps there are so
far (`-q`: just the number).

`record stop` prints the test, or with `-o FILE` appends it to FILE (made
if missing, a blank line before it in a file that has tests), so several
recordings can go into one file. URLs under the project's `base_url`
become paths, as in `history --as-scenario`.

## What becomes a step

| In the page | Step |
|---|---|
| a click on a checkbox, radio or switch | `check LOC` / `uncheck LOC` |
| another click; a double click | `click LOC` / `dblclick LOC` |
| typing into a field | one `fill LOC "value"`, written when another event comes |
| typing into a password field | `fill LOC "${secret:PASSWORD}"` and a comment; never the value |
| a choice in a `<select>` | `select LOC value…` |
| a file chosen | `upload LOC NAME` and a `# TODO: path` comment |
| Enter, Escape, Tab, other keys and shortcuts that type nothing | `press KEY` (`--in LOC` when focus is on another element than the step before acted on) |
| a URL typed or loaded by the browser, back, forward, reload | `goto URL`, `back`, `forward`, `reload` |
| a dialog someone answered | `dialog accept [TEXT]` / `dialog dismiss` |
| a new tab, a switch to another one | `tab new [URL]` / `tab tN` |

Each element is named when it is acted on, before the page reacts, by the
most robust locator that finds it alone: the same locators `history
--as-scenario` writes (role and name, label, test id, placeholder, text,
then those inside a named container, `>> nth=` as a last resort). A test
id uses `testid=…` for the configured `testid_attribute`. An element in
an iframe gets the iframe's `frame=` step first (its name, else a part of
its URL only it has), out-of-process iframes too. An element no locator
finds alone becomes a comment and a warning.

Some events make no step, because the replay repeats them by itself:

- A click on a field that is then typed into: `fill` focuses it.
- A navigation the page asked for (a link followed, a form sent, a
  redirect): the step that caused it is recorded.
- A popup's first page: the click that opened it is recorded; the steps
  in the popup start with `tab tN`.
- A dialog the session's policy answered (an `alert`, by default): the
  replay's policy answers it too. A `confirm` or `prompt` answered by
  someone, with `chrome-cdp dialog` or in the window, is a `dialog` step.
  In `test` and `run`, a step that a dialog interrupts (exit 5,
  `DialogOpen`) passes when the next step answers it.

## Limits

- Tab ids are the session's: a test that switches tabs with `tab tN`
  replays only where the tabs get the same ids. `tab new URL` makes the
  new tab active, so a test that only opens tabs replays anywhere.
- An uploaded file's path is not known to the page: replace the name the
  `upload` step has with a path (relative to where `test` runs).
- Chrome does not say who moved through the history: a page's own
  `history.back()` (an app's Back button) is recorded as `back` after the
  click that ran it, and the replay would go back twice. Delete the
  `back`.
- What happens inside closed shadow roots is recorded on their host.
