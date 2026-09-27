# Plan 04 — Snapshot, refs & locators

Status: IN PROGRESS (4.1 and 4.3 done)
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
- [ ] Refs survive re-renders where the DOM node is preserved, and stale refs
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
