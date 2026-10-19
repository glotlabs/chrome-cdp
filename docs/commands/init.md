<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp init`

Set up a project for chrome-cdp: `init e2e` scaffolds E2E tests with mocks, `init load` a load profile

```text
chrome-cdp init [OPTIONS] <COMMAND>
```

```text
Examples:
  chrome-cdp init e2e
  chrome-cdp init e2e --framework vite --dir web
  chrome-cdp init load
```

## `chrome-cdp init e2e`

Scaffold E2E tests: the [e2e] config, a scenario, a mock set and a CI workflow

```text
chrome-cdp init e2e [OPTIONS]
```

Options:

- `--dir <DIR>`: The project's directory (default: the current one)
- `--framework <NAME>`: The app's framework, instead of the one package.json names (one of `vite`, `next`, `cra`, `astro`, `sveltekit`, `none`)

```text
Writes, in the project directory (the current one, or --dir):

  chrome-cdp.toml               the [e2e] section (added to an existing
                                chrome-cdp.toml or .chrome-cdp.toml)
  e2e/scenarios/example.cdp     a scenario: steps are chrome-cdp commands
  e2e/mocks/base.toml           the mock set every test gets
  e2e/mocks/data/items.json     data the mocks answer with
  .github/workflows/e2e.yml     runs the tests on GitHub Actions
  .gitignore                    ignores e2e/.results/ (test artifacts)

The web server command and base_url come from package.json: Vite, Next.js,
Create React App, Astro or SvelteKit, run with npm, pnpm, yarn or bun (from
packageManager or the lock file). An existing file is never overwritten;
running it again changes nothing. `chrome-cdp help config` lists the [e2e]
keys.

Examples:
  chrome-cdp init e2e
  chrome-cdp init e2e --dir frontend
  chrome-cdp init e2e --framework next      # instead of reading package.json
  chrome-cdp init e2e --json                # each file and what was done
```

## `chrome-cdp init load`

Write an example load profile: e2e/load/example.toml

```text
chrome-cdp init load [OPTIONS]
```

Options:

- `--dir <DIR>`: The project's directory (default: the current one)
- `--name <NAME>`: The profile's name: e2e/load/&lt;NAME&gt;.toml, run as `chrome-cdp load NAME`, e.g. checkout (default `example`)

```text
Writes e2e/load/<--name>.toml in the project (the current directory's, or
--dir's): a load profile for `chrome-cdp load` (docs/e2e/load.md) with
the config's real backend, up to three of the project's tests as
journeys by weight, an open model (users arriving at a rate) and every
other key in comments. An existing file is never overwritten.

Examples:
  chrome-cdp init load
  chrome-cdp init load --name checkout
  chrome-cdp load example --plan      # what the profile would run
```

