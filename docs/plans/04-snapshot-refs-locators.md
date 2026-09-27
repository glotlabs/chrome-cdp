# Plan 04 — Snapshot, refs & locators

Status: IN PROGRESS (4.1-4.5 done; the token and latency criteria wait for plan 10's benchmarks)
Depends on: 03

## Goal

Build the agent's eyes. `chrome-cdp snapshot` returns a compact,
deterministic, accessibility-based view of the page, where every actionable
element has a short ref. A single locator engine resolves refs and
human-style selectors for every command. This is the product's main
differentiator: it must beat Playwright MCP's snapshot on tokens, fidelity
and speed.

## Milestone 4.1 — Output format

```
page t1 "Sign in – Acme" https://acme.test/login
- banner
  - link "Acme" [e1] → /
  - navigation "Main"
    - link "Pricing" [e2] → /pricing
- main
  - heading "Sign in" (h1)
  - form "Login"
    - textbox "Email" [e3] value="" required
    - textbox "Password" [e4] (password)
    - checkbox "Remember me" [e5] unchecked
    - button "Sign in" [e6]
    - text "Invalid password" (alert)
  - link "Forgot password?" [e7] → /forgot
- iframe "Payment" [e8]
  - textbox "Card number" [e9]
```

Rules:

- Two-space indentation, one node per line: `role "name" [ref] states…`.
  - Include only states that carry information: `checked`, `unchecked`,
    `mixed`, `expanded`, `collapsed`, `selected`, `disabled`, `readonly`,
    `required`, `invalid`, `focused`, `pressed`, and `(hN)` for heading level.
  - Values: `value="…"` for textboxes, comboboxes and sliders. Truncate them at
    100 characters with `…`, and never print password values.
  - Links show `→ href`, relative when same-origin.
- Refs go **only** on elements an agent can act on or that we need to address:
  interactive roles, elements with click handlers (see 4.3), scrollable
  containers, and iframes. Headings and text stay unreffed to save tokens.
  `--all-refs` overrides this.
- Pruning:
  - Remove `generic`/`none`/`presentation` nodes that have no name, and hoist
    their children.
  - Merge a parent's name when the only child is `StaticText` with the same
    text.
  - Merge adjacent text nodes.
  - Drop hidden and `aria-hidden` nodes.
  - Collapse long lists: after 30 siblings of the same role, print
    `… 170 more listitem (use --scope e40 --all)`.
- Text content: `text "…"` lines hold the visible text of non-interactive
  content, truncated at 200 characters per node. `--text-limit` controls this.
  `--no-text` gives a skeleton only.
- Deterministic: the same page state gives byte-identical output. Refs are
  stable too (4.2).

Flags:

- `--interactive` / `-i`: only reffed nodes and their landmark ancestors.
  Typically 5–10× smaller.
- `--scope <locator>`: a subtree only.
- `--depth N`.
- `--diff`: show only the changes since this tab's last snapshot, with `+`,
  `-` and `~` prefixes plus enough ancestor context to place each change.
  (Actions can include this automatically; see plan 08.)
- `--json`: a structured tree.
- `--max-chars N` (default 20k): above the limit, truncate with guidance
  ("snapshot truncated; use --interactive or --scope"). Never silently flood
  the agent's context.

## Milestone 4.2 — Ref registry

- Refs are per tab: `e<counter>`, where the counter is monotonic within a tab
  (`e1…`). Internally, map ref ↔ `(frame_id, backendDOMNodeId)`.
- **Stability:** if the same backend node appears in the next snapshot, it
  keeps its ref. New nodes get new numbers. Agents can therefore keep using
  `e6` after a re-render that didn't replace the node.
- **Invalidation:** a cross-document navigation of the main frame clears the
  tab's registry. The counter continues, so stale refs from before the
  navigation never silently hit a different element. A stale ref gives
  `RefStale` with a hint. A ref whose node was removed gives `RefDetached`
  ("element e6 is no longer in the page; run snapshot").
- **Fallback re-resolve:** store the node's role and name. If the backend node
  is gone but exactly one node with the same role and name exists in the same
  frame, use it and print a `note: e6 re-resolved by role+name`. This rescues
  most React re-renders. Disable it with `--strict-refs`.
- Refs from `find` (4.5) share the same registry.

## Milestone 4.3 — Snapshot engine

Implement two engines behind a trait. Choose the default by benchmark:

- **A. CDP AX tree:** `Accessibility.getFullAXTree` per frame (with
  `frameId`), merged across OOPIF sessions. Visibility and bounding boxes come
  from `DOMSnapshot.captureSnapshot` (layout + computed `visibility`/`display`)
  in the same round. Advantage: Chrome computes the names and roles, so they
  are spec-correct.
- **B. Injected walker:** a JS function in the isolated world walks the DOM,
  computes the role and accessible name (port of a compact accname
  implementation), and returns a flat array plus `backendNodeId` via
  `DOM.pushNodesByBackendIdsToFrontend`/`DOM.describeNode`. Advantage: it's
  fast on huge DOMs and can apply our own "is clickable" heuristics.

Clickable heuristic (either engine): non-semantic elements (`div`/`span`) are
promoted to reffed `clickable "text"` nodes when they have
`cursor:pointer` + a listener (`DOMDebugger.getEventListeners` is costly, so
batch it), `onclick`, `tabindex>=0`, or a role. This catches the many sites
that use `div`s as buttons.

Benchmark gate: run both engines on the corpus in plan 10 (about 30 real-world
page snapshots saved as MHTML/static fixtures, from simple to 50k nodes).
Criteria, in priority order: correctness vs expected tree, p95 latency (target
< 150 ms at 5k nodes, < 1 s at 50k), and output tokens. Record the result in
the decisions log.

## Milestone 4.4 — Locator engine

One grammar, used everywhere (`<loc>` in all plans):

| Form | Meaning |
|---|---|
| `e12`, `@e12` | ref |
| `#id`, `.cls`, `[attr]`, `css=…` | CSS (pierces open shadow roots) |
| `text=Sign in` / `text="exact"` / `text=/re/i` | visible text: substring by default, exact when quoted |
| `role=button[name="Sign in"]` | ARIA role + accessible name (same semantics as Playwright `getByRole`) |
| `label=Email`, `placeholder=…`, `alt=…`, `title=…`, `testid=…` | Playwright-parity getters (`testid` attribute configurable, default `data-testid`) |
| `xpath=//…` or leading `//` | XPath |
| `A >> B` | chaining (B inside A) |
| `>> nth=0`, `>> visible=true`, `>> has-text=…` | filters |
| `frame=… >> B` | scope into an iframe |

- Strictness: an action on a locator that matches **more than one** element
  fails with `Ambiguous`. The error lists the first 5 candidates with new refs
  and a snippet of context, so the agent can pick one directly
  ("3 matches for text=Delete: e41 (row 'Invoice 12'), e44 (row 'Invoice 13'),
  e47 …"). Read-only commands (`text`, `count`) accept multiple matches.
- Resolution happens in the isolated world through one injected `__ccdp`
  helper bundle, loaded once per context (`Page.addScriptToEvaluateOnNewDocument`
  with `worldName`). The bundle is TypeScript built at build time with
  `esbuild` via `xtask` and embedded with `include_str!`. The built JS file is
  committed so plain `cargo build` needs no Node.

## Milestone 4.5 — Read commands

- `find <query> [--role r] [--limit 10]`: fuzzy search over roles, names and
  text. Returns matches as snapshot lines with refs. It's cheaper than a full
  snapshot when the agent knows what it wants ("find checkout").
- `text [loc]`: the visible `innerText` of the page or element, whitespace
  normalised, with `--max-chars`. `html [loc] [--outer]`: pretty-printed and
  truncated. `attr <loc> <name>`, `value <loc>`, `count <loc>`,
  `box <loc>` (x, y, w, h, in viewport).
- `markdown [loc]`: the readable main content converted to Markdown
  (readability-style extraction). Agents often just want to read the page.

## Tests

- Golden-file snapshot tests (`insta`) for every fixture page. Cover forms
  (all input types), tables, lists (collapse), dialogs/modals, shadow DOM
  (open and closed), iframes (same-origin and OOPIF), `aria-hidden`, `display:none`,
  `visibility`, off-screen content, contenteditable, `div`-buttons, SVG icons
  with titles, and very long text.
- Ref stability: snapshot → re-render (React fixture that replaces a list
  with new keys vs. the same keys) → snapshot, then assert the expected refs
  are kept or changed.
- Locator tests: a table-driven test for each grammar form, including the
  ambiguity error format.
- Token budget test: the `-i` snapshot of the "big app" fixture stays under a
  set number of characters, so regressions fail CI.

## Acceptance criteria

- [ ] On the benchmark corpus, the default snapshot uses ≤ 60% of the tokens of
      Playwright MCP's `browser_snapshot` for the same pages, with no loss of
      actionable elements (audited list).
- [ ] p95 snapshot latency meets the targets in 4.3.
- [x] Refs survive re-renders where the DOM node is preserved, and stale refs
      never hit the wrong element (fuzz test: random DOM mutations + random ref
      use → either the correct element or an explicit error).

## Decisions log

- 4.3: The default engine is A, Chrome's AX tree. On the corpus both engines meet the expected trees, except that B (the DOM walker) cannot see into closed shadow roots: A 299/299 expected lines, B 298/299, no forbidden text from either. Output tokens are the same on 11 of 12 pages (15,627 vs 15,616 in total; the difference is the closed-shadow text box B misses). A also gets roles and names from Chrome, so they follow the specs as Chrome changes. The latency part of the gate, and the p95 acceptance box, are deferred: benchmarks run after all plans (`snapshot_latency_benchmark` in `crates/engine/tests/snapshot.rs` is built but `#[ignore]`d and not run).
- 4.3: The corpus is 12 fixture pages in `testdata/site/snapshot/` (simple, login, forms, hidden, clickable, lists-tables, widgets, shadow, frames, article, big-app ~5k nodes, huge ~50k nodes) rather than saved real-world MHTML pages; plan 10 grows it. Pages are generated in the page where they are large, so the repo stays small.
- 4.3: An "expected tree" is a `<page>.expected` file: lines that must appear in order and at their depth (refs written `[ref]`), plus `!text` that no line may contain. The score is the longest common subsequence. Partial trees scale to 50k-node pages, where a full hand-checked tree would not. The default engine must score 100% (`corpus_meets_expected_trees`), and its whole output is also kept as a golden file in `crates/engine/tests/snapshots/`.
- 4.3: Tokens are counted with a BPE-like split (a run of letters, up to three digits, or one other character), not a real tokenizer: deterministic, no dependency, and enough to compare engines and formats. Plan 10's Playwright comparison should use a real tokenizer.
- 4.3: Clickable heuristic: `onclick`, `tabindex >= 0`, or the element where a `cursor: pointer` region starts (its parent is not a pointer), and never `html`/`body`, labels, or anything inside a control. No `DOMDebugger.getEventListeners` check: React and most frameworks put their listeners on the root, so a per-node check misses them, and a listener without a pointer is mostly a container (a false positive). DOMSnapshot's `isClickable` has the same problem, so it is not used either.
- 4.3: Engine A reads what the AX tree lacks (display, cursor, overflow and scroll sizes, `onclick`, `tabindex`, `type=password`, `aria-valuetext`) from one `DOMSnapshot.captureSnapshot` per session, and stitches frames with `DOM.getFrameOwner`, all in one round of parallel calls. Chrome leaves elements without a role (a `<span>`, a `role=presentation` `<div>`) out of the tree; a clickable one is put back around its text through the DOM snapshot's parent links. Date/time inputs and contenteditable hosts are shown as `textbox` (Chrome exposes their inner parts or `generic`), and Chrome's slider value (clamped to the range) gives way to `aria-valuetext`.
- 4.3: Engine B (`--engine dom`) keys only the elements it must address (controls, clickables, scrollables, iframes, lists over 30, the `--scope` element) through `DOM.describeNode`, since one call per element would be too slow on big pages; its other nodes are matched by place in `--diff` and get no ref from `--all-refs`. It uses one object group per frame (frames of one process share a session).
- 4.1: An unnamed node whose only content is text shows the text as its name: `listitem "Apples"`, `alert "Invalid password"` (the plan's example has `text "Invalid password" (alert)`). One rule for list items, cells and alerts, with the role first like every other line; `--json` has the text in `name`.
- 4.1: Empty values are not shown (the example's `value=""`): an empty field is the common case and costs tokens on every form; no `value=` means empty. `scrollable` is shown as a state word after the others.
- 4.1: More pruning than the plan lists, each to save repeated text: a label's text next to (or around) its control, a legend or caption that repeats its group's name, children of a named button/link/heading/cell/option that hold nothing to act on, and a cell name that only repeats its buttons' names. Hoisted block elements keep their text apart (`text "First"`, `text "Second"`); inline ones join it, spaces kept.
- 4.1: Long lists are cut once: where any role reaches its 31st sibling, with the rest summed up per role (`… 570 more button`, `… 285 more checkbox`); a few nodes of rarer roles after the cut stay. Cutting each role separately split mixed lists (a row's checkbox, link and buttons) at different rows. Text is never collapsed. The list gets a ref so the hint (`use --scope e40 --all`) can name it.
- 4.1: `--interactive` keeps tables, grids and rows as well as landmarks around the refs, so the controls of one row stay together (the big-app `-i` is 7,650 chars; `interactive_big_app_stays_within_its_budget` fails above 8,500).
- 4.1: `--depth N` shows N levels; a node at the last one says `- … 12 more below (use --scope e5)` (or `use a larger --depth` without a ref).
- 4.1: `--diff` compares with the tab's last snapshot (kept as the engine's tree, whatever flags it had), with the current flags applied to both. Nodes match by (frame, backendNodeId), else by place (parent, role, name, index). An added subtree is shown whole, a removed node by its line only, a changed line ends with `(was …)` holding the old parts. Without an earlier snapshot, everything is `+` and the text says so.
- 4.1: `--max-chars` (default 20,000) cuts the text at a line boundary and ends with `… snapshot truncated: N more chars (M lines); use --interactive, --scope <ref> or --max-chars 0 for all of it`; `--json` is bounded by plan 08's rules.
- 4.1: Until the locator engine (4.4), `--scope` takes a ref, or a CSS/XPath locator (first match in the main frame). Refs have only what 4.1 needs: a per-tab registry keyed by (frame, backendNodeId) with a monotonic counter, so a node keeps its ref and numbers are never reused. Invalidation, `RefStale`/`RefDetached` and the role+name fallback are 4.2.
- 4.1: `snapshot` fails at once with `DialogOpen` (exit 5) while a dialog blocks the page, like `wait`.
- 4.2: A same-document navigation (`pushState`, `#fragment`) does not clear refs, though plan 03 counted it as one for ref invalidation: SPA route changes keep most nodes (the nav bar, the layout), and a ref names its node, so a kept node is still the right element and a replaced one fails. It turns the role+name fallback off for refs last shown before it, since after a route change a look-alike is likely another view's element. A later snapshot turns it on again for the refs it shows.
- 4.2: An iframe that loads a new document drops its own refs and its subframes' (`RefStale`, "from an earlier document of its frame"), not only the main frame's navigation: its backend ids name nodes of a document that is gone. A navigation counts when the frame's loader id changes, so a repeated `frameNavigated` for the same document keeps refs. A back/forward cache restore of the main frame is a new document too (its refs were cleared when it was left).
- 4.2: A number the tab never gave is `NotFound` ("tab t1 has no ref e99"), not `RefStale`; stale is only for numbers given before a navigation, which the registry tells apart from the counter (everything up to the counter's value at the last page load is stale).
- 4.2: A ref resolves with `DOM.resolveNode` (backend id → object in chrome-cdp's isolated world of its frame), then `isConnected && getRootNode({composed: true}) === document`: a node of an earlier document stays connected to that document, and closed shadow roots still reach the document through their host. An unknown backend id and a node from another document are both "gone".
- 4.2: The fallback reads the ref's frame alone with engine A and prunes it like a full snapshot, without collapsing lists or cutting names, and takes the one node with the same role and name (a name the snapshot cut at `--text-limit` matches by its start). Hidden elements are not candidates. With exactly one, the ref is pointed at it for later commands too; with none or several it is `RefDetached`, with `was: role "name"` and, for several, how many (details: `ref`, `role`, `name`, `matches`). The frame id is left out of details: it differs per run.
- 4.2: The "note: e6 re-resolved by role+name" is an output warning (stderr in text form, `warnings` in JSON): `Output` has no separate note channel, and it must not mix into stdout.
- 4.2: `--strict-refs` (with `CHROME_CDP_STRICT_REFS`) is a flag of the commands that take refs, only `snapshot --scope` today, not a global flag: plan 08 does not add flags that some commands would ignore. Actions (plan 05) and the locator engine (4.4) add it where refs are accepted.
- 4.2: `Engine::resolve_ref` is the one way in for everything that takes a ref (it returns a handle in the isolated world); `Engine::call_on_ref` calls a function on the element, the building block for plan 05's actions and plan 06's `eval --in`. `find` (4.5) gives refs through the same `Refs::assign`.
- 4.2: The re-render fixture (`testdata/site/refs/rerender.html`) is a small keyed renderer that reconciles the way React does (same key, same node; `remount` replaces all), not React itself: the fixtures stay offline and dependency-free. The fuzz test (`fuzz_refs_hit_their_element_or_fail` in `crates/engine/tests/refs.rs`, 40 rounds, `REFS_FUZZ_SEED` picks another run) removes, replaces with look-alikes, renames, moves, adds and hides elements, with a reload and a route change every ten rounds; every ref must hit its own element, the only shown look-alike after its own is gone (never under `--strict-refs` or after a route change), or fail as stale/detached, and one whose element is still in the page must hit it. Ten seeds passed.
- 4.2: The golden tests gained a `SessionAfter` fixture (setup commands before the case) for the stale, detached and ambiguous ref errors.
- 4.2: A snapshot read while a frame loads a new document is read again (up to 3 times): refs given after the registry was cleared would name the old document's nodes, and the fallback could then re-resolve one to a look-alike on the new page. `Refs::generation` counts documents; the check and the ref assignment happen under one lock. The CSS/XPath `--scope` lookup now releases its handle in the page.
- 4.4: The bundle is TypeScript in `crates/engine/src/injected/src/`, typechecked with `tsc` and bundled with `esbuild` (both pinned, run through `npx` by `cargo xtask build-js`) into `injected/ccdp.js`, which is committed unminified (about 30 KB) so diffs and stack traces stay readable. Its first line holds a hash of the sources, and an engine unit test that needs no Node fails when the bundle is older than them; `build-js --check` compares the whole file where Node is available. `xtask ci` does not run it, so CI needs no Node.
- 4.4: The bundle is the source of the `Page.addScriptToEvaluateOnNewDocument` call that already made chrome-cdp's isolated world, so every document gets `__ccdp` before its own scripts run; a world made later on demand gets it at once. Functions that use it throw if it is missing, and the engine then loads it and calls again.
- 4.4: Engine B's DOM walker moved into the bundle (`walker.ts`), so `role=` locators and the walker share one role and accessible-name implementation; the corpus scores are unchanged.
- 4.4: One rule for text values everywhere (`text=`, `has-text=`, `[name=…]`, `label=`, `placeholder=`, `alt=`, `title=`): unquoted is a case-insensitive part of the text, quoted is the whole text (case-sensitive), `/re/flags` a regex, whitespace collapsed on both sides. Playwright's `role=button[name="x"]` is a case-insensitive part; here the quotes mean exact, as in `text="x"`, so one rule covers every form. `testid=` is exact.
- 4.4: `text=` matches only visible elements: the innermost ones whose text (open shadow roots and slots included, hidden elements left out) matches, as Playwright picks the innermost. Playwright also matches hidden elements; here a hidden copy (a mobile menu, a template) would make actions ambiguous for no gain. Other finders count hidden elements, except `role=`, which leaves out what the accessibility tree hides unless `[include-hidden]` (as Playwright).
- 4.4: Bare forms: `#`, `.`, `[` start CSS, `//` and `(//` XPath, a quoted string is exact text. Any other bare word is `BadLocator` with a hint for `text=` and `css=`, not CSS as in Playwright: an agent typing `Save` more likely means the text. A quote opens only where a value starts (after `=`, `[`, `(`, `,` or at a step's start), so `text=Don't save` needs no quoting.
- 4.4: A ref can only be the first step and `frame=` steps only lead. A ref to an iframe followed by more steps looks inside the iframe's document (`e8 >> role=textbox`), like `frame=e8 >> …`. `frame=` takes the frame's name, a part of its URL, its id or its iframe's ref (tab-wide, plan 03's `FrameTree::find`). A locator that is only `frame=…` stands for the `<iframe>` element (`snapshot --scope frame=payment`), and for the frame's document in `wait --text … --in` (as does an iframe's ref alone there).
- 4.4: CSS pierces open shadow roots the way Playwright's does, cheaply: native `querySelectorAll` on the scope, plus, in each open shadow root, the selector's last compound natively and the compounds before it checked against the ancestors across shadow boundaries. So descendant combinators cross into shadow trees and `>`, `+`, `~` stay in one tree. Closed shadow roots are out of the isolated world's reach. XPath in a chain is relative to the element before it (`//x` becomes `.//x`), as in Playwright.
- 4.4: The `Ambiguous` error is `N elements match <locator>: e1, e2, e3` (`, …` past five), a note per candidate (`e41: button "Delete" (in row "Invoice 12 $120 Delete")`, `hidden` when it is), `…: N more`, and the candidates in `details`. Each gets a ref through the tab's registry, keyed like a snapshot's, with the bundle's role and name. The context is the nearest container (row, list item, dialog, form, region, …) with a name; rows and list items are named by their text, cut at 40 characters. The hint runs the command again with the first ref when the command says how (`snapshot --scope e1`), else says to use a ref or add `>> nth=<index>`.
- 4.4: `testid=` reads `data-testid` unless the `testid_attribute` config key or `CHROME_CDP_TESTID_ATTRIBUTE` names another; there is no flag, as plan 08 keeps global flags few. Requests that carry locators carry it too (`SnapshotRequest`, `WaitRequest`), with `strict_refs`.
- 4.4: `wait` resolves its locator's ref and `frame=` head on every poll round, so a frame that appears later is waited for (checked every 100 ms) and a navigation is followed. A ref whose element is gone meets `--state detached`/`hidden` and `--text-gone`; for other states it fails at once (`RefDetached`, `RefStale`): waiting cannot bring it back. `wait` gained `--strict-refs`, and prints the re-resolve note like `snapshot`.
- 4.4: `role=` uses the bundle's own role and name code (engine B's accname port), not Chrome's, so it runs in one call. Where the two differ, a locator and the snapshot can disagree: a table without header cells is a `layouttablerow` in Chrome's tree (and the default snapshot) but still `row` to `role=`.
- 4.4: The building blocks for plan 05 and milestone 4.5 are `Engine::call_on_locator` (exactly one element, `this` in the isolated world) and `Engine::call_on_all` (`this` is the array of matches), over `locate_one` and `place`. Candidate refs and fallback notes flow through them like `call_on_ref`'s.
- 4.5: `find` searches the tree a snapshot would show (engine A, pruned the same way, lists not collapsed), not the DOM, so its lines, roles and names are the snapshot's. Every query word must match, in any order: a part of the name or value (any case), the role, or, for words of five letters or more, a word of the name one edit away (optimal string alignment). Matches rank by how well the whole query fits the name (exact, prefix, part), then controls before other elements before text, then page order. Each line ends with the nearest named container (`(in form "Cart")`, `(in iframe "Payment")`); the output starts with the `page …` line like `snapshot`, `--limit 0` shows all, and `-q` prints the refs.
- 4.5: `find` gives refs through the tab's registry (as `--all-refs` would) to every match shown except text, which an action could not use; it does not change `snapshot --diff`'s baseline. No match is `NotFound` (exit 1) with a `snapshot --interactive` hint: an agent that searched meant to act. The generation check of `snapshot` (read again if a frame loaded a new document) applies too.
- 4.5: One `read` RPC serves `text`, `html`, `attr`, `value`, `count`, `box` and `markdown` (`Read` says which), over one `Engine::call_on`: exactly one match (`Ambiguous` otherwise, whose hint reruns the command with the first ref) or all of them (`text`, `count`). `call_on_locator` and `call_on_all` are now thin wrappers. Without a locator, and for one that names only a frame (`frame=…`, an iframe's ref), `text`, `html` and `markdown` read the (frame's) document; `attr`, `value`, `count` and `box` need a locator and take the `<iframe>` element for `frame=…`. Like `snapshot`, they fail at once with `DialogOpen` while a dialog blocks the page.
- 4.5: `text` is `innerText` tidied (no-break spaces and runs of spaces to one, lines trimmed, at most one empty line in a row). `innerText` does not enter shadow roots, so the elements around open shadow roots are walked in the bundle (block elements break lines) and the rest read with `innerText`. Several matches print one text per line (the JSON has the array); a hidden match has empty text and a warning; no match is `NotFound`, while `count` prints 0. The page's text leaves iframes out; `text frame=…` reads one.
- 4.5: `html` pretty-prints in the bundle: one element per line, two spaces per level, an element on one line when it and its content fit in 120 columns, whitespace between tags dropped and text collapsed (not in `pre`/`textarea`), attribute values, comments and script/style bodies cut at 200 characters, open shadow roots as `#shadow-root (open)`. It prints an element's content unless `--outer`, and the whole document (with its doctype) without a locator. `--raw` (not in the plan) gives the browser's own serialisation for when the exact markup matters.
- 4.5: `attr` of a missing attribute is not an error: stdout is empty (`value: null` in JSON) and a warning names the attributes the element has; `box` of an element that is not rendered is the same (`box: null`). An empty stdout means absent, and `$(…)` captures stay clean.
- 4.5: `value` reads inputs, textareas, selects (a multiple select's values one per line, and in `values`), file inputs (file names), `<output>`, `<progress>`, `<meter>`, editable elements (their text) and ARIA widgets (`aria-valuetext`, else `aria-valuenow`); the JSON says which `kind`. A password field's value is printed: the agent asked for it by name, unlike a snapshot, which never shows one. An element without a value is `NotFound` ("it is a <p>, not a field") with a `text` hint.
- 4.5: `count` also says how many matches are visible (`4 (3 visible)`; `-q` prints only the count). `count` and `box` are fixed-size, so they have no `--max-chars`.
- 4.5: `box` is `getBoundingClientRect` in the element's frame plus, for each frame it is in, where that frame's `<iframe>` content box starts (its box, border and padding) in the parent, so it works through cross-site (OOPIF) frames too. Values are CSS pixels rounded to hundredths; `in_viewport` says whether any of it is in view, and the text adds `(outside the WxH viewport)`. Plan 05's clicks can build on the same offsets.
- 4.5: `markdown` picks the main content like a small readability: a single visible `<main>`/`[role=main]`, else a single `<article>`, else the element whose paragraphs (`p`, `pre`, `blockquote`, `td`, `li` of 25+ characters, scored by length and commas, half to the grandparent) score highest, else `<body>`; JSON `source` names it (`div#content`). Inside it, `nav`, `aside`, `footer` and their roles are left out, and always controls, labels, media, iframes and hidden or `aria-hidden` elements. Block vs inline comes from computed `display`. It converts headings, paragraphs, tight and nested lists, links (relative on the page's origin), images (without `data:` URLs), code spans and fenced `pre` (with a `language-` class), blockquotes, GFM tables (the first row is the header), `dl`, `hr` and `br` (a plain line break). It does not escape Markdown characters in text: agents read it, and escapes cost tokens.
- 4.5: The Ambiguous error names a nameless candidate that is not a control by its text (`listitem "Apples"`), so the ref it gets has a role and name for the fallback.
- Close-out: `--diff` after the main frame loaded a new document (a reload, a form post) says `(a new page since the last snapshot; its nodes are compared by place)` (`new_page` in JSON): the new nodes never match the old by key, so a diff that looks like a large edit is explained. The last tree is kept rather than dropped at the load, since a place-matched diff of a reloaded page still shows what the load changed (an error message after a form post).
- Close-out: `find` takes `--scope <loc>` (and `--strict-refs`) like `snapshot`: the same locator lookup and subtree cut, the scope node itself included, contexts from the scope down. An ambiguous scope's hint reruns the `find` with the first ref; no match notes that only the scope was searched.
- Close-out: each ref remembers the engine whose role and name it keeps, and the role+name fallback reads the frame with that engine: `snapshot --engine dom` refs and the `Ambiguous` candidates' refs (named by the bundle, engine B's code) with the DOM walker, the rest with Chrome's tree. Where the two engines differ, a ref could otherwise never re-resolve.
