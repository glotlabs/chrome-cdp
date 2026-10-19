# The `.cdp` script syntax

Version 1.

`chrome-cdp batch` (reading stdin) and `chrome-cdp run <file.cdp>` read
this syntax. Plan 11's test scenarios use it too, with extra directives.
A script is a list of `chrome-cdp` command lines without the program name,
in the quoting you would type into a shell:

```
@syntax 1
# Log in and check the dashboard.
goto /login
fill label=Email ${email}
fill label=Password "${env:APP_PASSWORD}"
click 'role=button[name="Sign in"]'
wait --url /dashboard
screenshot -o /tmp/dashboard.png
```

The parser is `protocol::script` (`crates/protocol/src/script/`). Every
rule below has a test in `crates/protocol/src/script/tests.rs`, in the
same order.

## Grammar

EBNF (ISO 14977 style; `(* … *)` are comments). The grammar describes one
physical line at a time. The structure rules below the grammar join lines
with a continuation and nest them by indentation.

```ebnf
script        = { line } ;
line          = indent , [ content ] , [ ws ] , [ comment ] , eol ;
content       = directive | command ;
command       = word , { ws , word } , [ continuation ] ;
directive     = "@" , name , { ws , word } , [ continuation ] ;
continuation  = ws , "\" ;            (* last characters of the line *)

indent        = { " " } ;             (* spaces only *)
ws            = ( " " | tab ) , { " " | tab } ;
comment       = "#" , { any - eol } ; (* starts a line or follows ws *)
eol           = "\n" | "\r\n" | end of input ;

word          = piece , { piece } ;   (* not starting with "#" *)
piece         = bare | single | double | variable | "$${" | "$" ;
bare          = bare char , { bare char } ;
bare char     = ( any - ( ws | eol | "'" | '"' | "\" | "$" ) )
              | "\" , ( any - eol ) ;  (* the escaped character itself *)
single        = "'" , { any - ( "'" | eol ) } , "'" ;
double        = '"' , { dq char | variable | "$${" | "$" } , '"' ;
dq char       = ( any - ( '"' | "\" | "$" | eol ) )
              | '\"' | "\\" | "\n"
              | "\" , ( any - ( '"' | "\" | "n" | eol ) ) ; (* kept as is *)

variable      = "${" , ( path | namespaced ) , "}" ;
path          = ident , { "." , ident } ;
namespaced    = "env:" , env name
              | "var:" , ident
              | "secret:" , ident
              | "data:" , file , [ "#" , pointer ] ;
file          = char - ( "#" | "}" ) , { char - ( "#" | "}" ) } ;
pointer       = { "/" , { char - ( "/" | "}" ) } } ; (* RFC 6901 *)

ident         = letter | "_" , { letter | digit | "_" | "-" } ;
env name      = letter | "_" , { letter | digit | "_" } ;
name          = lower , { lower | digit | "-" } ;
```

## Lines

- One command per line. Blank lines (only spaces and tabs) are ignored.
- A line ends with `\n` or `\r\n`; the last line needs no line break.
- Words are separated by runs of spaces and tabs.
- Encoding is UTF-8. Columns in error positions count characters, not bytes.

## Indentation

- Indentation is spaces only. A tab in a line's indentation is an error;
  tabs between words are fine.
- Top-level lines start at column 1.
- A directive's body is the lines right below it indented **exactly 2
  spaces** more than the directive. Bodies nest (a directive inside a body
  has its own body 2 spaces deeper). The body ends at the first line
  indented less.
- Any other indentation is an error: a command has no body, and a body is
  not indented 4 spaces. One error covers the whole wrongly indented block.
- Comment lines may be indented any amount. A comment line indented less
  than the current body ends that body.

## Comments

- `#` starts a comment when it begins a line (after the indentation) or
  follows whitespace. The comment runs to the end of the line.
- A `#` inside a word (`css=#go`, `x#y`) or inside quotes is text.
- As in a shell, a bare `#submit` after a space is a comment. Quote it:
  `click '#submit'`, or escape it: `click \#submit`.
- A comment never continues onto the next line, even if it ends in `\`.

## Quoting

Quoting is POSIX-shell-like, with fewer features:

- **Single quotes** are literal: nothing inside them is special, not even
  `\` or `${`.
- **Double quotes** allow `\"`, `\\` and `\n` (a line break), and expand
  `${…}` variables. Any other backslash stays as written, so `"\d+"` is
  the four characters `\d+`.
- **Outside quotes**, a backslash escapes the next character (`\#`, `\ `,
  `\'`).
- Quoted and bare pieces join into one word: `role=button[name="Sign in"]`
  is one word, `role=button[name=Sign in]`.
- `''` and `""` are an empty word.
- A quote must close on the same line. There are no multi-line strings;
  pass long text with `\n` or from a file.
- Nothing else from the shell exists: no globbing (`*.png` is text), no
  pipes or `;`, no `$(…)` or backticks, no `$NAME` (only `${NAME}`), no
  `~` expansion.

## Line continuation

- A line ending in a space (or tab) and a backslash, ` \`, continues on the
  next line. The next line's indentation is ignored, so continued words can
  be lined up freely.
- The backslash must follow whitespace: `word\` at the end of a line is an
  error, so a continuation is never mistaken for an escape.
- A continuation needs a next line that is not blank, and it cannot be the
  last line of the file.
- The continued line is one command: `fill label=Email \` + `ada@x.com` is
  `fill label=Email ada@x.com`.

## Variables

`${…}` is replaced by a value in bare words and double quotes, never in
single quotes. The value is always part of the one word it was written in:
a value with spaces does not split into several words, and quotes or `#`
inside it are not interpreted.

| Form | Value | Available in |
|---|---|---|
| `${name}` | `--var name=value` of `batch` and `run`; plan 11's `@include` parameters | `batch`, `run`, `test` |
| `${env:NAME}` | the environment variable `NAME`; unset is an error, set-but-empty is `""` | `batch`, `run`, `test` |
| `${data:file.json#/a/0}` | the value at a JSON pointer in a JSON file (relative to the script's folder, or in `test` to the folder of the scenario or step file the line is in; the whole file without `#`). Strings are used as is, other values as compact JSON | `batch`, `run`, `test` |
| `${var:name}` | the run's backend's vars file ([docs/e2e/backends.md](e2e/backends.md)) | `test` |
| `${secret:NAME}` | the environment variable `NAME`, masked in all output and artifacts | `test` |
| `${saved.x.y}` | field `y` of the JSON answer an earlier `http … --save x` of the test (or of its file's `@before-all`) got; a number picks an item of a list (`${saved.x.items.0.sku}`) | `test` |
| `${test.id}`, `${run.id}` | unique ids for the test's try and for the run (`k3j9x0qa-4`, `k3j9x0qa`) | `test` |
| `${backend}` | the run's backend (`mock`, `local`, …), unless an `@include` parameter has the name | `test` |
| `${vu.id}`, `${iteration.id}` | unique ids for a load run's user and its iteration (`k3j9x0qa-u12`, `k3j9x0qa-u12-3`); under `test`, the one user `${run.id}-u1` and `${test.id}` | `load`, `test` |
| `${row.COLUMN}` | a column of the row the iteration took from its journey's `data` ([docs/e2e/load.md](e2e/load.md#per-user-data)); checked against the data's columns before the run | `load` |

- `$${` is a literal `${`: `$${x}` is the text `${x}`.
- A `$` not followed by `{` is text (`a$b`, `$HOME`).
- Names are `[A-Za-z_][A-Za-z0-9_-]*`; environment names
  `[A-Za-z_][A-Za-z0-9_]*`.
- An unknown kind (`${foo:x}`), an empty `${}` or an unclosed `${` is a
  syntax error. A variable that has no value where the script runs (an
  unknown `--var`, an unset environment variable, a plan 11 variable in
  `run`) is an error before the first step runs.

## Directives

- A line whose first word starts with an unquoted `@` is a directive:
  `@name args…`, optionally with a body (see indentation). Names are
  lowercase letters, digits and `-`. `@` later in a line, or quoted
  (`'@x'`), is an ordinary word.
- Arguments are words, with the same quoting and variables as commands.
- **Unknown directives are errors.** Each runner lists the ones it knows:
  - `batch` and `run` know only `@syntax`;
  - plan 11's `test` adds `@test`, `@before`, `@after`, `@before-all`,
    `@after-all`, `@include`, `@mocks`, `@tags`, `@skip`, `@only`,
    `@timeout`, `@viewport`, `@device`, `@retries` and
    `@allow-console-errors`
    ([docs/e2e/scenarios.md](e2e/scenarios.md)), and `@backend` with
    plan 11.11.
- `@syntax N` declares the syntax version. It must come before any command
  or other directive (comments may precede it), takes one number, and has
  no body. Without it, a file is read as the current version. A newer
  version than the build reads is an error that says to upgrade.

## Command lines

- A command line is exactly what follows `chrome-cdp` in a shell, after
  quoting: `click e6 --force`, `--tab t2 goto /`. Global flags work on any
  line.
- Each line inherits the global flags of the `batch` or `run` call
  (`--session`, `--timeout`, `--tab`, `--json`, `-q`); a flag on the line
  wins for that line. `--json` on a line of a text batch is an error: set it
  on the `batch` or `run` call.
- Every line is checked against the command definitions before the first
  one runs, so a typo (`clik e6`, `--forse`) fails without touching the
  browser.
- `batch`, `run` and `test` cannot be nested in a script. Reuse comes
  from `@include` (plan 11).
- In a scenario, each line also runs in the test's own browser context
  and session (`--context`, `--session`); see
  [docs/e2e/scenarios.md](e2e/scenarios.md).
- A text argument of `-` (read stdin) is not available in `batch`, whose
  stdin is the script.

## No control flow

There are no loops, conditionals, functions or expressions, by design.
Reuse happens through `@include` with parameters (plan 11). Anything more
complex belongs in a shell script or program that calls `chrome-cdp`.
This keeps scenarios readable in diffs and easy for agents to write.
Adding control flow needs a decisions-log entry in plan 08 that argues
against this section.

## Errors

Every problem names its place as `file:line:column` (`<stdin>` for
`batch`), like a compiler:

```
error: flow.cdp:12:5: a single quote without its closing quote
  hint: close it on the same line; quotes never span lines
```

Syntax errors are all reported at once; the script does not start while
any remain.

`chrome-cdp lint` checks files without running them (for scenarios, also
refs, sleeps and more; see docs/e2e/authoring.md), `chrome-cdp fmt` lays
them out in one style, and `chrome-cdp schema scenario` prints the
directives, commands, flags and variables as JSON.

## Versioning

The syntax is versioned by `@syntax N`. Changes that would make an existing
valid script mean something else, or stop parsing, need a new version.
Adding a directive, a variable kind or a command does not.
