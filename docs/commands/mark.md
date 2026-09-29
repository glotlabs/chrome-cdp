<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp mark`

Name this point in the tab's console and network logs, for expect --no-errors --since

```text
chrome-cdp mark [OPTIONS] <NAME>
```

Arguments:

- `<NAME>`: The mark's name, e.g. before-save

```text
A mark belongs to the tab (the active one, or --tab). Setting a name again
moves it. `expect --no-errors --since <name>` and `expect
--no-failed-requests --since <name>` look only at what happened after it,
as do `console`, `errors` and `network` with `--since <name>`. A name
starts with a letter: letters, digits, -, _ and . follow.

Examples:
  chrome-cdp mark before-save
  chrome-cdp click 'role=button[name="Save"]'
  chrome-cdp expect --no-errors --since before-save
  chrome-cdp network --since before-save
```

