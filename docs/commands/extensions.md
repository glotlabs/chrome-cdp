<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp extensions`

List, load, reload or uninstall unpacked extensions; open an extension's popup; read or write its chrome.storage

```text
chrome-cdp extensions [OPTIONS] [COMMAND]
```

Options:

- `--max-chars <N>`: Cut the list at this many characters (0 = no limit) (default `20000`)

```text
Without a subcommand, lists the unpacked extensions the browser has
loaded: id, name, version and directory. Start a browser with one with
`open --extension DIR`. Name an extension by its id or its name.

Examples:
  chrome-cdp extensions
  chrome-cdp extensions load ./my-extension
  chrome-cdp extensions reload 'My Extension'
  chrome-cdp extensions popup 'My Extension'
  chrome-cdp extensions storage 'My Extension' get
```

## `chrome-cdp extensions load`

Load an unpacked extension into the running browser

```text
chrome-cdp extensions load [OPTIONS] <DIR>
```

Arguments:

- `<DIR>`: The unpacked extension's directory (the one holding its manifest.json)

```text
Its content scripts run in pages loaded after it, not in those already
open: reload them. Loading the directory of a loaded extension reloads it.

Examples:
  chrome-cdp extensions load ./my-extension
  chrome-cdp open --extension ./my-extension   # load it at launch instead
```

## `chrome-cdp extensions reload`

Load an extension again from its directory, after editing its files

```text
chrome-cdp extensions reload [OPTIONS] <ID|NAME>
```

Arguments:

- `<ID|NAME>`: The extension's id or name, as `extensions` lists them

```text
It keeps its id and its chrome.storage, as the Reload button of
chrome://extensions does. Pages its content scripts ran in keep the old
scripts until they load again; its popup and other pages close.

Examples:
  chrome-cdp extensions reload 'My Extension'
  chrome-cdp extensions reload dealjoeccjhpbcmihadkoaebbihfdicd
```

## `chrome-cdp extensions uninstall`

Uninstall an unpacked extension (its storage goes with it)

```text
chrome-cdp extensions uninstall [OPTIONS] <ID|NAME>
```

Arguments:

- `<ID|NAME>`: The extension's id or name, as `extensions` lists them

```text
Examples:
  chrome-cdp extensions uninstall 'My Extension'
```

## `chrome-cdp extensions popup`

Run an extension's action on the tab, which opens its popup

```text
chrome-cdp extensions popup [OPTIONS] <ID|NAME>
```

Arguments:

- `<ID|NAME>`: The extension's id or name, as `extensions` lists them

```text
Clicks the extension's toolbar button for the active tab (or --tab). Its
popup opens as a new tab, which `tabs` lists and `tab` switches to. An
action without a popup runs its chrome.action.onClicked listener instead.

Examples:
  chrome-cdp extensions popup 'My Extension'
  chrome-cdp extensions popup 'My Extension' --tab t2
```

## `chrome-cdp extensions storage`

Read or write an extension's chrome.storage

```text
chrome-cdp extensions storage [OPTIONS] <ID|NAME> [OP]
```

Arguments:

- `<ID|NAME>`: The extension's id or name, as `extensions` lists them

```text
Without a subcommand, prints what the area holds, as `get` does. --area
picks the area: local (the default), sync, session, or managed (set by
policy; read-only).

Examples:
  chrome-cdp extensions storage 'My Extension'
  chrome-cdp extensions storage 'My Extension' get token --area session
  chrome-cdp extensions storage 'My Extension' set '{"onboarded": true}'
  chrome-cdp extensions storage 'My Extension' clear
```

### `chrome-cdp extensions storage get`

Print the items of some keys, or all of them, as JSON

```text
chrome-cdp extensions storage <ID|NAME> get [OPTIONS] [KEY]...
```

Arguments:

- `<KEY>`: The keys to print; every item without

```text
Examples:
  chrome-cdp extensions storage 'My Extension' get
  chrome-cdp extensions storage 'My Extension' get token user --area sync
```

### `chrome-cdp extensions storage set`

Merge the items of a JSON object into the area

```text
chrome-cdp extensions storage <ID|NAME> set [OPTIONS] <JSON>
```

Arguments:

- `<JSON>`: A JSON object of the items to set; `-` reads it from stdin

```text
Keys not in the object keep their values; `remove` takes keys out.

Examples:
  chrome-cdp extensions storage 'My Extension' set '{"theme": "dark", "count": 3}'
  chrome-cdp extensions storage 'My Extension' set - < seed.json
```

### `chrome-cdp extensions storage remove`

Remove some keys from the area

```text
chrome-cdp extensions storage <ID|NAME> remove [OPTIONS] <KEY>...
```

Arguments:

- `<KEY>`: The keys to remove

```text
Examples:
  chrome-cdp extensions storage 'My Extension' remove token
```

### `chrome-cdp extensions storage clear`

Remove every item from the area

```text
chrome-cdp extensions storage <ID|NAME> clear [OPTIONS]
```

```text
Examples:
  chrome-cdp extensions storage 'My Extension' clear
  chrome-cdp extensions storage 'My Extension' clear --area session
```

