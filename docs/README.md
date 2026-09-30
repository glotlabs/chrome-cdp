# chrome-cdp docs

## Start here

- [install.md](install.md): every install channel, `doctor`, `upgrade`,
  macOS Gatekeeper, versioning.
- [guide.md](guide.md): a tour of everything chrome-cdp does, one area at
  a time.
- [commands/](commands/README.md): the command reference, every command
  and flag with examples (generated from the CLI; the same text as
  `chrome-cdp help <command>`).

## Guides

| Guide | For |
|---|---|
| [locators-and-refs.md](locators-and-refs.md) | snapshots, refs and how long they last, the locator grammar, frames and shadow DOM |
| [waiting.md](waiting.md) | what commands wait for by themselves, `wait` for the rest, timeouts, `expect` |
| [debugging-a-dev-server.md](debugging-a-dev-server.md) | the see-why-fix-check loop on a local frontend: errors with source maps, requests, `route` |
| [testing.md](testing.md) | `.cdp` files: `batch`, `run`, `expect` and `chrome-cdp test` |
| [accessibility.md](accessibility.md) | `expect --a11y`: every built-in accessibility rule, with its WCAG criterion |
| [ci.md](ci.md) | GitHub Actions (and other CI) workflows |
| [docker.md](docker.md) | the `ghcr.io/glotlabs/chrome-cdp` image with Chrome and fonts |
| [migrating-from-playwright.md](migrating-from-playwright.md) | Playwright's API, call by call, as chrome-cdp commands |
| [security.md](security.md) | the security model: who can reach the browser, secrets, the sandbox, agents |
| [threat-model.md](threat-model.md) | the threat model behind it: trust boundaries, each defence in the code, what is left |
| [troubleshooting.md](troubleshooting.md) | every error kind: what it means and what to do |

## E2E testing

[e2e/README.md](e2e/README.md): E2E tests of a web frontend against a
mocked backend, and the same scenarios against a real one. Start with the
[quickstart](e2e/quickstart.md).

## Reference

- [output.md](output.md): the output contract, the `--json` envelope,
  exit codes, and what stays stable across releases.
- [scenario-syntax.md](scenario-syntax.md): the `.cdp` grammar.
- [e2e/mocks.md](e2e/mocks.md): every key of a mock file.
- [chrome-flags.md](chrome-flags.md): the flags Chrome is launched with,
  the sandbox and profiles.
- [playwright-parity.md](playwright-parity.md): each Playwright capability
  and its status.
- [benchmarks.md](benchmarks.md): chrome-cdp next to Playwright, and how
  the numbers are made.
- `chrome-cdp help locators|refs|waiting|errors|config|scripts`: the same
  ideas, short, in the terminal.
- `chrome-cdp schema <command>`: the JSON Schema of a command's `--json`
  output.

## For agents

`chrome-cdp skill` prints the agent skill
([source](../crates/chrome-cdp/src/help/SKILL.md)): the workflow, the
pitfalls and a cheat sheet, for Claude Code and other Agent Skills
clients. `chrome-cdp skill --install` adds it to a project.

## Working on chrome-cdp

- [development.md](development.md): building, testing, fixtures, the docs.
- [releasing.md](releasing.md): cutting a release.
- [plans/](plans/00-overview.md): the design and roadmap.
