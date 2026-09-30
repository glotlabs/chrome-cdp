# Output, `--json` and stability

Every command follows one output contract, so a script or an agent can
rely on it without knowing the command. This page is that contract, and
what chrome-cdp promises to keep stable across releases.

## Streams and forms

- **stdout** carries the result; **stderr** carries warnings, notes and
  errors. The two never mix.
- The **text** form (the default) is for reading: no colour unless stdout
  is a terminal and `NO_COLOR` is unset, bounded in size, ending with one
  line on how to see more when it was cut.
- **`-q`** prints only the essential value (a ref, a path, a count), for
  `$(…)` capture.
- **`--json`** prints one JSON object on stdout, described below. `batch`
  and `run` print one object per step (`chrome-cdp schema batch`).

## The envelope

```json
{"ok":true,"data":{},"warnings":["…"],"events":[{"type":"navigated"}],"truncated":[]}
{"ok":false,"error":{"kind":"Obscured","message":"…","hint":"…","notes":[],"details":{}}}
```

| Field | When | What |
|---|---|---|
| `ok` | always | whether the command succeeded |
| `data` | `ok` is true | the result; its shape is the command's schema |
| `error` | `ok` is false | `kind`, a one-line `message`, the next command to try in `hint`, labelled `notes`, structured `details` |
| `warnings` | not empty | non-fatal notes (stderr in the text form) |
| `events` | not empty | what happened in the browser while the command ran, each with a `type`: `tab_opened`, `dialog`, `download`, `navigated`, `console_error`, `issue`, `logpoint`, `hot_update`, `request_failed`, `focus_moved` |
| `truncated` | not empty | each value in `data` cut to fit `--max-chars`: its JSON Pointer `path`, `kind` (`string` or `array`), the original `len` and what was `kept` |

`--json` output stays valid JSON when it is bounded: strings are cut and
arrays lose their tail, never whole fields, and `truncated` says where.
`--max-chars 0` turns the bound off.

`chrome-cdp schema` lists the commands; `chrome-cdp schema <command>`
prints the JSON Schema of that command's envelope (`schema tab close`,
`schema version` for `--version --json`). `schema scenario` and
`schema mocks` describe the `.cdp` and mock file formats.

## Exit codes

| Exit | Meaning |
|---|---|
| 0 | success |
| 1 | the action or assertion failed on the page |
| 2 | usage: bad arguments or locator |
| 3 | timeout (the error says what was observed) |
| 4 | browser, daemon or environment |
| 5 | a dialog is blocking the page |

[troubleshooting.md](troubleshooting.md) has a section for every error
kind.

## Versions and stability

chrome-cdp follows [Semantic Versioning](https://semver.org/). The public
API is:

- the command line: commands, subcommands, arguments, flags, environment
  variables and config keys;
- exit codes and error kinds;
- the `--json` envelope and each command's schema;
- the `.cdp` scenario syntax (versioned by `@syntax N`,
  [scenario-syntax.md](scenario-syntax.md)) and the mock file format.

From 1.0, a minor release may only add to these: a new command, flag,
error kind, event type or optional field. Removing or renaming any of
them, changing a field's type, or changing an exit code needs a major
release. A consumer should ignore fields and event types it does not know.

Not part of the API: the text and `-q` forms' wording and layout, hints,
the order of `warnings`, and anything under `details` that the schema
does not name. `chrome-cdp --version --json` reports the version (and the
CDP protocol revision it was built against); [CHANGELOG.md](../CHANGELOG.md)
lists what each release changed.

Every schema is kept in the repo as a golden file
(`crates/chrome-cdp/tests/golden/schema/`), and so are the commands with
their arguments, flags and environment variables
(`crates/chrome-cdp/tests/golden/cli.txt`), so a change to one fails the
tests until it is reviewed and recorded. Before a release,
`cargo xtask release check` compares them with the previous release's and
fails on a breaking change unless the major version went up
([releasing.md](releasing.md#compatibility-check)).
