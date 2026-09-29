// Plan 10.4's micro suite, Playwright side: the Playwright library's
// equivalents of chrome-cdp's micro benchmarks (crates/bench/src/micro.rs),
// on the same fixture pages and the same Chrome. `cargo xtask bench micro`
// runs it; it prints the metrics as JSON on stdout.
//
//   node micro.mjs --origin http://127.0.0.1:PORT [--cold 5] [--warm 50] [--each 10]
//
// Every measurement but the cold start is a call inside this one Node
// process; the cold start times a fresh `node` that launches Chrome and
// opens about:blank, as an agent's first call would.

import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright-core';

const SELF = fileURLToPath(import.meta.url);
const SNAPSHOT_PAGES = ['simple', 'forms', 'big-app', 'huge'];
const LOCATORS = [
  ['css', 'css=section[aria-label="Section 480"] h2'],
  ['text', 'text=Item 480.7'],
  ['role', (page) => page.getByRole('heading', { name: 'Section 480', exact: true })],
  ['xpath', 'xpath=//section[@aria-label="Section 480"]/h2'],
];
const ROUTE_RULES = 20;
const PAGE_REQUESTS = 50;

function args() {
  const out = { origin: null, cold: 5, warm: 50, each: 10, coldChild: false };
  const argv = process.argv.slice(2);
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--origin') out.origin = argv[++i];
    else if (a === '--cold') out.cold = Number(argv[++i]);
    else if (a === '--warm') out.warm = Number(argv[++i]);
    else if (a === '--each') out.each = Number(argv[++i]);
    else if (a === '--cold-child') out.coldChild = true;
    else throw new Error(`unknown argument ${a}`);
  }
  return out;
}

const launch = () => chromium.launch({ executablePath: process.env.CHROME_PATH, headless: true });

function percentile(values, p) {
  if (!values.length) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  const rank = Math.ceil((p / 100) * sorted.length);
  return sorted[Math.min(Math.max(rank, 1), sorted.length) - 1];
}

function metric(name, values, { unit = 'ms', better = 'lower', note } = {}) {
  const m = {
    name, unit, better,
    samples: values.length,
    p50: percentile(values, 50),
    p95: percentile(values, 95),
    mean: values.reduce((s, v) => s + v, 0) / (values.length || 1),
  };
  if (note) m.note = note;
  return m;
}

async function timeN(n, f) {
  const out = [];
  for (let i = 0; i < n; i++) {
    const start = performance.now();
    await f();
    out.push(performance.now() - start);
  }
  return out;
}

/** One cold start: a new Node process until it has about:blank open. */
function coldOnce() {
  return new Promise((ok, fail) => {
    const start = performance.now();
    const child = spawn(process.execPath, [SELF, '--cold-child'], { stdio: ['ignore', 'pipe', 'inherit'] });
    let done = false;
    child.stdout.on('data', (chunk) => {
      if (!done && chunk.toString().includes('ready')) {
        done = true;
        ok(performance.now() - start);
      }
    });
    child.on('error', fail);
    child.on('exit', (code) => { if (!done) fail(new Error(`cold child exited ${code}`)); });
  });
}

async function coldChild() {
  const browser = await launch();
  const page = await browser.newPage();
  await page.goto('about:blank');
  process.stdout.write('ready\n');
  await browser.close();
}

/** The snapshot Playwright MCP sends, when this Playwright has it. */
async function snapshot(page) {
  if (typeof page._snapshotForAI === 'function') return page._snapshotForAI();
  return page.locator('body').ariaSnapshot();
}

async function main() {
  const opts = args();
  if (opts.coldChild) return coldChild();
  if (!opts.origin) throw new Error('--origin is required');
  if (!process.env.CHROME_PATH) throw new Error('set CHROME_PATH');
  const metrics = [];

  const cold = [];
  for (let i = 0; i < opts.cold; i++) cold.push(await coldOnce());
  metrics.push(metric('cold_open', cold, { note: 'node + launch + about:blank' }));

  const browser = await launch();
  const page = await browser.newPage();
  try {
    await page.goto('about:blank');
    metrics.push(metric('warm_round_trip', await timeN(opts.warm, () => page.evaluate('1')),
      { note: 'page.evaluate in-process' }));

    const snapshotKind = typeof page._snapshotForAI === 'function' ? '_snapshotForAI' : 'ariaSnapshot';
    for (const name of SNAPSHOT_PAGES) {
      await page.goto(`${opts.origin}/snapshot/${name}.html`);
      metrics.push(metric(`snapshot/${name}`, await timeN(opts.each, () => snapshot(page)),
        { note: snapshotKind }));
    }

    await page.goto(`${opts.origin}/snapshot/huge.html`);
    for (const [kind, loc] of LOCATORS) {
      const locator = typeof loc === 'function' ? loc(page) : page.locator(loc);
      metrics.push(metric(`locator/${kind}`, await timeN(opts.each, () => locator.innerText())));
    }

    await page.goto(`${opts.origin}/bench/micro/click.html`);
    metrics.push(metric('click_report', await timeN(opts.each * 2, () => page.click('#inc'))));

    const requests = `${opts.origin}/bench/micro/requests.html`;
    const plain = metric('route/load_plain', await timeN(opts.each, () => page.goto(requests)));
    for (let i = 1; i < ROUTE_RULES; i++) {
      await page.route(`**/no-such-path-${i}/**`, (route) => route.fulfill({ status: 404 }));
    }
    await page.route('**/bench/micro/asset.svg*', (route) =>
      route.continue({ headers: { ...route.request().headers(), 'x-bench': '1' } }));
    const routed = metric('route/load_20_rules', await timeN(opts.each, () => page.goto(requests)));
    await page.unrouteAll();
    metrics.push(plain, routed);
    metrics.push(metric('route/overhead_per_request',
      [Math.max(0, (routed.p50 - plain.p50) / (PAGE_REQUESTS + 1))]));
  } finally {
    await browser.close();
  }
  console.log(JSON.stringify(metrics, null, 2));
}

main().catch((e) => {
  console.error(`error: ${e.message}`);
  process.exit(1);
});
