<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp components`

Print the page's component tree, or the component that rendered an element, with its props and state

```text
chrome-cdp components [OPTIONS] [LOCATOR]
```

Arguments:

- `<LOCATOR>`: The element whose component is shown: a ref (e12) or a locator that matches one; the whole tree if omitted

Options:

- `--depth <N>`: The tree's levels (default 4), or with a locator the owners listed (default 8; the rest are counted)
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Without a locator: the component tree from each root of the app's dev
build, --depth levels (default 4), each component at its own file and
the line of the first element it renders, with its props one level deep
(`items=[3 items]`; functions left out). Siblings with the same name,
place, props and components below are folded (`<CartItem> ×3`).

With a locator: the component that rendered the element, its props and
state in full (within the limits), and its owners, innermost first, at
most --depth (default 8). State is a React function component's
useState/useReducer hooks by index (`#0`), a class component's state,
Vue's setupState (refs unwrapped) and data; Svelte 5 has no public API
for it. Values are previewed in the page: 2 levels deep, strings cut at
80 characters, collections over 5 entries counted (`[12 items]`,
`{…8 keys}`), functions as `ƒ`.

Reads React 18 and 19, Vue 3 and Svelte 5 dev builds (the page's debug
info, which it could fake); a production build fails with NoSource.
Places are files relative to the project root, as source prints them.
-q prints the tree without the last line, or the component's place.
Needs a running session.

Examples:
  chrome-cdp components
  chrome-cdp components --depth 2
  chrome-cdp components e12               # the component behind e12
  chrome-cdp components 'role=button[name="Add"] >> nth=0' --json
```

