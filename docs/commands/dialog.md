<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp dialog`

Answer the JavaScript dialog (confirm, prompt, …) the page waits on

```text
chrome-cdp dialog [OPTIONS] <COMMAND>
```

```text
By default an alert, and a beforeunload ("leave the page?"), are accepted
at once; a confirm or prompt waits for this command, and the command that
ran into it fails with exit code 5 (see `open --dialogs`).

Examples:
  chrome-cdp dialog accept
  chrome-cdp dialog dismiss
  chrome-cdp dialog accept 'Ada Lovelace'     # a prompt's answer
```

## `chrome-cdp dialog accept`

Accept the dialog (OK); a prompt gets TEXT, else its default

```text
chrome-cdp dialog accept [OPTIONS] [TEXT]
```

Arguments:

- `<TEXT>`: The answer to a prompt; `-` reads it from stdin

```text
Examples:
  chrome-cdp dialog accept
  chrome-cdp dialog accept 'Ada Lovelace'     # a prompt's answer
  chrome-cdp dialog accept --tab t2
```

## `chrome-cdp dialog dismiss`

Dismiss the dialog (Cancel; stay on the page for beforeunload)

```text
chrome-cdp dialog dismiss [OPTIONS]
```

```text
Examples:
  chrome-cdp dialog dismiss
  chrome-cdp dialog dismiss --tab t2
```

