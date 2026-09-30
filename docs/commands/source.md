<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp source`

Print the file and line an element came from, and its components

```text
chrome-cdp source [OPTIONS] <LOCATOR>
```

Arguments:

- `<LOCATOR>`: The element: a ref (e12) or a locator that matches one

Options:

- `--depth <N>`: List at most this many components around it; the rest are counted (default `8`)
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
For the app's own dev build: the element's place (`at`) and the
components around it, innermost first (`in`), each at the line of its
file that renders the next step towards the element; then what answered.
Places are files relative to the project root (the directory of
chrome-cdp.toml or .git, or source_root) when chrome-cdp finds them on
disk, else URLs. It asks, in order: the framework's debug info in the
page (React 18 and 19, Vue 3, Svelte 5 dev builds; the page could fake
it), the stack Chrome recorded when a script made the element (node
stacks: on for localhost pages, see open --node-stacks), and the
document's HTML. When nothing answers it fails with NoSource and says
why. -q prints the place alone. Needs a running session.

Examples:
  chrome-cdp source e12
  chrome-cdp source 'role=button[name="Add to cart"]' --depth 3
  $EDITOR "$(chrome-cdp source e12 -q)"
```

