# Plan 20 — Overlays

Status: NOT STARTED
Depends on: 05 (the actionability pipeline, the obscured error's
`covering()`), 08 (output, errors), 13 (the target graph, for consent
banners in out-of-process iframes), 16 (`why`, `layout`'s `covered`,
`explore`, stable locators)

## Goal

Real sites open with something in the way: a cookie banner, a consent
wall that locks the scroll, a newsletter modal, an "install our app"
strip. Today an agent finds out when a click fails with `Obscured`,
clicks the hinted button, and tries again, once per overlay. This plan
adds two commands that deal with all of them in one step:

| Command | Question it answers |
|---|---|
| `overlays` | What is over the page right now, and what would close each one? |
| `close-overlays` | Close them, and tell me exactly what was clicked |

Like `forms` and `fill-form`, the first is a report and the second acts.

The commands are a heuristic and say so. They never guess silently:
every overlay they close is closed by an ordinary action that the output
names with a stable locator, so a test can carry the explicit `click`
instead of `close-overlays`.

Read plan 05 §5.1 (the obscured error), plan 16 §16.1-§16.2 (`why`,
`layout`) and §16.6 (`explore`) first. Code:
`crates/engine/src/injected/src/actionable.ts` (`covering()`, `DISMISS`,
`named()`), `injected/src/layout.ts` (`covered`), `injected/src/dom.ts`
(shadow-aware `parentOf`), `injected/src/stable.ts`,
`crates/engine/src/input/` (actions and the action report),
`crates/engine/src/frames/` (the target graph).

## Rules for every milestone

- Plan 12 §7 and plan 16's "Rules for every command" apply: plan 08
  §2–§4 output, refs from the tab's registry, every frame, page code in
  the isolated world (one new module, `injected/src/overlays.ts`).
- **Act like a user.** Closing is done with the normal action code
  (actionability, the action report, history). The only exception is
  `--hide`, which is opt-in and reported as "hidden, not closed".
- **Never by default:** follow a link that navigates (a "Privacy policy"
  link is not a close button), submit a form, type, accept consent, or
  click a control `explore` calls destructive or one that signs up or
  pays (`subscribe|sign ?up|register|join|buy|pay|start (free )?trial|install|download`).
- **Deterministic.** No timers that depend on load speed. Waiting for a
  late overlay is an explicit `--wait`.
- The default output of existing commands does not change, except the
  `Obscured` hint's choice of button (20.4), which is a hint, not schema.

## What an overlay is

`overlays.ts` scans the viewport and keeps elements that are **on top**
and **not part of the page's own layout**.

**Candidates.** Hit-test a grid of points (5 × 5 over the viewport, plus
the centre and a point 8 px inside each edge's midpoint) with
`elementsFromPoint`, and climb each hit as `covering()` does to its
highest ancestor below `<body>`. Add every element in the top layer
(`:modal`, `:popover-open`) and every visible `[aria-modal=true]`,
`role=dialog|alertdialog` and `<dialog open>`, hit or not.

**Kept** when one of these holds:

| Kind | Rule |
|---|---|
| `modal` | in the top layer, `aria-modal`, a dialog role, or `position: fixed` and covering ≥ 40 % of the viewport (its own box or with a backdrop sibling) |
| `banner` | `position: fixed` or `sticky` (on it or the climbed ancestor), touching the top or bottom edge, at least 60 % of the viewport's width and at most 50 % of its height, and **not** the page's header or nav (see below) |
| `widget` | `position: fixed`, under 15 % of the viewport, in a corner (chat bubbles, feedback tabs) |

A `scrim` (a fixed element covering ≥ 80 % of the viewport with no text
and no controls) is not an overlay of its own: it is attached to the
modal or banner above it as its backdrop.

**Not overlays:** a sticky element that is a `header`, `nav`,
`role=banner|navigation`, or that holds three or more same-origin links,
and is at most 20 % of the viewport high. With `--all` they are listed
under "not overlays" with the reason, so a wrong call is visible.

**Signals** reported per overlay, not used to keep or drop it:

- `consent`: its text (first 2 000 characters) matches the consent
  words of any language in the table below, or its id or class matches a
  known consent tool root (`onetrust`, `CybotCookiebot`, `didomi`,
  `qc-cmp2`, `truste`, `usercentrics`, `sp_message`, `cmplz`,
  `cookie-law-info`, `cc-window`, `osano`, `iubenda`, `klaro`,
  `borlabs`, `cookiefirst`, `termly`). These are matched as names only;
  no code or styling from those tools is used.
- `locks scroll`: `overflow: hidden` on `html` or `body`, or `body` is
  `position: fixed`.
- `inerts page`: other content is `inert` or `aria-hidden` because of it.

Each overlay gets an id `o1`, `o2`, … in the order of its stacking
(topmost first), valid until the next scan.

**Buttons.** For each overlay, its controls (`button`, `[role=button]`,
`input[type=button|submit]`, and `a` without an `href` or with
`href="#"`) that are visible, are classified by their accessible name
(and `aria-label`, `title`, and the class for icon-only buttons):

| Role | Matches (case-insensitive, whole name or its start) |
|---|---|
| `reject` | reject, decline, deny, refuse, only (necessary\|essential\|required), continue without accepting, use necessary |
| `accept` | accept, agree, allow (all), i agree, ok(ay)?, got it, i understand |
| `close` | close, dismiss, ×, ✕, ✖, x, no,? thanks, not now, maybe later, skip, continue |
| `settings` | settings, preferences, customi[sz]e, manage, options, more options, learn more |

The same table in Norwegian (bokmål and nynorsk), Swedish, Danish,
German, French, Spanish, Italian, Dutch, Portuguese, Polish and Finnish,
picked by the page's `lang` and else tried in that order. The table is
one constant in `overlays.ts` with a test per language. In a `consent`
overlay, `ok` and `got it` count as `accept`; elsewhere they count as
`close`.

## Milestone 20.1 — `overlays`

```
chrome-cdp overlays [--all] [--frame FRAME]
```

A report: it never acts and exits 0 whatever it finds.

```
2 overlays
o1 consent banner  div#onetrust-banner-sdk [e12]  bottom, 1280×180
     reject: button "Reject All" [e14]  accept: button "Accept All Cookies" [e15]  settings: button "Cookie Settings" [e16]
o2 modal           dialog "Join our newsletter" [e20]  640×420, scrim div.backdrop [e19], locks scroll
     close: button "Close" [e21]  Escape: likely (modal dialog)
→ chrome-cdp close-overlays   (would click e14, then press Escape for o2)
```

The last line says what `close-overlays` would do with the same flags,
so the report doubles as its dry run. With nothing found: `no overlays`.

`Escape: likely` is said for a modal `<dialog>` (which closes on Escape
unless the page cancels it), and for an overlay with a keydown listener
for Escape found on it or `document` (`DOMDebugger.getEventListeners`,
only when the overlay has no `close` button, as it costs a round trip).

`--json`:
`{overlays: [{id, kind, element, box, consent, locks_scroll, inerts_page, scrim?, frame?, buttons: {reject?, accept?, close?, settings?}, escape: "likely"|"unknown"}], not_overlays?: [{element, reason}], plan: [step]}`,
where `element` is `{ref, role, name, cls?}` as in `why`.

Fixtures (`testdata/site/overlays/`, structure and ids only, nothing
copied from a real consent tool):

- `bottom-banner.html`: an OneTrust-like bottom banner with reject,
  accept and settings.
- `consent-wall.html`: a centred consent modal with a scrim and a scroll
  lock, accept and settings only (no reject on the first layer).
- `newsletter.html`: a `<dialog>` opened with `showModal()` after load,
  with an icon-only × button.
- `escape-only.html`: a `div[role=dialog]` without buttons that closes
  on Escape.
- `sticky-header.html`: a sticky `header` with nav links and a hero
  section: no overlays.
- `chat-widget.html`: a fixed bubble in the corner: one `widget`.
- `stacked.html`: a consent banner under a newsletter modal.
- `lang/{nb,de,fr}.html`: the bottom banner in each language.
- `app-modal.html`: an ordinary app modal ("Edit item", Save and
  Cancel) opened by a button, for the case where the overlay is the
  page's own.

## Milestone 20.2 — `close-overlays`

```
chrome-cdp close-overlays [--consent reject|accept] [--include-widgets]
                          [--escape|--no-escape] [--hide] [--rounds N]
                          [--wait DURATION] [--frame FRAME]
```

For each overlay, topmost first, it tries in order and stops at the
first that works:

1. In a `consent` overlay, the button for `--consent` (default
   `reject`). When that button is missing, it does **not** fall back to
   the other choice and does not open `settings`: the overlay is left
   and reported (multi-step consent dialogs are out of scope, see
   "Not in this plan").
2. Its `close` button.
3. For a non-consent overlay with exactly one button classified
   `accept` (a "Got it" notice), that button.
4. Escape (`press Escape` with focus inside the overlay), unless
   `--no-escape`. On by default for `modal` kinds, off for `banner`
   (`--escape` turns it on for all).
5. With `--hide`: `display: none !important` on the overlay and its
   scrim, and the scroll lock undone (the `overflow` and `position`
   values put back to what the page's stylesheet had before the
   overlay, or removed). Reported as `hidden`, never `closed`.

`widget`s are left alone unless `--include-widgets`.

A step **worked** when, within 1 s (or `--timeout` if shorter), the
overlay is detached, invisible, or no longer on top at any of its grid
points. A click whose report shows a navigation does count as worked,
and the output says so (some consent walls reload the page).

After each round it scans again, because closing one overlay can reveal
the next. It stops when a scan finds nothing it can act on, or after
`--rounds` (default 3). `--wait 3s` keeps scanning for that long after
the last round and closes overlays that appear late; without it, an
overlay that appears after the command returned is the next command's
problem.

```
closed o2 modal dialog "Join our newsletter": press Escape
closed o1 consent banner div#onetrust-banner-sdk: click 'role=button[name="Reject All"]'
left   o3 consent modal div.cmp-wall: no reject button (accept: button "Accept" [e40]; settings: button "Manage" [e41])
  hint: chrome-cdp close-overlays --consent accept, or chrome-cdp click e41 to choose
```

Each line names the action as `history --as-scenario` writes it (with
`stable.ts` locators), and `history` records those actions, not
`close-overlays`. So a scenario recorded after `close-overlays` replays
exactly what happened.

**Exit.** 0 when nothing is left (including when there was nothing).
When an overlay is left, exit 1 with a new error kind `OverlaysLeft`
(the error schema allows new kinds; docs/output.md lists it), its
`details` the `--json` below, and the hint for the first one left. A
step that throws (the page crashed, a dialog opened) fails as the action
would.

`--json`:
`{rounds, closed: [{id, kind, element, how: "click"|"escape"|"hide", step, navigated}], left: [{id, kind, element, why, buttons}]}`.

## Milestone 20.3 — Frames, shadow roots, late overlays

- **Out-of-process iframes.** A consent tool that draws into a
  cross-site `<iframe>` (Sourcepoint does) shows up in the main frame as
  an overlay whose element is the `iframe`. Its buttons are found by
  scanning that frame's document through the target graph (plan 13),
  and clicked in that frame, as `click` does today. The output names the
  frame (`in frame f3 (cmp.example)`).
- **Same-process iframes** are scanned the same way, without the
  target graph.
- **Shadow roots.** Open shadow roots are walked (`dom.ts` already
  climbs through hosts). An overlay with a **closed** shadow root is
  reported with `buttons: {}` and `why: "closed shadow root"`; only
  Escape or `--hide` can close it.
- **Late overlays.** The `--wait` loop uses a `MutationObserver` on
  `document` and the top layer (`toggle` events) in each frame, so it
  wakes on a change instead of polling, and still ends at the deadline.

Fixtures: `iframe-cmp.html` (a banner in an iframe on the second fixture
origin), `shadow-open.html`, `shadow-closed.html`, `late.html` (a banner
added 1.5 s after load: found with `--wait 3s`, not without).

## Milestone 20.4 — Where the rest of the tool meets overlays

- **The obscured error.** `covering()` uses the button table instead of
  `DISMISS`, with the same default as `close-overlays` (`reject` before
  `accept` in a consent overlay), and when the covering element is an
  overlay by 20.1's rules, adds the note
  `chrome-cdp close-overlays closes all of them`. The main hint stays the
  single `click`. `why`'s hit test line names the button the same way.
- **`layout`.** A `covered` finding whose cover is an overlay says so
  (`under consent banner o1`), so a cookie banner is not reported as a
  layout bug of every element under it.
- **`explore --close-overlays`.** Runs `close-overlays` (default
  flags) on every new state before listing its candidates, and records
  the closing steps in the state's path, so `states/*.cdp` replay them.
  Without the flag, `explore` is unchanged.
- **`chrome-cdp skill`**: one line ("a banner or modal in the way:
  `overlays`, then `close-overlays`; put the printed clicks in tests"),
  within the skill's token budget.
- **Docs.** `docs/commands/overlays.md` and `close-overlays.md`
  (generated), the `Obscured` section of `docs/troubleshooting.md`, a
  row in `docs/playwright-parity.md` (Playwright has none; its closest is
  `page.addLocatorHandler`, which runs a handler the user writes).

## Not in this plan

- **Multi-step consent** ("Manage" → turn everything off → "Save").
  The rules for it are per consent tool. If 20.2's `left` rate on real
  sites shows it matters, a later plan can bundle a rule set: Consent-O-
  Matic's rules are MIT (allowed by `deny.toml`); DuckDuckGo's
  `autoconsent` is MPL-2.0, which `deny.toml` does not allow today and
  would need the maintainer's decision.
- **A session mode** (`open --close-overlays` closing overlays in the
  background as they appear). It would make every later command depend
  on timing and hide what happened from tests. `--wait` and
  `explore --close-overlays` cover the cases seen so far.
- **Setting consent cookies up front.** Per site and per consent tool,
  and `cookies set` already does it for a site the user knows.
- **A `locator handler`** like Playwright's `addLocatorHandler`. The
  scenario format has no control flow (plan 08); revisit with plan 18's
  healing if tests keep breaking on overlays.

## Acceptance criteria

- [ ] `overlays` finds exactly the fixtures' overlays with the right
      kind, signals and buttons, and none on `sticky-header.html`.
- [ ] `close-overlays` leaves no overlay on every fixture where 20.2's
      steps can close it, and the page under it takes a click at the
      point that was covered.
- [ ] By default it never clicks an accept button in a consent overlay,
      never follows a link, never submits a form; `consent-wall.html`
      ends in `OverlaysLeft` with the hint, and `--consent accept`
      closes it.
- [ ] Each closing step in the output replays: `run` of the printed
      steps on a fresh load leaves the same overlays closed, and
      `history --as-scenario` after `close-overlays` has those steps.
- [ ] `--hide` reports `hidden`, and the page scrolls afterwards.
- [ ] The banner in a cross-site iframe and in an open shadow root are
      closed; the closed shadow root is reported.
- [ ] `late.html` is closed with `--wait 3s` and not without.
- [ ] The `Obscured` hint and `why` pick the same button as
      `close-overlays` on every fixture.
- [ ] `explore --close-overlays` maps `testdata/site/explore/` with a
      banner added to its pages to the same map as without the banner.
- [ ] Every new command and flag has help with an example, a
      `docs/commands/` page, a golden schema, and the skill line.

## Tests

- **overlays**: a golden per fixture, `--all` on `sticky-header.html`,
  `--json` schema, each language fixture, stacking order on
  `stacked.html`, a viewport where the banner is over half the height
  (a phone-sized `viewport`: still a `banner`).
- **close-overlays**: per fixture: what was closed and how; `--consent
  accept`; `--no-escape` on `escape-only.html` (left); `--escape` on a
  banner; `--include-widgets`; `--rounds 1` on `stacked.html` (one
  left); `--hide` (scroll restored, checked with `scroll` and `eval`);
  a close button that navigates (`navigated: true`); `app-modal.html`
  closed with Cancel, not Save.
- **Frames and shadow roots**: the 20.3 fixtures, and the `Obscured`
  error for a button under the iframe banner.
- **No false actions**: a fixture whose overlay has only a "Privacy
  policy" link, a "Subscribe" button and an email field: left, with no
  request made and no history entry.
- **Real sites** (an `#[ignore]` test, not run in CI: tests are
  offline): a list of 30 sites in several languages, recording per site
  what was found, closed and left. Run by hand; its numbers go in the
  decisions log and decide whether the multi-step rule set is worth a
  plan.

## Decisions log

_(append here)_
