// Writes the storage state file Playwright saves after logging in on the
// fixture site (testdata/site/state/login.html): chrome-cdp's test that
// Playwright's files load unchanged (plan 07, milestone 7.4) reads it.
// With the fixture server running (`cargo xtask fixtures --port 47311`):
//
//   npm ci && CHROME_PATH=<chrome> node storage-state.mjs http://127.0.0.1:47311 \
//     > ../../testdata/state/playwright.json
//
// The test puts its own fixture origin in place of the one recorded here.
import { chromium } from 'playwright-core';

const [site] = process.argv.slice(2);
const browser = await chromium.launch({
  executablePath: process.env.CHROME_PATH,
  chromiumSandbox: true,
});
const context = await browser.newContext();
const page = await context.newPage();
await page.goto(`${site}/state/login.html`);
await page.click('#login');
await page.getByText('Logged in as Ada').waitFor();
const state = await context.storageState({ indexedDB: true });
process.stdout.write(JSON.stringify(state, null, 2) + '\n');
await browser.close();
