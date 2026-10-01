# Plan 21 — Named locators

Status: NOT STARTED
Depends on: 04 (the locator grammar), 09 §9.4 (`convert`), 11 (the
runner, `@include`, `lint`, `fmt`, `history --as-scenario`), 18
(fingerprints, `--heal`, `record`)

## Goal

A suite of a thousand tests names the same few hundred elements over and
over. Today each scenario spells out each locator:

```
click 'role=button[name="Add to cart"]'
```

When the button becomes "Add to basket", every line that names it
changes. `test --heal --write` makes that one run and one patch, but the
patch still touches every file that names the button, and a reviewer
reads the same hunk forty times. `@include` step files share *flows*
(log in, check out), but there is no way to share one *element* between
tests that do different things with it.

Playwright suites solve this with page objects: a class per page whose
fields are locators (`cartPage.addButton`). That is a code pattern, not
a feature. Everyone writes it a little differently, and nothing checks
that a field is used or that two fields mean the same element.

This plan adds named locators to the locator grammar:

```toml
# e2e/locators.toml
[cart]
item   = 'role=list[name="Cart items"] >> role=listitem'
add    = 'role=button[name="Add to cart"]'
remove = 'role=button[name="Remove"]'
```

```
click loc=cart.add
expect 'loc=cart.item >> has-text=Mouse >> loc=cart.remove' visible
```

What comes with them:

- **One place to change.** A rename is one line in `locators.toml`, and
  `--heal` patches that line, not the call sites.
- **Checked.** `lint` reports unknown names, unused names, entries that
  don't parse, and literal locators that duplicate an entry.
- **Written for you.** `history --as-scenario`, `record` and `convert`
  write `loc=…` when an entry matches. `locators extract` turns an
  existing suite's repeated literals into entries.
- **Optional.** A suite without a locators file works exactly as
  before. Literal locators stay first-class: a locator used once doesn't
  need a name.

### Why this beats Playwright page objects

| | Playwright page objects | chrome-cdp named locators |
|---|---|---|
| Defining them | A class per page, written by hand | A TOML table, or `locators extract` from the suite |
| After a redesign | A person edits the class | `--heal` patches the entry |
| Unused or duplicate locators | Nothing notices | `lint` warnings |
| A literal locator that should use the shared one | Nothing notices | `lint` suggests the name |
| Using one from the shell | Not possible | `chrome-cdp click loc=cart.add` in the project |
| Flows (log in, check out) | Methods on the class | `@include` step files (unchanged) |

Read plan 04 §4.4 (the grammar), plan 11 §11.4 (scenarios, `@include`)
and §11.12 (lint, `fmt`), and plan 18 §Healing first. Code:
`crates/engine/src/locator/parse.rs` (the grammar),
`crates/e2e/src/config.rs` (`[e2e]`), `crates/e2e/src/scenario/`
(loading, `@include`), `crates/chrome-cdp/src/script.rs`
(`locator_values`), `crates/chrome-cdp/src/lint.rs`,
`crates/chrome-cdp/src/test/{heal,fingerprints,exec}.rs`,
`crates/chrome-cdp/src/{history,record}.rs`, `crates/chrome-cdp/src/convert/`.

## Rules for every milestone

- Plan 12 §7 applies: the CLI and `--json` schemas only grow, every
  command follows plan 08 §2–§4, and every new flag has help with an
  example, a `docs/commands/` page and a golden schema.
- **Without a locators file nothing changes**: not output, not
  fingerprints, not patches. `loc=` with no file is an error that says
  where the file would be.
- **Names never hide what was found.** Every message that shows a
  locator that went through a name shows both:
  `loc=cart.add (role=button[name="Add to cart"])`. That covers action
  reports, errors, report.md, `steps.jsonl` and `--json` (a new optional
  `resolved` field next to the locator as written).
- **Nothing edits a committed file unless asked** (plan 18's rule):
  `locators extract` and `--heal` write patches; only `--write` applies
  them.

## Design

### The file

- `[e2e] locators` names a TOML file or a directory. Default:
  `e2e/locators.toml` if it exists, else `e2e/locators/` if it exists,
  else none. A directory's `*.toml` files are read in path order. Each
  file's stem is the first part of its names (`e2e/locators/cart.toml`'s
  `add` is `cart.add`), so a big suite gets one file per page or area.
- A value is a string (an entry) or a table (a namespace). Names are
  the keys joined by `.`. A key is `[a-z0-9_-]+`; uppercase is a load
  error with a hint, so names never differ only in case.
- An entry is a locator in the plan 04 grammar. It may use other entries
  (`remove = 'loc=cart.item >> role=button[name="Remove"]'`). A cycle is
  a load error that names the chain. It may use `${var:…}` and
  `${env:…}`, resolved per backend as in scenarios. It may not use refs
  (they die with the session) or `${saved.…}` and `${test.id}`
  (per-test values have no place in a shared definition).
- Two entries with the same name (in one file, or a directory's file
  and a table of the same name) are a load error at both places.
- Comments document an entry. There is no description field: TOML
  comments already do it and `fmt` keeps them.

### `loc=` in the grammar

- `loc=NAME` is a new step kind of the grammar. It stands for the
  entry's chain. Like a finder, it can be the first step of a chain or
  follow `>>`, and it can appear inside `has=`, `has-not=`, `and=` and
  `or=`.
- Expansion happens **before** the engine sees the locator, in one
  function of the e2e crate (`locators::expand(&Locators, &str) ->
  Result<Expanded, LocError>`). The engine stays unaware of names, and
  the daemon protocol only gains the optional `written` text for
  messages. `Expanded` keeps the spans of every `loc=` so messages and
  heal can tell which part of a chain came from which entry.
- An entry that starts with `frame=` or with a ref can only stand first,
  as those steps can. Expanding one later in a chain is an error at the
  use that names the entry.
- Where it works:
  - `test` and `run`: the project's locators, loaded once per run.
  - Interactive commands (`click loc=cart.add`): the CLI loads the
    locators of the project the current directory is in (the nearest
    `chrome-cdp.toml`, as for `[e2e]`) and caches them by mtime in the
    session dir. Outside a project, `loc=` is an error with the hint
    "named locators come from a project's e2e/locators.toml".
  - `history`: the session records the locator as written. `--as-scenario`
    writes it back as written.

### Healing an entry

Plan 18 heals a step whose locator is written in the file and refuses
one that comes from a variable. A `loc=` step is between the two: the
text is in a file, just not the scenario's.

- A step whose locator is exactly `loc=NAME` heals by changing the
  **entry**. `heal.patch` gets a hunk for the locators file, not the
  scenario.
- An entry is patched only if every use of it in the run agrees:
  - every step that used it and failed healed to the same new locator,
    and
  - no step that used it **passed** with the old one. A pass means the
    old locator still finds something, and changing the entry would
    break that use. The report says so and suggests the fix:
    "`cart.add` still finds an element at checkout.cdp:14; the uses may
    mean different elements: give them different names".
- An entry that uses other entries is patched only when the healed part
  is the entry's own literal text. Otherwise the report names the entry
  that should change, and nothing goes into the patch.
- A chain that mixes names and literal text
  (`loc=cart.item >> has-text=Mouse`) is patched in the scenario when
  the new locator keeps the named prefix (only the literal part
  changed). Otherwise it is reported unpatched, with the entry it
  passes through.
- Fingerprint keys stay `<line>:<locator as written>`
  (`5:loc=cart.add`). Each fingerprint gains an optional `loc` field
  with the entry names the locator used. The heal planner groups
  fingerprints by it.

### Lint

New rules, in `lint` and the lint that `test` runs first:

| Rule | Severity | When |
|---|---|---|
| `locators-load` | error | the locators file doesn't parse, has a bad name, a duplicate or a cycle |
| `unknown-locator` | error | `loc=NAME` names no entry; the hint names the closest names by edit distance |
| `locator` (existing) | error | an entry doesn't parse, or doesn't parse once expanded at a use |
| `ref` (existing) | error | an entry holds a ref |
| `unused-locator` | warning | no scenario or step file uses an entry; only reported when `lint` covers the whole `tests` set |
| `inline-locator` | warning | a literal locator equals an entry's text (after `fmt`'s quoting); the hint is the `loc=` to use |
| `duplicate-locator` | warning | two entries have the same text |

`inline-locator` and `unused-locator` read every file once: a set of
entry texts and a set of used names. That's linear in the suite's size.

### `locators` command

```
chrome-cdp locators [list] [--unused] [--json]   # names, values, use counts
chrome-cdp locators where NAME                   # every file:line that uses it
chrome-cdp locators extract [--min-uses N] [--write]
```

`extract` is how an existing suite moves to names:

- It finds literal locators (whole words, and chain prefixes ending at
  a `>>`) used at least `--min-uses` times (default 3) across the
  `tests` set and its step files.
- It names each one from its fingerprint: the page template's first
  path part (`/products/:id` → `products`, `/` → `home`), then the
  role and accessible name in snake case (`add_to_cart`), with a
  counter for clashes. Without a fingerprint, the namespace is
  `shared`.
- It writes `<artifacts>/locators.patch`: the new entries, and every
  use rewritten to `loc=…`. With `--write` it applies the patch. Run
  `fmt` and `lint` after.
- A locator with `${…}` in it is never extracted.

## Milestone 21.1 — The file, `loc=`, and running it

- `[e2e] locators` in `crates/e2e/src/config.rs`, and the loader
  (`crates/e2e/src/locators.rs`) with load errors at `file:line:col`.
- `loc=` in the grammar (the parser accepts the step; the engine's
  resolver refuses an unexpanded one with an internal error) and
  `locators::expand`.
- Expansion in `test`, `run` and the interactive commands that take a
  locator (`locator_values`' list), and `written`/`resolved` in
  messages, `steps.jsonl` and `--json`.
- `fmt` formats locator files: plan 11's quoting rule for values, keys
  in the order written, comments kept.
- The fixture suite, `testdata/locators/`: a project that runs against
  e2e-shop's built app (`dist/`) and mocks, with 40 or so tests that
  name the same elements many times (cart, product, checkout and
  sign-in controls, each in 10 or more tests). It has two copies of the
  scenarios: one with literal locators and one with names and a
  `locators/` directory (one file per page), so every later milestone
  can compare the two. It is written by hand once. The literal copy is
  what `extract` runs on in 21.4.
- `docs/e2e/locators.md` (new), and the reuse section of
  `docs/e2e/scenarios.md` points at it. It opens with when names are
  worth it: an element used across many tests or by several people
  writing tests. A handful of scenarios reads better with literal
  locators, and `--heal --write` already fixes those in one run.
  e2e-shop is the example of that and stays as it is.

## Milestone 21.2 — Lint and the `locators` command

- The rules of the lint table, with goldens.
- `locators list`, `where` and `--unused`.

## Milestone 21.3 — Healing entries

- The fingerprint `loc` field, grouping in the heal planner, the
  entry hunks, and the "still finds" and "uses another entry" reasons.
- `docs/e2e/healing.md`: a section on named locators.

## Milestone 21.4 — Writing names

- `history --as-scenario` and `record` write `loc=NAME` when a step's
  stable locator equals an entry's text exactly, and keep the literal
  otherwise. `--no-locators` turns this off.
- `locators extract`.
- `convert`: a Playwright page-object class whose constructor assigns
  locators to fields (`this.addButton = page.getByRole(…)`) becomes
  entries named after the class and the field (`CartPage.addButton` →
  `cart_page.add_button`), written to the file `--locators FILE` names
  (default: printed after the scenario as a TOML block). Uses of the
  field become `loc=…`. The class's methods are inlined where they are
  called, like the helpers `convert` already inlines. Other classes stay
  `# TODO`.
- `docs/e2e/migrating.md`: page objects → named locators and step files.
- The skill gets one line: "a locator the suite already names: `loc=`;
  `chrome-cdp locators` lists them".

## Not in this plan

- **Parameters on entries** (`loc=cart.item(name=Mouse)`). Chaining
  covers the common case (`loc=cart.item >> has-text=Mouse`). Add them
  if `extract` on real suites shows many near-identical entries that
  differ in one name.
- **Names in `snapshot`** (`button "Add to cart" [e6 loc=cart.add]`).
  It would help an agent reuse names, but every snapshot would resolve
  every entry. The history and `record` path covers writing tests;
  revisit if agents keep writing `inline-locator` warnings.
- **Methods and flows in the locators file.** Flows are `@include` step
  files. One mechanism each.

## Acceptance criteria

- [ ] Both copies of `testdata/locators/` pass, and give the same
      results test by test.
- [ ] Against the redesigned shop (plan 18.2's fixture), `test --heal`
      passes the same tests in both copies. In the named copy,
      `heal.patch` changes only the `locators/` files, one hunk per
      renamed element. The literal copy's patch has one hunk per use.
- [ ] `locators extract --write` on the literal copy gives entries for
      every locator used 3 or more times, and the suite still passes.
      Its names may differ from the hand-written copy's.
- [ ] An entry used for two different elements, where one was renamed,
      isn't patched, and the report says which use still passes.
- [ ] Each lint rule fires on a fixture and nowhere in the named copy
      of `testdata/locators/`.
- [ ] `chrome-cdp click loc=cart.add` works in `testdata/locators/`,
      and outside a project fails with the hint.
- [ ] e2e-shop is unchanged: no locators file, same goldens.
- [ ] Every message that shows a `loc=` locator also shows what it
      expanded to.
- [ ] A suite with no locators file gives byte-identical goldens,
      fingerprints and patches before and after this plan.
- [ ] `convert` on a Playwright spec with a page-object class gives
      entries and a scenario that pass against the same fixture.
- [ ] `cargo xtask release check` passes: the CLI and schemas only grew.

## Tests

- Unit: loading (namespaces, directory mode, bad names, duplicates,
  cycles), expansion (first and later in a chain, inside `has=`,
  `frame=` placement, spans), `fmt` round trips, the heal planner's
  entry rules on hand-made heals, `extract`'s naming and clashes.
- Integration (real Chrome): both copies of `testdata/locators/`; the
  redesign heal on both; `extract` on the literal copy; interactive
  `loc=`.
- Golden: each lint rule, `locators list/where/extract` text and
  `--json`, `test --heal` output with an entry hunk, `convert` of the
  page-object fixture.

## Decisions log

- `loc=NAME`, not `${loc:NAME}`: it is a step of the grammar, so it
  chains with `>>`, works inside `has=`, works from the shell without
  quoting `$`, and heal sees a literal word, not a variable. `@name` was
  ruled out because `@` already marks refs in places (`@e12`) and
  directives at the start of a line.
- TOML, like the mock and vars files. No new format.
- e2e-shop stays on literal locators. Its 7 scenarios repeat no locator
  more than 3 times, so names would add a second file to read and save
  next to nothing. It is the example new users copy, and literal
  locators are the right default at that size. Named locators get their
  own fixture suite, `testdata/locators/`, shaped like the suites they
  are for.
