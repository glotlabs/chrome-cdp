# Real Playwright suites for `chrome-cdp convert`

Unmodified copies of small, permissively licensed Playwright test suites.
`convert` is tuned on them (plan 18, milestone 18.7): the golden tests
in `crates/chrome-cdp/tests/golden/convert-*.txt` keep what each file
converts to, TODO lines included. Nothing here is run as it is; the
suites need their own apps and helpers.

| Folder | Source | License |
|---|---|---|
| `playwright-todomvc` | [microsoft/playwright](https://github.com/microsoft/playwright) `examples/todomvc/tests/integration.spec.ts` at v1.48.0 (0cdbb110) | Apache-2.0, `playwright-todomvc/LICENSE` |
| `playwright-examples` | [microsoft/playwright](https://github.com/microsoft/playwright) `examples/mock-battery/tests/` and `examples/svgomg/tests/` at v1.48.0 (0cdbb110) | Apache-2.0, `playwright-examples/LICENSE` |
| `nextjs-with-playwright` | [vercel/next.js](https://github.com/vercel/next.js) `examples/with-playwright/e2e/` at 13b59c9e | MIT, `nextjs-with-playwright/LICENSE` |
| `freecodecamp` | [freeCodeCamp/freeCodeCamp](https://github.com/freeCodeCamp/freeCodeCamp) six files of `e2e/` at 5d79b322 | BSD-3-Clause, `freecodecamp/LICENSE` |

The freeCodeCamp files import translations and helpers from the rest of
that repository, which is not copied: what reads them stays a TODO.
