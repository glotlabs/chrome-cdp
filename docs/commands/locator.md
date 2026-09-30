<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp locator`

Print the locator a test should use for one element (a ref's stable locator)

```text
chrome-cdp locator [OPTIONS] <LOCATOR>
```

Arguments:

- `<LOCATOR>`: The element: a ref (e12) or any locator that matches one

Options:

- `--all`: List every candidate tried, with how many elements each finds
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
Refs last only as long as the session; a committed script or test needs a
locator. This prints the most robust one that finds the element and
nothing else, as `history --as-scenario` writes it: role and name, then
label, test id, placeholder or text; then one of those inside a unique
container (a row, a list item, a dialog); then `>> nth=` as a last
resort. An element in an iframe gets a frame= step first. --all lists
every candidate tried, best first, with how many elements each finds (1
is usable). testid= reads --testid-attribute. Needs a running session.

Examples:
  chrome-cdp locator e12
  chrome-cdp locator e12 --all              # every candidate, with counts
  chrome-cdp locator 'text=Delete >> nth=1'
  chrome-cdp locator e12 -q                 # only the locator
```

