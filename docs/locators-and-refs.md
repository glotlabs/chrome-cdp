# Locators and refs

Every chrome-cdp command that names an element takes a **locator**. The
shortest and most reliable one is a **ref**: a name like `e12` that
`snapshot` or `find` gave an element. This page explains both, and when to
use which. `chrome-cdp help locators` and `chrome-cdp help refs` are the
short versions.

## The loop: look, then act by ref

```sh
chrome-cdp snapshot -i
```

```
page t1 "Sign in" https://acme.test/login
- banner
  - link "Acme" [e1] → https://acme.test/
  - navigation "Main"
    - link "Products" [e2] → https://acme.test/products
- main
  - textbox "Email" [e3]
  - textbox "Password" [e4] (password)
  - checkbox "Remember me" [e5] unchecked
  - button "Sign in" [e6]
```

```sh
chrome-cdp fill e3 ada@example.com
chrome-cdp fill e4 "$APP_PASSWORD"
chrome-cdp click e6
```

`snapshot` shows the page as an accessibility tree: roles and names as a
screen reader gets them, one node per line
(`- role "name" [ref] (hN) value="…" states… → href`). Everything an agent
can act on has a ref: controls, links, elements that only act like buttons
(`onclick`, `tabindex`, `cursor: pointer`), scrollable boxes and iframes,
whose content, cross-site too, is shown inside them. `-i` keeps only what
has a ref, with the landmarks, tables and rows around it for context.

The output is bounded: hidden content and the page behind an open modal
are left out, a list is cut after 30 items of a kind (`--all` shows them),
text after 200 characters (`--text-limit`) and the whole output after
20,000 (`--max-chars`). The same page gives the same output.

To see less:

```sh
chrome-cdp snapshot --scope e40            # the part under e40 (or any locator)
chrome-cdp snapshot --scope 'frame=payment'
chrome-cdp snapshot --depth 3
chrome-cdp snapshot --diff                 # only what changed since the tab's last snapshot
chrome-cdp find 'sign in' --role button    # search instead of reading the page
```

`find` searches names, values and text (every word must match, a typo in
a long word is forgiven) and prints the best matches as snapshot lines,
each with the named container it is in. It is the cheapest way to get a
ref when you know what you are looking for.

`eval` gives refs too: elements in a script's value print with theirs,
so a query leads straight to an action (the first 50 elements of a value
get one; a warning says how many more did not):

```sh
chrome-cdp eval '[...document.querySelectorAll(".row button.delete")]'
# ["button.delete [e41]", "button.delete [e44]", …]
chrome-cdp click e44
```

After acting, you rarely need a whole new snapshot: each action reports
what it caused (a navigation, a dialog, errors), and `--snap` adds what
changed on the page (`snapshot --diff -i`) to the report:

```sh
chrome-cdp click e6 --snap
```

## How long a ref lives

- A ref names one element for as long as its document lives, also when
  the page re-renders around it. Ref numbers are never reused in a tab.
- A navigation (a new document in the tab or in a frame) drops its refs.
  Using one then fails with `RefStale`: take a new `snapshot -i`. The
  action that navigated says so in its report (`→ navigated to …`).
- When a framework replaced the element (React, Vue and Svelte do that),
  the ref moves to the one element of its frame with the same role and
  name, and says so (`warning: e6 re-resolved by role+name`). With none,
  or several, it fails with `RefDetached` and what it was.
- `--strict-refs` (or `CHROME_CDP_STRICT_REFS=1`) turns the role+name
  fallback off, and so does a same-document route change (`pushState`)
  for the refs shown before it.
- Refs belong to a tab. Pass the same `--tab` you took the snapshot in.

Refs are for the session that made them. Scripts and tests that run
later, in a new browser, need locators: `chrome-cdp lint` flags refs in a
scenario, and `chrome-cdp history --as-scenario` turns the refs of the
commands you ran into the most robust locator that found each element.

## Locators

| Form | Finds |
|---|---|
| `e12`, `@e12` | the element a snapshot gave that ref |
| `#id`, `.cls`, `[attr]`, `css=…` | CSS; a space also reaches into open shadow roots |
| `//…`, `xpath=…` | XPath |
| `text=Sign in`, `text="Sign in"`, `text=/sign.?in/i` | the innermost visible element with that text: a case-insensitive part, the whole text when quoted, or a regex |
| `role=button[name="Save"]` | an ARIA role and accessible name (quoted: exact), with `[checked]`, `[disabled]`, `[expanded]`, `[level=2]`, `[pressed]`, `[selected]`, `[include-hidden]` |
| `label=Email`, `placeholder=…`, `alt=…`, `title=…`, `testid=…` | by label, attribute, or test id (`data-testid`; `testid_attribute` in the config changes it) |
| `A >> B` | B inside A; `>> nth=0` (`-1` is the last), `>> visible=true`, `>> has-text=…` filter |
| `frame=payment >> #card` | inside an iframe, named by its name, a part of its URL, or its ref (`e8 >> #card` also works) |

Text values (`text=`, `name=`, `label=`, `placeholder=`, `alt=`,
`title=`, `has-text=`) match a case-insensitive part of the text;
`"quoted"` matches the whole text, case-sensitive; `/regex/i` is a regex.
Whitespace is collapsed. A bare word is not a locator: `h1` is an error
that suggests `css=h1` or `text=h1`.

Quote locators for the shell. Single quotes are easiest, since role
names use double quotes:

```sh
chrome-cdp click 'role=button[name="Sign in"]'
chrome-cdp fill label=Email ada@example.com
chrome-cdp click '#list >> text=Delete >> nth=-1'
chrome-cdp fill 'frame=payment >> placeholder=Card number' 4242424242424242
chrome-cdp click 'role=row >> has-text=Invoice 12 >> role=button[name="Delete"]'
```

### One match, or an error that lists them

A command that acts on one element needs exactly one match. With none it
waits (up to `--timeout`) and then fails with `NotFound`. With several it
fails at once with `Ambiguous` and lists them with new refs and where
each is, so the next command can use the right ref:

```
error: 4 elements match .item: e1, e2, e3, e4
  e1: listitem "Apples"
  e2: listitem "Bananas"
  e3: listitem "Cherries" (hidden)
  e4: listitem "Dates"
  hint: chrome-cdp html e1   (the first one), or add >> nth=<index> to .item
```

`wait`, `count` and `text` take any number of matches.

### Which locator to write

In a script or a test, prefer what a user sees, since it survives
redesigns: `role=` with a name, then `label=`, then `text=`. Use
`testid=` where the app has test ids, CSS for structure the user cannot
see, and `nth=` only as a last resort. That is also the order `history
--as-scenario` picks in.

## Reading without a snapshot

- `text [loc]`: the visible text of the page or of each match, tidied.
- `markdown [loc]`: the page's main content as Markdown, without
  navigation, sidebars and footers.
- `html [loc] [--outer] [--raw]`: the HTML, pretty-printed, open shadow
  roots included.
- `attr <loc> <name>`, `value <loc>`, `count <loc>` (`4 (3 visible)`),
  `box <loc>` (`x=24 y=310 w=120 h=36`).

`text frame=payment` reads an iframe's document. An element that is not
there is `NotFound`; an attribute that is not there prints nothing and a
warning.

## Frames and shadow DOM

Each tab's frames are tracked, cross-site (out-of-process) iframes
included. A snapshot shows an iframe's content inside it, with refs that
work like any other. To name something inside a frame with a locator,
start from the frame: `frame=<name, URL part or id> >> …`, or the
iframe's ref, `e30 >> text=Pay`. `eval --frame` and `cdp --frame` run in
a frame too.

CSS and the other locators reach into open shadow roots by themselves;
closed shadow roots are out of reach, as they are for the page's own
scripts.
