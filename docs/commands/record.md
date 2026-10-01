<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp record`

Record what happens in the page as test steps

```text
chrome-cdp record [OPTIONS] <COMMAND>
```

```text
`record start` listens to the page itself, in every tab and iframe of the
session (or --context), so it records whoever acts: a person in a headed
window, an agent's chrome-cdp commands, or raw `cdp Input.*` events.
`record stop` prints the recorded test (or appends it to -o FILE): clicks,
checks, typing (one `fill` per field), choices, files, keys, browser
navigations, dialogs answered and tab switches, each element by the same
stable locators `history --as-scenario` writes. It guesses no assertions:
add `expect` steps. docs/e2e/recording.md has the details.

Examples:
  chrome-cdp record start --name "adds a mouse to the cart"
  chrome-cdp record status
  chrome-cdp record stop -o e2e/scenarios/cart.cdp
```

## `chrome-cdp record start`

Start recording what happens in the page

```text
chrome-cdp record start [OPTIONS]
```

Options:

- `--name <NAME>`: The recorded test's name (`@test "NAME"`) (default `recorded`)

```text
From now on every page and iframe of the session's tabs (or --context's)
reports what is done in it, from the active tab on. Each element is named
at the moment it is acted on, by the most robust locator that finds it
alone (testid= for the configured testid_attribute). Starting again drops
the earlier recording. Needs a running session.

Examples:
  chrome-cdp record start
  chrome-cdp record start --name "signs in"
```

## `chrome-cdp record status`

How many steps the recording has so far

```text
chrome-cdp record status [OPTIONS]
```

```text
Typing still going on counts as one step.

Examples:
  chrome-cdp record status
  chrome-cdp record status -q     # just the number
```

## `chrome-cdp record stop`

Stop recording and print the test, or append it to a file

```text
chrome-cdp record stop [OPTIONS]
```

Options:

- `-o, --output <FILE>`: Append the test to this file (made if missing) instead of printing it
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Prints `@test "NAME"` and its steps, ending with a comment asking for
`expect` steps (the recorder guesses no assertions). URLs under base_url
become paths. A password field's value is never recorded: the step fills
${secret:PASSWORD}. A file chosen gets `upload LOC NAME` and a TODO for
its path. With -o, the test is appended to FILE (made if missing).

Examples:
  chrome-cdp record stop
  chrome-cdp record stop -o e2e/scenarios/signin.cdp
  chrome-cdp test e2e/scenarios/signin.cdp
```

