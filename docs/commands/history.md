<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp history`

The commands the session ran; --as-scenario writes them as a test

```text
chrome-cdp history [OPTIONS]
```

Options:

- `--as-scenario <NAME>`: Print the commands as a test with this name (`@test "NAME"`)
- `--from <N>`: Start at command N (as `history` numbers them)
- `--clear`: Forget the commands so far (after printing them)
- `--limit <N>`: Without --as-scenario: show at most this many commands, the newest (0 = all) (default `50`)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
The session keeps its last 1,000 commands (from any shell, `batch` and
`run`), each with whether it succeeded. --as-scenario NAME prints them as
a test for a scenario file, the core authoring workflow: explore with the
usual commands, then save what worked.

In the test, refs (e12) become the most robust locator that found their
element alone when the command ran: role and name, then label, test id
(testid=…, read with the config's testid_attribute, as the project's
`test` reads it), placeholder or text, then one of those inside
a named container (a list item, a row, a dialog), and `>> nth=` only as
a last resort. An element in an iframe gets frame=NAME (or a part of its URL only it has) first:
frame=pay >> role=button[name="Pay"]. Commands that only read (snapshot, console, text, network, …) and failed ones are left
out; `expect` commands stay; `open URL` becomes `goto`; URLs under
base_url become paths; `mock use SETS` becomes `@mocks SETS`. --from N
starts at command N (see `history`), and --clear starts the history over
(after printing it).

Examples:
  chrome-cdp history
  chrome-cdp history --as-scenario "adds item to cart" >> e2e/scenarios/cart.cdp
  chrome-cdp history --as-scenario checkout --from 12
  chrome-cdp history --clear -q
```

