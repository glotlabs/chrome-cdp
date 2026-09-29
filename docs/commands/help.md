<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp help`

Show help: an overview, every command by task, a command in full, or a topic

```text
chrome-cdp help [OPTIONS] [TOPIC]...
```

Arguments:

- `<TOPIC>`: A topic (locators, refs, waiting, errors, config, scripts), `commands`, or a command (`click`, `tab close`)

```text
Topics: locators, refs, waiting, errors, config, scripts. `help commands`
lists every command by task; `help <command>` is the command's --help.

Examples:
  chrome-cdp help                  # the overview, as --help
  chrome-cdp help commands
  chrome-cdp help locators
  chrome-cdp help tab close
```

