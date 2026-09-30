<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp compare`

Compare two pages: what their snapshots differ in, and with --visual their pixels

```text
chrome-cdp compare [OPTIONS] <A> <B>
```

Arguments:

- `<A>`: The first page: a URL, tab:&lt;id&gt;, session:&lt;name&gt; or a snapshot --json file
- `<B>`: The second page, the same kinds as A: what it has that A has not is marked +

Options:

- `-i, --interactive`: Compare only nodes with refs and the landmarks around them
- `--scope <LOCATOR>`: Compare only this part of each page: a locator that matches one element on both
- `--visual`: Compare screenshots of the two viewports too
- `--threshold <T>`: With --visual: how far a pixel's colour may move before it differs (0 to 1) (default `0.1`)
- `--viewport <WxH>`: With --visual: the viewport both pages are shot at, WIDTHxHEIGHT (A's if left out)
- `--exit-code`: Exit 1 when anything differs
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
A and B are each a URL (opened in a new tab of the current context,
compared, then closed), tab:<id> (a tab of this session, as it is),
session:<name> (the active tab of another session), or a .json file
`snapshot --json` wrote (--visual needs a live page). Nodes are matched
by structure, not by ref: siblings line up by role and name, and a node
left over with the same role where another was counts as changed. The
diff reads as snapshot --diff's: + only in B, - only in A, ~ changed
(was …), with the nodes around each change; refs are left out. When the
origins differ (local against production), B's links to its own origin
are read as links to A's, so only real differences show. A block that
moved shows as removed and added.

--visual shoots both viewports at one size (--viewport, else A's; a URL
side gets that size; tabs of different sizes compare the area they
share, with a warning) and groups the pixels that differ past
--threshold (0 to 1, default 0.1) into regions: the 10 largest are
listed, each named by the smallest element of B at its centre (with a
ref when B is a tab or session). The diff image (B faded, changes red,
regions boxed) is saved in the session's shots directory. It exits 0
whatever differs; --exit-code exits 1 when anything does. -q prints how
many differences (changes and regions) there are. Needs a running
session.

Examples:
  chrome-cdp compare http://localhost:5173/cart https://shop.example.com/cart
  chrome-cdp compare tab:t1 tab:t2 -i
  chrome-cdp compare before.json tab:t1 --scope 'role=main'
  chrome-cdp compare session:local session:prod --visual --exit-code
```

