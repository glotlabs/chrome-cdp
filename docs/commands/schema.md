<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp schema`

Print the JSON Schema of a command's --json output

```text
chrome-cdp schema [OPTIONS] [COMMAND]...
```

Arguments:

- `<COMMAND>`: The command, e.g. `version` or `tab close`; omit it to list them

```text
Examples:
  chrome-cdp schema                 # list the commands that have one
  chrome-cdp schema version         # the schema of chrome-cdp --version --json
  chrome-cdp schema scenario        # .cdp directives, commands, flags, variables
  chrome-cdp schema mocks           # the mock file format
```

