# Plan 08 — Agent UX, output contract & scripting

Status: TODO (§2–§4 must be agreed before other plans implement commands)
Depends on: 01. Cross-cutting: every plan follows §2–§4.

## Goal

Make `chrome-cdp` the easiest browser tool for an LLM agent to use correctly
on the first try. This plan defines the output contract, errors, exit codes,
help and discoverability, and multi-step scripting. It also covers optional
MCP mode.

## 1. Who the user is

Assume the main user is an agent running shell commands (Claude Code, Codex,
Cursor, …). It:

- sees stdout and stderr, often truncated at around 30k characters;
- pays for every token;
- can't press keys interactively;
- retries on errors;
- learns the tool from `--help` and a skill file.

Humans are the second user; they get colour and progress bars only on a TTY.

## 2. Output contract

- **stdout** carries the result. **stderr** carries warnings, notes and
  errors. The two never mix.
- Default text output is plain ASCII-friendly UTF-8: no ANSI codes unless
  stdout is a TTY (and `NO_COLOR` is unset), no spinners, no tables wider
  than 120 columns.
- Every command is bounded by default. Anything unbounded (snapshot, text,
  html, console, network, eval results) has a default `--max-chars` or
  `--limit`. On truncation, the output ends with one line saying how to get
  more (`… truncated 18,203 chars; use --scope e12 or --max-chars 50000`).
- **`--json`** is available on every command. It prints one JSON object on
  stdout:
  `{"ok":true,"data":{…},"warnings":[…],"events":[…]}` or
  `{"ok":false,"error":{"kind":"Obscured","message":"…","hint":"…","details":{…}}}`.
  Schemas live in `crates/protocol` and are exported as JSON Schema by
  `chrome-cdp schema <command>`. They are stable from v1; add fields only.
- Paths in output are absolute. Durations use human units (`340ms`,
  `1.2s`). Sizes use `12KB`.
- `-q/--quiet` prints only the essential result (a ref, a path, a value),
  for `$(…)` capture.

## 3. Errors & exit codes

| Exit | Meaning | Example kinds |
|---|---|---|
| 0 | success | |
| 1 | action/assertion failed (page-level) | `NotFound`, `Ambiguous`, `Obscured`, `NotEnabled`, `RefStale`, `RefDetached`, `ExpectFailed`, `EvalError`, `NavigationFailed` |
| 2 | usage error | `BadArgs`, `BadLocator` |
| 3 | timeout | `Timeout` (always with observed state) |
| 4 | browser/daemon/environment | `ChromeNotFound`, `LaunchFailed`, `BrowserCrashed`, `DaemonUnavailable`, `ProtocolError` |
| 5 | dialog blocking | `DialogOpen` |

Every error has:

- a one-line `message`;
- a `hint` that names the concrete next command;
- `details` (candidates, observed URL/title, obstructing ref, …).

Text form:

```
error: element e6 is covered by another element
  covering: div "We use cookies" [e88]
  hint: chrome-cdp click e89   (button "Accept" inside the banner)
```

Hints are unit-tested (golden files). Wording is part of the UX: review it
like an API.

## 4. Command conventions

- Verb-first, flat commands (`click`, `fill`) for the hot path. Nouns with
  subcommands (`tab`, `route`, `state`) for resource management.
- Global flags: `--session`, `--tab`, `--timeout`, `--json`, `--quiet`,
  `--snap`, `--no-wait-after`, `-v`.
- Every global flag also has a `CHROME_CDP_*` env var. The config file is
  `~/.config/chrome-cdp/config.toml` (or macOS
  `~/Library/Application Support/chrome-cdp/config.toml`), plus a project-local
  `.chrome-cdp.toml` found by walking up from the current directory. That lets
  a repo pin its `base_url`, viewport and so on.
- `base_url` in the project config lets `goto /login` work.
- Positional text arguments take `-` for stdin (for easy quoting).
- Commands are idempotent where it makes sense: `open` on an existing session
  is a no-op, and `check` on a checked box is a no-op that says so.

## Milestone 8.1 — Implement the contract

- Crate `protocol`: the `ErrorKind` enum → exit code; the `Hint` builder; and
  `Output` types with text and JSON renderers implemented once and shared.
- Tests: a golden test per command for text and `--json` output, from a shared
  fixture run.

## Milestone 8.2 — Batch & scripts

- `chrome-cdp batch` reads commands from stdin, one per line, in the same
  syntax as the CLI without the program name. `#` starts a comment.
  - It runs them sequentially in one daemon round trip stream and stops at the
    first error unless `--continue`.
  - Output is per step: `[3/7] click e6 ✓` plus the step's normal output. With
    `--json`, it prints one JSON line per step.
  - This cuts agent round trips: a login flow is one call.
- `chrome-cdp run <file.cdp> [--var key=value]` runs the same syntax from a
  file, with `${var}` and `${env:VAR}` substitution. It is the basis for plans
  09 and 11.
- Shell-style quoting.

### The `.cdp` format is ours, so specify it

`.cdp` is a format we invented (the closest prior art is Hurl, Maestro and
Robot Framework). Treat it as a specified format from day one:

- **Grammar document:** `docs/scenario-syntax.md` has an EBNF grammar and
  prose rules for:
  - lines and indentation (spaces only; a directive's body is indented
    exactly 2 spaces);
  - comments (`#` at the start of a line or after whitespace, never inside
    quotes);
  - quoting (POSIX-shell-like: single quotes are literal, double quotes allow
    `\"` `\\` `\n` and `${…}`; no other shell features such as globbing,
    pipes or `$(…)`);
  - line continuation (a trailing ` \`);
  - variables (`${name}`, `${env:X}`, `${var:x}`, `${data:file#/ptr}`,
    `${saved.x.y}`, `${test.id}`, `${run.id}`; `$${` escapes);
  - directives (`@name args…`; plan 11 defines the test directives, and
    unknown directives are errors).

  The grammar is versioned. A file may start with `@syntax 1`; without it,
  the file is assumed to be the current version.
- **Parser:** a hand-written lexer and parser in the `protocol` crate (not
  `shlex`, which has no source positions). It produces an AST where every node
  carries a **span** (file, line, column). Every later error (unknown
  command, bad flag, failed step) points to `file.cdp:12:5`. Command lines are
  validated against the clap definitions at parse time, so a typo fails
  before the browser starts.
- **No control flow, by design:** no loops, conditionals, functions or
  expressions. Reuse happens through `@include` with parameters (plan 11).
  Anything more complex belongs in a shell or program that calls the CLI. This
  keeps scenarios readable in diffs and writable by agents. Adding control
  flow needs a decisions-log entry that argues against this paragraph.

## Milestone 8.3 — Help & discoverability

- `chrome-cdp --help` is short: the top 15 commands grouped by task plus the
  locator grammar in 5 lines. Full reference: `chrome-cdp help <command>` and
  `chrome-cdp help locators|refs|waiting|errors|config`.
- Every command's help has **one example** at least.
- Typos get "did you mean" suggestions (`strsim`) for commands, flags and
  device names.
- `chrome-cdp skill` prints `SKILL.md`: a Claude Code/Agent Skills compatible
  file (frontmatter name/description + body) that teaches the workflow:
  `open → snapshot -i → act by ref → read the action report → errors/console
  when stuck`, the common pitfalls, and a cheat sheet under 2k tokens.
  `chrome-cdp skill --install [--project|--user]` writes it to
  `.claude/skills/chrome-cdp/SKILL.md` (or the user dir).
  Also generate `AGENTS.md` snippet output and `llms.txt`.
- `chrome-cdp completions <shell>` (clap_complete).

## Milestone 8.4 — MCP server mode (v1 stretch, likely v1.1)

- `chrome-cdp mcp` runs a stdio MCP server that exposes the same commands as
  tools (generated from the clap definitions, so it can't drift). Tool results
  use the same text renderer. The command shares the daemon, so a CLI user and
  an MCP client can look at the same session.
- It is behind a Cargo feature `mcp` (`rmcp` crate) to keep the default binary
  lean.

## Acceptance criteria

- [ ] Every command obeys §2–§4 (a checklist test iterates all clap commands
      and asserts `--json` works, help has an example, and output is bounded).
- [ ] A fresh Claude Code session given only `chrome-cdp skill` completes the
      plan 10 agent benchmark tasks without reading other docs.
- [ ] A `batch` of 10 steps runs ≥ 3× faster than 10 separate invocations.
- [ ] `docs/scenario-syntax.md` exists, and the parser's test suite includes
      one test per grammar rule, plus a fuzz target
      (`cargo fuzz`: never panics, and spans are always within the input).

## Decisions log

_(append here)_
- (from plan 01, milestone 1.4) The `protocol` crate already has `ErrorKind` with the §3 exit codes, `ErrorInfo` and the `Envelope`, and the binary renders all output through `output::render`. Milestone 8.1 builds on these instead of starting over. Open question left for 8.1: how `--json` output stays bounded (1.4 only bounds the text forms).
