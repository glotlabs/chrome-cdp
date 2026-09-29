<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp find`

Search the page for elements by role, name or text; shows refs

```text
chrome-cdp find [OPTIONS] <QUERY>...
```

Arguments:

- `<QUERY>`: Words to look for; `-` reads them from stdin

Options:

- `--role <ROLE>`: Only nodes with this role (button, link, textbox, heading, …)
- `--scope <LOCATOR>`: Only search this part of the page: a ref (e12) or any locator that matches one element
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)
- `--limit <N>`: Show at most this many matches, best first (0 = all) (default `10`)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Every word must match, in any order: a part of a node's name, value or
text (any case), its role (button, link, textbox, …), or, for words of
five letters or more, a word of the name one typo away. The best matches
come first, as snapshot lines with refs and the named container each is
in. Cheaper than a whole snapshot when you know what you want. Needs a
running session.

Examples:
  chrome-cdp find checkout
  chrome-cdp find 'sign in' --role button
  chrome-cdp find email textbox
  chrome-cdp find delete --scope e40        # only inside e40
  chrome-cdp find delete --limit 0          # every match
  chrome-cdp find checkout -q               # only the refs
```

