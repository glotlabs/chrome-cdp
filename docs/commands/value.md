<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp value`

Print a field's current value: input, textarea, select, editable text

```text
chrome-cdp value [OPTIONS] <LOCATOR>
```

Arguments:

- `<LOCATOR>`: The field: a ref (e12) or a locator that matches one

Options:

- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
A select prints the chosen option's value (a multiple select, and a file
input's file names, one per line); an editable element its text; a slider,
spin button, progress bar or meter its value. An element not there yet
is waited for (up to --timeout, 10s by default). Needs a running session.

Examples:
  chrome-cdp value e3
  chrome-cdp value 'label=Email'
  chrome-cdp value '#country'
```

