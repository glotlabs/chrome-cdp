<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp snapshot`

Show the page as an accessibility tree, with refs (e12) to act on

```text
chrome-cdp snapshot [OPTIONS]
```

Options:

- `-i, --interactive`: Only nodes with refs and the landmarks around them (much smaller)
- `--scope <LOCATOR>`: Only this part of the page: a ref (e12) or any locator that matches one element
- `--depth <N>`: Show this many levels; deeper nodes are counted, not shown
- `--diff`: Only what changed since this tab's last snapshot: + added, - removed, ~ changed, with the nodes around them for context
- `--all-refs`: A ref on every node, not only on those an agent acts on
- `--all`: Do not collapse long lists
- `--text-limit <N>`: Cut text and names after this many characters (0 = no limit) (default `200`)
- `--no-text`: Leave text out: the page's skeleton
- `--engine <ENGINE>`: How to read the page: ax (Chrome's accessibility tree, the default) or dom (chrome-cdp's DOM walker) (one of `ax`, `dom`)
- `--strict-refs`: A --scope ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
One node per line, two spaces per level:
  - role "name" [ref] (hN) (password|file) value="…" states… → href
Refs go on what can be acted on (controls, links, clickable elements,
scrollable boxes, iframes); a node keeps its ref while it stays in the
page. Hidden content is left out, lists longer than 30 collapse, and text
is cut at --text-limit. Needs a running session.

Examples:
  chrome-cdp snapshot
  chrome-cdp snapshot -i                  # only what can be acted on
  chrome-cdp snapshot --scope e12         # one part of the page
  chrome-cdp snapshot --scope '#results' --depth 3
  chrome-cdp snapshot --scope 'frame=payment'
  chrome-cdp snapshot --diff              # what changed since the last one
```

