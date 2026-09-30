# Plan 16 — Seeing more of the page

Status: IN PROGRESS (16.1 done)
Depends on: 04 (snapshot, refs, locators), 05 (actionability pipeline, action report), 06 (screenshots), 07 (viewport emulation), 09 (`expect`, baselines), 11 (contexts, stable locators), 13 (the target graph, for out-of-process iframes)

## Goal

The snapshot tells an agent what is on the page. It does not say why an
action would fail, what is wrong with how the page looks, or what changed
between two pages. On a big page it is also too long. This plan adds six
tools for that. Each one prints compact text an agent can act on, not a
picture for a human.

| Command | Question it answers |
|---|---|
| `why <loc>` | Why can't I click this? (without waiting 10 s for a timeout) |
| `layout` | What looks broken: covered, clipped, off-screen, too small, unreadable? |
| `snapshot --budget` / `--fold` | Show me a big page in N tokens, and let me open the parts I need |
| `forms`, `fill-form` | What does this form need, and fill it in one step |
| `compare` | What differs between these two pages (local vs production, before vs after)? |
| `explore` | What pages and states does this app have, and which ones are broken? |

Playwright has none of these. Its closest is the log of a timed-out
action.

## Rules for every command in this plan

- Plan 08 §2–§4 apply: text by default, `--json` with a schema,
  `--max-chars`, errors with a hint, and `-q` where one value is the
  answer.
- Elements are named as the snapshot names them (`button "Save" [e12]`)
  and get refs from the tab's registry (plan 04, 4.2), so the agent's
  next command can use them. An element without a role is named by its
  tag and first class (`div.cookie-banner [e88]`), as the obscured error
  does.
- They work in every frame. Same-process iframes come with the main
  frame; out-of-process iframes go through plan 13's target graph, with
  boxes moved into the tab's viewport the way the actionability pipeline
  does (`input/actionability/geometry.rs`).
- A report (`why`, `layout`, `compare`, `explore`) exits 0 whatever it
  finds. `compare` and `explore` take `--exit-code` to exit 1 when they
  found a difference or a problem, like `git diff --exit-code`. Only
  `expect --layout` is an assertion.
- The default output of existing commands does not change. `snapshot`
  without the new flags prints what it printed in v1, byte for byte:
  `expect-snapshot` baselines depend on it.
- Page code runs in chrome-cdp's isolated world (`injected/src/`, one
  new module per milestone), as the locator and actionability code do.

## Milestone 16.1 — `why <loc>`

```
chrome-cdp why <LOCATOR> [--for click|hover|fill|type|check|select|upload|drag] [--no-scroll] [--strict-refs]
```

`why` runs the actionability pipeline (plan 05, 5.1) once, now, and
prints the verdict of every check. It does not act, wait or retry. The
pipeline stops at the first failed check; `why` runs them all, so the
agent sees every problem at once.

`--for` picks the checks the action needs (default `click`):

| `--for` | enabled | editable | hit test |
|---|---|---|---|
| click, check, drag | yes | — | yes |
| hover | — | — | yes |
| fill, type | yes | yes | — |
| select | yes | — | — |
| upload | — (file inputs are waited for, not checked) | — | — |

It scrolls the element into view as the action would, because the hit
test needs a point in the viewport. `--no-scroll` leaves the page where
it is and marks the hit test `skipped` when the element is out of view.

Output:

```
why e12 button "Pay" (for click)
✓ resolve    1 match
✓ visible    120×32 at 40,610
✗ stable     moved 14px between two frames (animation "pulse" on it)
✓ enabled
✓ in view    after scrolling main [e3] by 420px
✗ hit test   covered by div.cookie-banner [e88]; its button "Accept" [e90] likely dismisses it
→ click would wait until the timeout: not stable; covered by div.cookie-banner [e88]
```

The last line says what the action would do: `click would act now`,
`click would wait until the timeout: <checks>`, or
`click would fail at once: <reason>` (a gone ref, `Ambiguous`, an
element that can never take text).

What each check reports when it fails. The page-side code finds the
*cause*, not only the symptom:

| Check | Reports |
|---|---|
| resolve | no match (with the locator engine's own near-miss notes); or `Ambiguous` with the first 5 candidates and refs |
| visible | `display: none` on it or on which ancestor (with a ref); `visibility: hidden` (which element sets it); `opacity: 0` (which ancestor); a zero-size box; `content-visibility: hidden` |
| stable | the distance moved between two animation frames, and the running animations and transitions on it or its ancestors (`getAnimations()`: name or transitioned property) |
| enabled | which applies: its `disabled`; the disabled `<fieldset>` it is in (ref, and not in its first `<legend>`); `aria-disabled="true"` on it or which ancestor; `inert` on it or which ancestor; a modal `<dialog>` open elsewhere (ref) that makes it inert |
| editable | read-only (`readonly`, `aria-readonly`), or it cannot take text (its role) |
| in view | which scroll container it is in, and how far it scrolled; or that it lies outside any scrollable area (at a negative offset, or clipped by an `overflow: hidden` ancestor, with a ref) |
| hit test | what is at the point (the obscured error's element and dismiss button, 5.1's code), or `pointer-events: none` on it or which ancestor |

Errors: only usage errors (a bad locator, an unknown `--for`) and
browser errors. A locator that matches nothing is a verdict, not an
error, so the agent gets the near-miss notes in one call.

`--json`: `{locator, for, element?, checks: [{name, status: "ok"|"failed"|"skipped", why?, element?}], verdict: {acts_now, waits: [names], fails: reason?}}`.
`element` is `{ref, role, name}`.

Hints: the `Timeout`, `Obscured` and `NotEnabled` errors of actions add
`why <loc> --for <action>` as a note (not the main hint, which stays the
fix), so an agent that hits one knows the tool exists.

Implementation: the checks live in `injected/src/actionable.ts` and
`input/actionability/`. Add an "explain" mode that runs every check and
returns the causes, rather than a second copy of the rules. The pipeline
and `why` must agree; a test runs both on every fixture case.

Fixture: `testdata/site/why/cases.html`, one element per case with an
`id`, and `why/*.expected` golden outputs: a button under a cookie
banner; one in a disabled fieldset; one with `aria-disabled` on an
ancestor; one inside an `inert` region; one behind an open modal
`<dialog>`; one under a `pointer-events: none` parent; one with an
endless CSS animation; one hidden by `display: none` on its grandparent;
one at `opacity: 0`; one clipped out of an `overflow: hidden` box; a
read-only input (`--for fill`); one far below the fold (passes after
scrolling; `skipped` with `--no-scroll`); one in a cross-site iframe; one
that is fine. Two buttons with the same name, for `Ambiguous`.

## Milestone 16.2 — `layout`

```
chrome-cdp layout [LOCATOR] [--viewports LIST] [--only KINDS] [--all] [--full]
                  [--min-target PX] [--stretch-text PCT]
chrome-cdp expect --layout [LOCATOR] --baseline NAME [--update] [layout's flags]
```

`layout` lints what the page looks like, in the active tab, or inside
the element `LOCATOR` names. It reads geometry and computed styles in
the page (`injected/src/layout.ts`, `getClientRects` and
`getComputedStyle`, across shadow roots). Only rendered elements count:
a box of at least 1×1 px, no `display: none`, `visibility: visible`, and
no ancestor at `opacity: 0`. "Interactive" means the elements the
snapshot gives refs to by default (plan 04, 4.1: interactive roles,
clickables, and not iframes or scrollers).

### Finding kinds

Each has a severity: **error** (shown by default) or **info** (shown
with `--all`).

| Kind | Severity | Rule |
|---|---|---|
| `covered` | error | An interactive element in the viewport whose centre point hit-tests (`elementsFromPoint`) to an element that is neither it, inside it, nor one of its `<label>`s. Names the top element as the obscured error does. Only in the viewport; `--full` scrolls the page one viewport at a time and restores the scroll. |
| `overlap` | error | Two interactive elements, neither inside the other, whose boxes intersect by more than 4 px² and by at least 10% of the smaller box's area. |
| `clipped` | error | An element with its own text whose content overflows (`scrollWidth > clientWidth + 1` or `scrollHeight > clientHeight + 1`), where the overflowing axis is `hidden` or `clip`, with no `text-overflow: ellipsis` and no `-webkit-line-clamp`. Also text cut off by an ancestor like that: the text's range box is not inside the ancestor's padding box. Reports how much is shown (`212px of 318px`). |
| `truncated` | info | The same, but cut on purpose with an ellipsis or line clamp. Reports the full text (bounded). |
| `overflow-x` | error | The document scrolls sideways: `scrollWidth > innerWidth + 1`. Names up to 5 of the outermost elements whose right edge is past the viewport. |
| `offscreen` | error | An interactive element that no scroll can reach: its box is entirely at a negative x or y of the document, or past its scroll size. Visually hidden elements (≤ 1 px, or `clip`/`clip-path` to nothing: the `.sr-only` pattern) and skip links (a link to `#…` that comes into view on focus) do not count. |
| `small-target` | error | An interactive element smaller than 24×24 CSS px (WCAG 2.5.8), except: a link inside a line of text (its block has other text); or when a 24 px circle centred on it overlaps no other target and no other target's circle (the spacing exception). `--min-target PX` changes 24. |
| `contrast` | error | Rendered text with contrast below 4.5:1, or 3:1 for large text (at least 24 px, or 18.66 px and weight 700 or more). Measured on pixels, see below. |

`--only covered,clipped` limits the kinds.

**Contrast on pixels.** The a11y rule of `expect --a11y` (plan 09) reads
declared colours, so it misses text on images and gradients. `layout`
takes two screenshots of the area it checks (CSS pixels): one as is, one
with the text of every checked element made transparent by an injected
style. For each text box, the second shot holds only background. Contrast
is computed between the text's computed colour (with its opacity) and the
10th and 90th percentile background luminance in the box, and the worse
one counts. At most 2,000 text elements; past that, a warning says how
many were skipped. The style is removed in a `finally`, and after the
shots the page is as it was.

**Viewports.** `--viewports mobile,tablet,desktop` (or sizes, `390x844`)
runs the whole check at each size: `mobile` is 390×844 at a pixel ratio
of 3 with touch and mobile mode, `tablet` is 820×1180 at 2 with touch,
`desktop` is 1280×800 at 1. Before each, it sets the viewport as
`viewport` does (plan 07) and waits as `wait --settle` does. After the
last, it restores the tab's emulation exactly as it was. Findings carry
their viewport.

**Stretched text.** `--stretch-text 40` makes every visible text node
40% longer before checking, as a translation into German or Finnish
would: it repeats the node's own words until it is long enough, so lines
wrap as real text does. It leaves form values and `<script>`/`<style>`
alone. It then reports only the findings that the unstretched run does
not have, marked `(stretched 40%)`. The original text comes back in a
`finally` (kept in a `WeakMap`); if the daemon dies first, a reload
restores the page.

### Output

```
layout t1 https://shop.test/cart (1280x800): 5 errors, 2 info (--all)
✗ covered       button "Checkout" [e12] under div.cookie-banner [e88] at 640,760
✗ clipped       heading "Wireless noise-cancelling headphones" [e21]: 212px of 318px shown (overflow: hidden on div.card [e20])
✗ overflow-x    the page is 1412px wide at 1280px: table "Orders" [e30] ends at 1412px
✗ small-target  link "×" [e40]: 16×16px, needs 24×24
✗ contrast      text "Free shipping" [e41]: 2.9:1 on #7fb1ff–#a8c9ff, needs 4.5:1
```

With several viewports, one block per viewport, and a first line that
counts findings per viewport. Findings are sorted by kind (the table's
order), then by position (top to bottom, left to right), so the output
is deterministic.

`--json`: `{url, viewports: [{name, width, height, findings: [...]}]}`,
each finding `{kind, severity, key, element: {ref, role, name}, box: {x, y, width, height}, details: {...}}`
(`details` per kind: the covering element, the shown and full sizes, the
ratio and the colours, …).

### `expect --layout`

`expect --layout --baseline NAME` fails (exit 1, `ExpectFailed`) when
there are error findings that the baseline does not list. The baseline
is a text file with one finding key per line, `#` for comments, stored
where `expect-snapshot` stores its baselines (`expect/baseline.rs`: the
project's `.chrome-cdp/`, or `e2e/snapshots/` under the test runner),
as `layout/<NAME>.txt`. `--update` writes the current findings to it. A
missing baseline fails and hints at `--update`, as `expect-snapshot`
does. Found findings that the baseline lists are fine, and listed ones no
longer found are named in a note so the file can be cleaned up.

A finding's **key** must survive a re-run and small page changes:
`<kind> <viewport> <stable locator>`. The stable locator comes from
`injected/src/stable.ts` (the code `history --as-scenario` uses), for
example `clipped desktop role=heading[name="Wireless noise-cancelling headphones"]`.
`covered` and `overlap` add the second element's locator.

`expect --layout` retries like the other matchers until `--timeout`, so
a page still settling does not fail it.

Fixture: `testdata/site/layout/`, one page per kind with a known number
of findings, plus a clean page with none. `layout/cards.html` has a card
whose title fits at 1280 px and clips at 390 px (for `--viewports`) and
one that clips only with `--stretch-text 40`. `layout/contrast.html` has
text on a gradient and on a photo, with declared colours that pass and
rendered pixels that fail, and the reverse. `layout/frames.html` puts a
clipped label in a cross-site iframe.

## Milestone 16.3 — Big pages in `snapshot`

```
chrome-cdp snapshot [...] --budget TOKENS
chrome-cdp snapshot [...] --fold
chrome-cdp snapshot --expand REGION [REGION...] [--budget TOKENS]
```

Two new ways to shorten a snapshot. Both are off by default; the config
key `snapshot_budget` (plan 08's config files) gives a session default.

**Folding repeats (`--fold`, and always with `--budget`).** Four or more
siblings in a row with the same *shape* (the roles of their subtree, in
order, to a depth of 4, ignoring names, values and states) print the
first one in full, then one line:

```
- list "Products"
  - listitem
    - link "Mug" [e40] → /products/1
    - text "12 €"
    - button "Add to cart" [e41]
  - … 59 more listitem like the one above [region r5]
```

The folded nodes still get refs in the registry, so `find`, locators and
refs from an earlier snapshot keep working. v1's long-list collapse
(past 30 siblings of one role) still applies after folding.

**A token budget (`--budget N`).** The snapshot is rendered in full; if
its estimated size is over N tokens, whole regions are folded into stubs
until it fits:

```
- main
  - heading "Orders" (h1)
  - table "Orders" [region r4: 412 nodes, 96 refs, ~5,100 tokens]
  - navigation "Pages"
    - button "Next" [e7]
```

- Tokens are estimated as characters ÷ 4 (a fixed rule, so the output is
  deterministic; the token suite measures the real count).
- A region is a node with 20 or more nodes below it. Landmarks, lists,
  tables, grids, trees, listboxes, menus, dialogs, forms and groups
  fold first; then any node, largest first.
- It never folds the node that has focus or its ancestors, the `--scope`
  root, the page's first `h1` and the landmarks at depth 1 (their stub
  shows their name instead).
- If the stubs alone do not fit, it folds deeper ancestors too; then
  `--max-chars` applies as before.

**Regions.** A region id (`r4`) names a node the way a ref does: it is
kept per tab, stable across snapshots for the same node, and cleared by
a cross-document navigation (the ref registry's rules, plan 04, 4.2).
`snapshot --expand r4` prints the region's subtree, like `--scope`, with
the same budget and folding inside it. An unknown or stale region is
`RefStale` with a hint to take a new snapshot. Regions are only for
`--expand`; other commands do not take them.

`--diff` compares unfolded trees and does not fold; a diff is already
short. `-i` folds after it has filtered.

`--json`: folded nodes are `{role, folded: {region, nodes, refs, tokens, like?}}`
in place of their children (new optional fields in the snapshot schema).

The token suite (`cargo xtask bench tokens`) gets rows for
`snapshot --fold` and `snapshot --budget 2000` on every corpus page, so
the saving is counted.

Fixture: `testdata/site/snapshot/cards.html` (a 60-card product grid,
with `.expected` files for `--fold` and `--budget 1500`), and `huge.html`
from plan 04 with `--budget 4000`. A test checks that a ref of a folded
card works with `click`, and that `--expand` of every stub, put together,
contains every node of the full snapshot.

## Milestone 16.4 — Forms

```
chrome-cdp forms [LOCATOR]
chrome-cdp fill-form [LOCATOR] (--json JSON | --file FILE) [--submit] [--timeout T]
```

**`forms`** lists the page's forms (or the one that `LOCATOR` names or is
inside) as units: every `<form>`, `role=form` and `role=search`, and one
`(fields outside a form)` group for controls in none. For each field: the
accessible name, the kind, required, disabled, read-only, the value
(never a password's; a file input's file names), and the validation
message when it is invalid (`validationMessage` for constraint
validation, else the text `aria-errormessage` or `aria-describedby`
points to when `aria-invalid` is true). Selects and listboxes show their
first 10 options. Radios are grouped by `name` (or `radiogroup`).
Custom widgets count by role (`combobox`, `listbox`, `checkbox`,
`switch`, `radiogroup`, `textbox`, `slider`, `spinbutton`).

```
form "Checkout" [e10] → POST /api/orders (5 fields, 2 required empty)
  textbox "Email" [e11] (email) required value=""
  textbox "Card number" [e12] required invalid: "Please enter a valid card number."
  combobox "Country" [e13] value="Norway" options: Norway, Sweden, Denmark, Finland
  checkbox "Save card" [e14] unchecked
  radiogroup "Shipping": radio "Standard" [e15] checked, radio "Express" [e16]
  file "Receipt" [e17] accept=.pdf
  button "Pay" [e18] (submit) disabled
```

**`fill-form`** fills a form from a JSON object in one step. Keys are
matched to fields in this order: the accessible name (ignoring case,
spaces at the ends, a trailing `*` or `:`), then the `name` attribute,
then the `id`, then the placeholder, then a unique substring of the
name. Values pick the action:

| Field | Value | Action |
|---|---|---|
| text-like, date, colour, range | string or number | `fill` |
| `<select>`, listbox | string, or an array for multiple | `select` |
| checkbox, switch | `true` / `false` | `check` / `uncheck` |
| radio group | the label (or value) of the choice | `check` on that radio |
| file input | a path, or an array of paths (relative to the CLI's directory) | `upload` |
| combobox that is not a `<select>` | string | `fill`, then click the option with that name if a listbox opens |

Fields are filled in document order, not the JSON's order. After each
field, keys not yet matched are matched again, because fields often
appear only after an earlier choice (a region after a country). Each
field goes through the same code as the single command (`Engine::act`),
so actionability, waits and the action report apply.

Keys that match no field, or more than one, fail the command after the
others are filled: `NotFound` (exit 1) naming each key, with the form's
field names as "did you mean" candidates. A value of the wrong type for
its field is `BadArgs` before anything is filled. Without `LOCATOR`, a
page with more than one form is `Ambiguous`, with the candidates.
`--submit` then clicks the form's default button (the first submit
button), or presses Enter in the last field if there is none.

Output: one line per field (`filled textbox "Email" [e11]`), then one
action report for the whole form (plan 05, 5.5), covering everything
since the first field. `history --as-scenario` records `fill-form` as one
line with its JSON, and the scenario linter accepts it.

`--json`: `forms` is `{forms: [{element, method?, action?, fields: [...]}]}`;
`fill-form` is `{filled: [{key, element, action}], unmatched: [...]}`.

Fixture: `testdata/site/forms/checkout.html` (every field kind above, a
region select that appears after a country is chosen, a custom
combobox, a field that is invalid until filled) and
`forms/two-forms.html` (login and newsletter), with the values each
test fills and the expected state after.

## Milestone 16.5 — `compare`

```
chrome-cdp compare <A> <B> [-i] [--scope LOCATOR] [--visual] [--threshold T]
                   [--viewport WxH] [--exit-code]
```

`compare` diffs two pages: their snapshots, and with `--visual` their
pixels. `A` and `B` are each:

- a URL: opened in a new tab of the session's current context, compared,
  then closed;
- `tab:<id>`: an open tab of this session, as it is;
- `session:<name>`: the active tab of another session (its daemon is
  asked for the same data over its socket);
- a path to a `.json` file written by `snapshot --json` (snapshot only;
  `--visual` needs a live page).

**The snapshot diff.** Two different pages share no backend node ids, so
`compare` matches nodes by structure, not by ref: siblings are aligned
by the longest common sequence of `(role, name)`, then nodes left over
with the same role at the same place count as changed (`~`). Refs,
`[region]` stubs and ref numbers are left out. When the two origins
differ (local against production), hrefs on B's origin are rewritten to
A's before comparing, so only real differences show. The output reuses
`snapshot --diff`'s form (`+`, `-`, `~`, with the ancestors that place
each change). The pure diff and render code must be callable where both
trees are; move it out of the engine's snapshot module if the CLI needs
it for `session:` sides.

**The visual diff (`--visual`).** Both pages are shot at the same
viewport (`--viewport`, else A's; a URL side gets that viewport; a tab
side with another size gets a warning and only the shared area is
compared) with `expect-screenshot`'s pixel comparison
(`expect/pixels.rs`) and `--threshold` (default 0.1). Changed pixels are
grouped into regions: 8×8 px cells with any change, joined when they
touch, then boxes within 16 px of each other merged. It prints the 10
largest, each named by the smallest element of B whose box holds the
region's centre (with a ref when B is live). A diff image is saved in
the session's `shots/` directory: B, faded, with the changed pixels in
red and the regions boxed.

```
compare http://localhost:5173/cart ↔ https://shop.example.com/cart
snapshot: 3 changes
- main
  ~ heading "Your cart (2)" → "Your cart (3)" (h1)
  + button "Apply coupon"
  - link "Continue shopping" → /
visual: 2 regions differ (1280x800, threshold 0.1)
  1. 40,210 600×48 around heading "Your cart (3)"
  2. 900,720 200×40 around button "Apply coupon" [e9]
  image: /run/user/1000/chrome-cdp/default/shots/compare-3.png
```

`--exit-code` exits 1 when anything differs. `--json`:
`{a, b, snapshot: {changes, diff: [...]}, visual?: {width, height, changed_pixels, regions: [{x, y, width, height, element?}], image}}`.

Fixture: `testdata/site/compare/a.html` and `b.html`, served on both
fixture origins, differing in exactly: one heading's text, an added
button, a removed link, a moved block (a remove and an add, not a
change), and a colour that only `--visual` sees. `compare a b` without
`--visual` reports four changes, and `--visual` five regions or fewer
that cover the five differences.

## Milestone 16.6 — `explore`

```
chrome-cdp explore [URL] [--max-states N] [--max-actions N] [--depth N] [--time DURATION]
                   [--click-buttons] [--submit-forms] [--allow-destructive]
                   [--origin ORIGIN]... [--include PATTERN]... [--exclude PATTERN]...
                   [--layout] [--screenshots] [-o DIR] [--exit-code]
```

`explore` maps an app: it starts at `URL` (else the active tab's URL),
follows what can be followed, and reports every page and state it found,
how to reach each, and what was wrong on each.

**Where it runs.** In a new browser context (plan 11, 11.4) with a copy
of the active context's cookies and storage, routes, mocks and
emulation, so a logged-in or mocked app looks the same, and the agent's
tabs are left alone. The context is closed at the end, also on Ctrl-C
or a timeout.

**States.** A state is the normalised URL (no fragment, query keys
sorted, `utm_*` dropped) plus a hash of the page's skeleton: the roles of
`snapshot -i`, and the names of headings, landmarks and dialogs. So an
open modal at the same URL is a new state, and a list with other data is
not. Each state gets an id `s1`, `s2`, … in the order found.

**The walk.** Breadth-first from the start, so the result does not depend
on timing. On each state, the candidates are taken in document order:

- links (`a[href]`) to an allowed origin (the start's, plus `--origin`)
  that `--include`/`--exclude` (globs, as `wait --url` takes) allow;
- with `--click-buttons`: buttons, tabs, menu items, and controls with
  `aria-expanded` or `aria-haspopup`.

Each candidate is clicked (not `goto`, so client-side routing runs) with
the normal action code, its report tells whether it navigated, and the
state it leads to is recorded. To get back to a state for its next
candidate, `explore` loads its URL directly when that gives the same
skeleton (checked the first time), and otherwise replays the path from
the start.

**What it never does by default:**

- submit a form: submit buttons, and Enter in a field (`--submit-forms`
  allows it);
- type into a field or upload a file (never, even with flags);
- click a destructive-looking control, even with `--click-buttons`: a
  name matching `delete|remove|destroy|log ?out|sign ?out|unsubscribe|deactivate|close account|cancel (order|subscription|account)|pay|purchase|buy|place order|checkout|confirm|reset|archive|drop`
  (ignoring case; `--allow-destructive` allows it);
- leave the allowed origins: other links are listed as external, not
  opened;
- follow `mailto:`, `tel:`, `javascript:` or `download` links, or links to
  files (`.pdf`, `.zip`, …);
- accept a dialog: `alert` is accepted, `confirm` and `prompt` are
  dismissed, `beforeunload` is accepted.

A new tab that a click opens is recorded as a candidate (when its URL is
allowed) and closed.

**The budget.** It stops at the first of: `--max-states` (default 30),
`--max-actions` (default 200, counting replayed steps), `--depth`
(default 3 actions from the start) and `--time` (default 2m). Each
action has the usual `--timeout`. The output says which limit stopped
it and what was left unvisited.

**What it records per state:** the title, the status of its document,
the console errors and uncaught exceptions and failed requests (the
action report's rules) since it was reached, and with `--layout` the
error findings of `layout` at the current viewport.

```
explore http://localhost:5173/: 14 states, 63 actions, 41s (stopped: all visited)
s1  /                 "Shop"                ok
s2  /products         "Products"            ok       ← s1 click link "Products"
s3  /products/1       "Mug"                 1 error  ← s2 click link "Mug"
      ! console error: TypeError: price is undefined (src/Product.tsx:12)
s4  /cart             "Cart" + dialog "Coupon"  ok   ← s3 click button "Add coupon"
...
broken: /about 404 (from s1 link "About")
not followed: 3 external links (github.com, x.com), 1 destructive (button "Delete account" on s7), 2 form submits (s5; --submit-forms)
```

**`-o DIR`** writes `map.json` (the `--json` data), `states/<id>.cdp` (the
path to each state as a scenario: `goto <start>`, then each step with a
stable locator from `stable.ts`, so `run states/s4.cdp` gets there), and
with `--screenshots` `states/<id>.png`. The directory must be empty or
not exist.

`--exit-code` exits 1 when any state had an error, a failed request, a
broken link or (with `--layout`) a finding. `--json`:
`{start, stopped_by, budget, states: [{id, url, title, status, depth, path: [step], errors, failed_requests, layout?}], edges: [{from, to, step}], broken: [...], skipped: {external, destructive, forms, files}}`.

Fixture: `testdata/site/explore/`, a small app with a known map: six
pages, one only reachable through a client-side route, a modal state, a
link loop, a "Delete account" button, a form, an external link, a link
to a 404, and a page that logs a console error. `explore/expected.txt` is
the output with and without `--click-buttons`. A test runs
`explore -o` and then `run` on every `states/*.cdp`, checking each lands
on its state.

## Acceptance criteria

- [x] `why` and the actionability pipeline agree on every case of
      `why/cases.html`: `why` says "would act now" exactly when the
      action passes at once.
- [x] `why` names the cause (the element and the property) for every
      failed check on the fixture, not only the check.
- [ ] `layout` finds each fixture's known findings and nothing on the
      clean page, at every viewport of `--viewports mobile,tablet,desktop`,
      and leaves the page's emulation, scroll and text as they were.
- [ ] `contrast` fails the gradient and photo cases that `expect --a11y`
      passes.
- [ ] `expect --layout` passes with an up-to-date baseline and fails on a
      new finding; finding keys survive a reload and a re-render.
- [ ] `snapshot` without the new flags prints exactly what v1 printed on
      every corpus page (the plan 04 `.expected` files are unchanged).
- [ ] `--budget N` output is at most N estimated tokens on every corpus
      page where the stubs fit, and every ref and region in it works.
- [ ] `fill-form` fills `forms/checkout.html` in one call, including the
      field that appears after the country, and the page's own state
      shows every value.
- [ ] `compare` reports exactly the known differences of `compare/a.html`
      and `b.html`, across the two fixture origins.
- [ ] `explore` maps `testdata/site/explore/` to its expected map, never
      clicks the destructive button or submits the form without the
      flags, and its `states/*.cdp` files replay.
- [ ] `explore` maps `examples/e2e-shop` (run with its mock backend)
      fully within its default budget.
- [ ] Every new command and flag has help with an example, a
      `docs/commands/` page, a golden schema, and a line in
      `chrome-cdp skill` where an agent should reach for it (`why`,
      `forms`/`fill-form`, `snapshot --budget`); the skill stays within
      its token budget.
- [ ] The token suite has rows for `snapshot --fold` and
      `--budget 2000`, and the saving is recorded in the decisions log
      (`cargo xtask bench tokens` is a count, not a timing, so it is run).

## Tests

- **why**: one golden output per case (`why/*.expected`), each
  `--for` value on a case where it matters, `--no-scroll`, a case in a
  cross-site iframe, `Ambiguous` and no match; a table test that runs
  `why` and then the action with a short timeout on every case and
  checks that they agree.
- **layout**: a golden output per fixture page and kind; `--only`,
  `--all`, `--full` on a long page; `--viewports` (findings differ per
  size; emulation restored, checked with `viewport` and `emulate`
  afterwards); `--stretch-text` (only new findings; text restored,
  checked with `text`); contrast on declared-pass/rendered-fail and the
  reverse; `expect --layout` with a missing, matching, outdated and
  `--update`d baseline, and under the test runner (baseline in
  `e2e/snapshots/`).
- **snapshot**: `--fold` and `--budget` goldens on `cards.html` and
  `huge.html`; region ids stable across two snapshots and cleared by a
  navigation; `--expand` of each stub; refs of folded nodes act;
  `--diff` and `-i` with the new flags; every existing snapshot golden
  unchanged.
- **forms**: `forms` golden on both fixtures; `fill-form` for each value
  type, a dependent field, an unmatched key (partial fill, then
  `NotFound`), a wrong value type (`BadArgs`, nothing filled), two forms
  without a locator (`Ambiguous`), `--submit` with and without a submit
  button; `history --as-scenario` after `fill-form`.
- **compare**: URL/URL, tab/URL, `session:`, `.json` file, `-i`,
  `--scope`, `--visual` regions and the diff image's size,
  `--exit-code`, viewports that differ.
- **explore**: the fixture map with and without `--click-buttons`,
  `--submit-forms` and `--allow-destructive`; each budget limit stopping
  it (`stopped_by`); `--include`/`--exclude`; `-o` layout and replay of
  every state; the agent's tabs and context untouched afterwards; the
  e2e-shop map.

## Decisions log

_(append here)_
- 16.1: `enabled` also fails on an inert element (`inert` on it or an
  ancestor, or a modal `<dialog>` open elsewhere) in the pipeline itself,
  not only in `why`: an inert element takes no clicks, focus or text, and
  `fill` would otherwise pass the pipeline and `why` would disagree.
  `expect enabled` keeps its ARIA meaning and does not count inertness.
- 16.1: the plan's table says `drag` needs its element enabled; the code
  did not check it. Now what `drag` drags must be enabled (as for
  `click`); where it drops need not be.
- 16.1: `--for upload` on a file input (or its label) checks only
  `disabled`, which is all `upload` waits for; on another element, which
  `upload` clicks to open the file chooser, it runs `click`'s checks.
- 16.1: the pipeline fails at once on an element that can never take
  text only when the checks before `editable` pass; so does `why`'s
  verdict (a hidden checkbox with `--for fill` waits).
- 16.1: `--no-scroll` with the element out of view marks `in view` and
  `hit test` `skipped`; the verdict is then `click would act now, unless
  in view or hit test fails`.
- 16.1: `element` in the JSON (and each check's) is `{ref, role, name,
  tag, class?}`: an element without a role is named by its tag and first
  class. Checks also carry `dismiss` (hit test), `candidates`
  (`Ambiguous`) and `notes` (the locator's notes on no match).
- 16.1: the locator engine has no "did you mean" for a locator that
  matches nothing; `why` shows the notes its `NotFound` error has (such as
  `text=` matching visible elements only).
- 16.1: the `why/*.expected` goldens mask box sizes and places, scroll
  distances and how far an animation moved, which depend on fonts and
  timing. The agreement test tells "fails at once" from "waits" by half
  the action's timeout: the pipeline gives up one retry before its
  deadline.
