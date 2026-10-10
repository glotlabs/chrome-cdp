<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp state`

Save or load cookies and storage, e.g. a login

```text
chrome-cdp state [OPTIONS] <COMMAND>
```

```text
A state file is storageState JSON: the cookies, and each origin's
localStorage and sessionStorage, plus IndexedDB with
`state save --indexeddb`. The file holds secrets (session cookies, tokens): it is written readable by you
only; keep it out of version control. For a whole persistent browser
profile instead, use `open --profile <name>`.

Examples:
  chrome-cdp state save auth.json
  chrome-cdp state load auth.json
  chrome-cdp open --state auth.json https://app.example.com
```

## `chrome-cdp state save`

Write the cookies and storage to a file (readable by you only)

```text
chrome-cdp state save [OPTIONS] <FILE>
```

Arguments:

- `<FILE>`: The file to write

Options:

- `--indexeddb`: Also save each origin's IndexedDB databases

```text
Saves the browser's cookies and the storage of every origin the session
visited (in a page or an iframe): localStorage, sessionStorage
and, with --indexeddb, IndexedDB. An origin no tab has open now (a login
provider the app redirected through) is read in a hidden page that gets
an empty document of it; its sessionStorage went with its tab.

Examples:
  chrome-cdp state save auth.json
  chrome-cdp state save auth.json --indexeddb
```

## `chrome-cdp state load`

Load cookies and storage from a storageState file

```text
chrome-cdp state load [OPTIONS] <FILE>
```

Arguments:

- `<FILE>`: The file to read; `-` reads stdin

```text
Adds the file's cookies, and gives each origin its storage: at once if a
tab has it open, else its next page load gets it before the page's
scripts run. The storage of those origins is replaced; other cookies and
origins stay. `open --state <file>` does the same before the first page
loads.

Examples:
  chrome-cdp state load auth.json
  chrome-cdp state load .auth/user.json
```

