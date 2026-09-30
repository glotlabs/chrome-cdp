// Plan 10's token-efficiency suite, and plan 04's first acceptance box:
// for each page of the snapshot corpus (testdata/site/snapshot/), count the
// tokens of `chrome-cdp snapshot`, `snapshot -i`, `snapshot --fold`,
// `snapshot --budget 2000` (plan 16) and Playwright MCP's
// `browser_snapshot`, and audit that every element Playwright gives a ref
// to an agent can act on has a ref in chrome-cdp's tree too.
//
// A deterministic count, not a timing benchmark. Needs Node, `npm ci` here,
// a built chrome-cdp and fixtures binary (`cargo build -p chrome-cdp -p
// fixtures`) and CHROME_PATH, the Chrome both tools drive. Usage:
//
//   node snapshot-tokens.mjs [--pages simple,login] [--dump <dir>] [--json]
//                            [--tokenizer auto|anthropic|o200k]
//
// `cargo xtask bench tokens` runs it and writes the table to
// docs/benchmarks.md. Tokenizers: `anthropic` counts with the Anthropic
// token counting API (model `claude-opus-5-5`; needs ANTHROPIC_API_KEY or
// ANTHROPIC_AUTH_TOKEN, and sends the fixture pages' snapshots there);
// `o200k` uses the o200k_base BPE (gpt-tokenizer), offline, as an
// approximation. `auto` (the default) takes `anthropic` when a key is set.
// The output names the tokenizer used; the ratio is what matters, and both
// tools' outputs go through the same one.

import { spawn, execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createInterface } from 'node:readline';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import { encode } from 'gpt-tokenizer';
import Anthropic from '@anthropic-ai/sdk';

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, '../..');
const TARGET = process.env.CARGO_TARGET_DIR || join(ROOT, 'target');
const BIN = join(TARGET, process.env.PROFILE || 'debug');

const CORPUS = ['simple', 'login', 'forms', 'hidden', 'clickable', 'lists-tables', 'widgets',
  'modal', 'shadow', 'frames', 'article', 'big-app', 'huge', 'cards'];

// Roles an agent acts on (chrome-cdp's `is_interactive`, less its own
// `clickable` and `iframe`).
const ACTIONABLE = new Set(['button', 'link', 'textbox', 'searchbox', 'checkbox', 'radio',
  'switch', 'combobox', 'listbox', 'option', 'menuitem', 'menuitemcheckbox', 'menuitemradio',
  'tab', 'slider', 'spinbutton', 'treeitem']);

function args() {
  const out = { pages: CORPUS, dump: null, json: false, tokenizer: 'auto' };
  const argv = process.argv.slice(2);
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--pages') out.pages = argv[++i].split(',');
    else if (argv[i] === '--dump') out.dump = argv[++i];
    else if (argv[i] === '--json') out.json = true;
    else if (argv[i] === '--tokenizer') out.tokenizer = argv[++i];
    else throw new Error(`unknown argument ${argv[i]}`);
  }
  return out;
}

/** Starts the fixture server; resolves to its origin A. */
function fixtures() {
  const child = spawn(join(BIN, 'fixtures'), [], { stdio: ['ignore', 'pipe', 'inherit'] });
  return new Promise((ok, fail) => {
    child.on('error', fail);
    createInterface({ input: child.stdout }).on('line', (line) => {
      const m = /^origin A: (\S+?)\/?$/.exec(line);
      if (m) ok({ origin: m[1], stop: () => child.kill() });
    });
  });
}

/** chrome-cdp in a session of its own, with no user config. */
function chromeCdp() {
  const dir = mkdtempSync(join(tmpdir(), 'ccdp-tokens-'));
  for (const d of ['run', 'config', 'tmp']) mkdirSync(join(dir, d));
  const env = {
    ...process.env,
    XDG_RUNTIME_DIR: join(dir, 'run'),
    XDG_CONFIG_HOME: join(dir, 'config'),
    TMPDIR: join(dir, 'tmp'),
    CHROME_CDP_CHROME: process.env.CHROME_PATH,
  };
  const run = (...a) => execFileSync(join(BIN, 'chrome-cdp'), a, { env, encoding: 'utf8', maxBuffer: 1 << 28 });
  run('open', 'about:blank');
  return {
    run,
    stop() {
      try { run('close'); } catch { /* already gone */ }
      rmSync(dir, { recursive: true, force: true });
    },
  };
}

async function playwrightMcp() {
  const out = mkdtempSync(join(tmpdir(), 'pw-mcp-'));
  const transport = new StdioClientTransport({
    command: process.execPath,
    // The same Chrome as chrome-cdp's, with its sandbox on.
    args: [join(HERE, 'node_modules/@playwright/mcp/cli.js'), '--headless', '--isolated',
      '--executable-path', process.env.CHROME_PATH, '--sandbox', '--output-dir', out],
    stderr: 'ignore',
  });
  const client = new Client({ name: 'chrome-cdp-bench', version: '0' });
  await client.connect(transport);
  const call = async (name, a) => {
    const res = await client.callTool({ name, arguments: a });
    const text = res.content.filter((c) => c.type === 'text').map((c) => c.text).join('\n');
    if (res.isError) throw new Error(`${name}: ${text}`);
    return text;
  };
  return {
    call,
    async stop() {
      await client.close();
      rmSync(out, { recursive: true, force: true });
    },
  };
}

/** The model whose tokenizer the `anthropic` tokenizer counts with. */
const COUNT_MODEL = 'claude-opus-5-5';

/** A counter: `{ name, count(text) → Promise<number> }`. */
async function tokenizer(which) {
  const keyed = Boolean(process.env.ANTHROPIC_API_KEY || process.env.ANTHROPIC_AUTH_TOKEN);
  if (which === 'auto') which = keyed ? 'anthropic' : 'o200k';
  if (which === 'o200k') return { name: 'o200k_base', count: async (text) => encode(text).length };
  if (which !== 'anthropic') throw new Error(`unknown tokenizer ${which}; use auto, anthropic or o200k`);
  const client = new Anthropic();
  const raw = async (text) => (await client.messages.countTokens({
    model: COUNT_MODEL,
    messages: [{ role: 'user', content: text }],
  })).input_tokens;
  // The count includes the message's framing: subtract a one-character
  // message's count, less that character.
  const framing = (await raw('.')) - 1;
  return { name: `anthropic:${COUNT_MODEL}`, count: async (text) => (await raw(text)) - framing };
}

/** (role, name) of every line with a ref, from either format. */
function reffed(text, pattern) {
  const out = [];
  for (const line of text.split('\n')) {
    const m = pattern.exec(line);
    if (m) out.push({ role: m[1], name: m[2] ?? '', line: line.trim() });
  }
  return out;
}
const PW_LINE = /^\s*- (\w+)(?: "((?:[^"\\]|\\.)*)")?.*\[ref=\w+\]/;
const OUR_LINE = /^\s*[-+~]? ?- (\w+)(?: "((?:[^"\\]|\\.)*)")? \[e\d+\]/;

/** Playwright's actionable elements that chrome-cdp's tree has no ref for,
 * matched by name (roles differ in places: a colour input, a date field).
 * A nameless one (a link around an image, which chrome-cdp names by the
 * image) takes any left of its role. */
function lost(pw, ours) {
  const count = (key) => {
    const m = new Map();
    for (const n of ours) m.set(key(n), (m.get(key(n)) || 0) + 1);
    return m;
  };
  const names = count((n) => n.name);
  const roles = count((n) => n.role);
  const take = (m, k) => {
    const left = m.get(k) || 0;
    if (left > 0) m.set(k, left - 1);
    return left > 0;
  };
  const missing = [];
  const actionable = pw.filter((n) => ACTIONABLE.has(n.role));
  for (const n of actionable.filter((n) => n.name)) {
    if (take(names, n.name)) {
      const role = ours.find((o) => o.name === n.name)?.role;
      take(roles, role);
    } else missing.push(n.line);
  }
  for (const n of actionable.filter((n) => !n.name)) {
    if (!take(roles, n.role)) missing.push(n.line);
  }
  return missing;
}

async function main() {
  const opts = args();
  if (!process.env.CHROME_PATH) throw new Error('set CHROME_PATH to the Chrome chrome-cdp should use');
  const counter = await tokenizer(opts.tokenizer);
  const tokens = counter.count;
  const site = await fixtures();
  const ccdp = chromeCdp();
  const pw = await playwrightMcp();
  const rows = [];
  try {
    for (const page of opts.pages) {
      const url = `${site.origin}/snapshot/${page}.html`;
      ccdp.run('goto', url);
      const ours = ccdp.run('snapshot');
      const interactive = ccdp.run('snapshot', '-i');
      const fold = ccdp.run('snapshot', '--fold');
      const budget = ccdp.run('snapshot', '--budget', '2000');
      // Every node, for the audit: nothing collapsed or cut.
      const full = ccdp.run('snapshot', '--all', '--max-chars', '0');
      await pw.call('browser_navigate', { url });
      const theirs = await pw.call('browser_snapshot', {});
      const pwRefs = reffed(theirs, PW_LINE);
      const missing = lost(pwRefs, reffed(full, OUR_LINE));
      const row = {
        page,
        chrome_cdp: await tokens(ours),
        interactive: await tokens(interactive),
        fold: await tokens(fold),
        budget: await tokens(budget),
        playwright: await tokens(theirs),
        actionable: pwRefs.filter((n) => ACTIONABLE.has(n.role)).length,
        missing,
      };
      rows.push(row);
      if (opts.dump) {
        mkdirSync(opts.dump, { recursive: true });
        writeFileSync(join(opts.dump, `${page}.chrome-cdp.txt`), ours);
        writeFileSync(join(opts.dump, `${page}.chrome-cdp-all.txt`), full);
        writeFileSync(join(opts.dump, `${page}.playwright.txt`), theirs);
      }
    }
  } finally {
    await pw.stop();
    ccdp.stop();
    site.stop();
  }
  const sum = (k) => rows.reduce((s, r) => s + r[k], 0);
  const total = {
    chrome_cdp: sum('chrome_cdp'), interactive: sum('interactive'), fold: sum('fold'),
    budget: sum('budget'), playwright: sum('playwright'),
  };
  if (opts.json) {
    console.log(JSON.stringify({ tokenizer: counter.name, rows, total }, null, 2));
    return;
  }
  const pct = (a, b) => `${Math.round((100 * a) / b)}%`;
  const pad = (s, n) => String(s).padStart(n);
  console.log(`tokenizer: ${counter.name}`);
  console.log(`${'page'.padEnd(14)}${pad('snapshot', 10)}${pad('-i', 8)}${pad('--fold', 8)}${pad('--budget', 10)}${pad('playwright', 12)}${pad('ratio', 7)}${pad('actionable', 12)}${pad('lost', 6)}`);
  for (const r of rows) {
    console.log(`${r.page.padEnd(14)}${pad(r.chrome_cdp, 10)}${pad(r.interactive, 8)}${pad(r.fold, 8)}${pad(r.budget, 10)}${pad(r.playwright, 12)}${pad(pct(r.chrome_cdp, r.playwright), 7)}${pad(r.actionable, 12)}${pad(r.missing.length, 6)}`);
  }
  console.log(`${'total'.padEnd(14)}${pad(total.chrome_cdp, 10)}${pad(total.interactive, 8)}${pad(total.fold, 8)}${pad(total.budget, 10)}${pad(total.playwright, 12)}${pad(pct(total.chrome_cdp, total.playwright), 7)}`);
  for (const r of rows.filter((r) => r.missing.length)) {
    console.log(`\n${r.page}: Playwright refs with no chrome-cdp ref of the same name:`);
    for (const line of r.missing) console.log(`  ${line}`);
  }
}

main().catch((e) => {
  console.error(`error: ${e.message}`);
  process.exit(1);
});
