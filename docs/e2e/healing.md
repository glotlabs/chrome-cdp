# Healing broken locators

A redesign renames a button or moves it, and every test that clicks it
fails on a locator that finds nothing. `chrome-cdp test` remembers what
each step's element looked like when the test last passed, so a failure
can name the element it most likely is now, and `--heal` can fix the
tests.

```
chrome-cdp test                   # the failure names the likely element
chrome-cdp test --heal            # run with it; heal.patch holds the changes
chrome-cdp test --heal --write    # and apply them to the scenario files
```

## Fingerprints

After a test passes, the runner writes a fingerprint of the element each
of its steps found to `e2e/fingerprints/<scenario>.json` (next to
`e2e/snapshots`). Commit these files: they are what healing works from.

A fingerprint holds the element's role and accessible name, tag and
`type`, test id (of the project's `testid_attribute`), id, label,
placeholder, the first 80 characters of its text, up to 5 class names
(without generated ones such as `css-1x2y3z`), its container (the nearest
dialog, form, list item, row, region or landmark) and its place among the
elements of its role there, the page's path as a template
(`/products/:id`), and the stable locators that find it. Nothing that
changes between two runs of an unchanged app: two runs write the same
bytes, and a file is only rewritten when something changed.

The steps that get one are those whose locator must find one element:
actions (`click`, `fill`, `select`, `check`, `hover`, `drag`, `upload`,
…), `wait <loc>` for an element that is there, and `expect <loc>` with a
matcher that reads the element (`visible`, `text`, `value`, …, not
`count`, `hidden` or `--not`). The steps of a test's `@before` and
`@after` count as the test's; `@before-all` and `@after-all` get none.

Each file has one object per test, then one per step:
`<line>:<locator as written>`, with the step file first for an
`@include`d step (and the include's parameters the locator uses):

```json
{
  "version": 1,
  "tests": {
    "adds a product to the cart": {
      "5:role=button[name=\"Add to cart\"]": {
        "role": "button",
        "name": "Add to cart",
        "tag": "button",
        "testid": "add-to-cart",
        "text": "Add to cart",
        "classes": ["button", "button-primary"],
        "container": { "role": "main" },
        "index": 0,
        "page": "/products/:id",
        "candidates": ["role=button[name=\"Add to cart\"]", "testid=add-to-cart", "text=\"Add to cart\""]
      }
    }
  }
}
```

A passed test's fingerprints replace the ones it had, so those of steps
that are gone go too. `[e2e] fingerprints` says what a run does with
them:

| Value | Reads | Writes |
|---|---|---|
| `auto` (default) | yes | yes, unless the `CI` environment variable is set |
| `write` | yes | yes |
| `read` | yes | no |
| `off` | no | no (and `--heal` is an error) |

Only runs against the mocks write them (a real backend's data changes).
No value of a password field or a `${secret:…}` is ever written: a field
of a fingerprint that holds a secret's value is left out.

## When a locator breaks

When a step with a fingerprint fails and its locator finds no element
now (or several, where it must find one), the runner scores the page's
elements against the fingerprint:

| Signal | Weight |
|---|---|
| test id equal | 2 |
| role equal (of one family, such as `button` and `link`: 1.5) | 3 |
| name equal / similar | 3 / 2 |
| label, placeholder, text equal / similar (each, when not the name) | 2 / 1.5 |
| container's role and name equal | 2 |
| id equal | 1 |
| class names (their Jaccard index) | 1 |
| tag and `type` equal | 1 |
| place in the container equal | 0.5 |

Texts are similar when they are close by edit distance, share two thirds
of their words, or one is the other with words added (`Add to cart`,
`Add to basket`). An element of another role gets half marks at most.
The **confidence** is the best score over the most it could have had,
lowered when the runner-up comes close; `high` is 0.8 or more, `medium`
0.6 or more.

Without `--heal`, the test fails as before, and the error and
`report.md` name the element:

```
✗ e2e/scenarios/cart.cdp:3 adds a product to the cart (4.3s)
    failed at e2e/scenarios/cart.cdp:5:3: click 'role=button[name="Add to cart"]'
    error: no element matches role=button[name="Add to cart"]
      waited: 5s
      likely the same element now: role=button[name="Add to basket"] (confidence 0.84: same testid, role, container and tag; similar name)
      hint: run it with --heal to use it (chrome-cdp test e2e/scenarios/cart.cdp:3 --heal), or change the locator
```

A low match is named as `closest element now`, "too low to heal": the
element may be gone, and the test needs a person (or an agent) to look.

## `--heal`

With `--heal`, a `high` match (and with `--heal=medium`, a `medium` one)
runs the step again with the element's most robust locator, and the test
goes on. The step is marked healed in `steps.jsonl`, under the test's
line and in `--json` (`healed: [{file, line, from, to, confidence,
reasons}]`):

```
✓ e2e/scenarios/cart.cdp:3 adds a product to the cart (5.5s)
    healed e2e/scenarios/cart.cdp:5: role=button[name="Add to cart"] -> role=button[name="Add to basket"] (0.84: same testid, role, container and tag; similar name)
heal.patch: /app/e2e/.results/heal.patch (1 locator; --heal --write applies it)
```

The heals of the tests that passed become `<artifacts>/heal.patch`, a
unified diff of the scenario and step files with paths relative to the
project root (`git apply e2e/.results/heal.patch` there). Nothing changes
a committed file unless you ask:

- A test that fails later, after a heal, contributes nothing: its
  `report.md` lists the heal as suspect.
- A locator the file does not spell out (it comes from `${var:…}` or an
  `@include` parameter) cannot be patched: the run says where it comes
  from and what the step should find.
- Two tests that heal one line differently are a conflict: neither goes
  into the patch, and both are reported.

`--heal --write` applies the patch after the run (only if every change
still applies to the files as they are then), prints the files it
changed, and moves the healed steps' fingerprints to their new keys.

## How it is built

The runner sends each step's locator with the step's call to the daemon;
the daemon reads the element (`injected/src/fingerprint.ts`) when it
resolves the locator for the action, before the action runs, or once an
`expect` or `wait` passed. On a failure, a call of its own
(`heal_match`) reads the page's candidates, the engine scores them
(`crates/engine/src/fingerprint/score.rs`), and names the best one's
stable locator, the one `history --as-scenario` would write.
