## Browser: chrome-cdp

Use `chrome-cdp` to open, read, operate and check web pages. It drives a
real Chrome, one shell command per step.

- `chrome-cdp open <url>`, then `chrome-cdp snapshot -i` lists what can be
  acted on, with refs (`e12`).
- Act by ref: `chrome-cdp click e12`, `chrome-cdp fill e7 'text'`,
  `chrome-cdp press Enter`. Actions wait for their element and print what
  happened (navigation, dialog, console error, failed request).
- After a navigation, run `snapshot -i` again: the old refs are stale.
- Never `sleep`: use `chrome-cdp wait --text 'Saved'`, `wait <locator>` or
  `wait --url /path`.
- Verify with `chrome-cdp expect <locator> text 'Saved'` (or `visible`,
  `count 3`, `--no-errors`): it retries, and a failure says what it saw.
- When stuck, run the command in the error's `hint:` line, and check
  `chrome-cdp errors`, `chrome-cdp console` and `chrome-cdp screenshot`.
- E2E tests: `chrome-cdp test` runs `e2e/scenarios/*.cdp` against the
  mocks in `e2e/mocks`; a failure prints a `report.md` to read.
- `chrome-cdp close` when done. Full guide: `chrome-cdp skill`; reference:
  `chrome-cdp help`.
