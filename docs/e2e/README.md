# E2E testing with chrome-cdp

`chrome-cdp test` runs a web frontend's end-to-end tests against a mocked
backend (fast, deterministic, offline) and, with the same scenario files,
against a real one. Start with the quickstart; the example project
[`examples/e2e-shop`](../../examples/e2e-shop) shows every feature on a
small React app.

| Page | What it covers |
|---|---|
| [quickstart.md](quickstart.md) | `init e2e`, a first mock and test, running, CI |
| [scenarios.md](scenarios.md) | scenario files: directives, `expect request`, the checks the runner adds, running and output |
| [mocks.md](mocks.md) | every key of a mock file (generated from [the JSON Schema](mocks.schema.json)) |
| [recipes.md](recipes.md) | authentication, pagination, file upload, optimistic UI, polling, feature flags, error states |
| [authoring.md](authoring.md) | `history --as-scenario`, `mock record`, `mock import`, `mock check`, `fmt`, `lint` |
| [debugging.md](debugging.md) | artifacts, `report.md`, reporters, `--debug`, `--watch` |
| [determinism.md](determinism.md) | frozen time, seeded random values, no animations, `--repeat`, fonts |
| [healing.md](healing.md) | fingerprints, the likely element of a broken locator, `--heal`, `heal.patch` and `--write` |
| [backends.md](backends.md) | the same scenarios against a real backend, hybrid mode, `--verify-mocks` |
| [beyond-fetch.md](beyond-fetch.md) | WebSockets, Server-Sent Events, `mock serve` for server-side rendering |
| [migrating.md](migrating.md) | from `@playwright/test` and `page.route` |

The workflow for an agent (or a person) that just changed the frontend:
explore the change with the usual commands and `mock use`, save it with
`chrome-cdp history --as-scenario`, add the mocks and data it needs, run
`chrome-cdp test`, and read the `report.md` of anything that failed.
