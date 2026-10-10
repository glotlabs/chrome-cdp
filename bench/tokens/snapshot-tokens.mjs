// Plan 10's token-efficiency suite, and plan 04's first acceptance box:
// for each page of the snapshot corpus (testdata/site/snapshot/), count the
// tokens of `chrome-cdp snapshot`, `snapshot -i`, `snapshot --fold`,
// `snapshot --budget 2000` (plan 16).
//
// A deterministic count, not a timing benchmark. Needs Node, `npm ci` here,
// a built chrome-cdp and fixtures binary (`cargo build -p chrome-cdp -p
// fixtures`) and CHROME_PATH, the Chrome chrome-cdp drives. Usage:
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
// The output names the tokenizer used.

import { spawn, execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createInterface } from 'node:readline';
import { encode } from 'gpt-tokenizer';
import Anthropic from '@anthropic-ai/sdk';

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, '../..');
const TARGET = process.env.CARGO_TARGET_DIR || join(ROOT, 'target');
const BIN = join(TARGET, process.env.PROFILE || 'debug');

const CORPUS = ['simple', 'login', 'forms', 'hidden', 'clickable', 'lists-tables', 'widgets',
  'modal', 'shadow', 'frames', 'article', 'big-app', 'huge', 'cards'];

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

async function main() {
  const opts = args();
  if (!process.env.CHROME_PATH) throw new Error('set CHROME_PATH to the Chrome chrome-cdp should use');
  const counter = await tokenizer(opts.tokenizer);
  const tokens = counter.count;
  const site = await fixtures();
  const ccdp = chromeCdp();
  const rows = [];
  try {
    for (const page of opts.pages) {
      const url = `${site.origin}/snapshot/${page}.html`;
      ccdp.run('goto', url);
      const ours = ccdp.run('snapshot');
      const interactive = ccdp.run('snapshot', '-i');
      const fold = ccdp.run('snapshot', '--fold');
      const budget = ccdp.run('snapshot', '--budget', '2000');
      const row = {
        page,
        chrome_cdp: await tokens(ours),
        interactive: await tokens(interactive),
        fold: await tokens(fold),
        budget: await tokens(budget),
      };
      rows.push(row);
      if (opts.dump) {
        mkdirSync(opts.dump, { recursive: true });
        writeFileSync(join(opts.dump, `${page}.txt`), ours);
      }
    }
  } finally {
    ccdp.stop();
    site.stop();
  }
  const sum = (k) => rows.reduce((s, r) => s + r[k], 0);
  const total = {
    chrome_cdp: sum('chrome_cdp'), interactive: sum('interactive'), fold: sum('fold'),
    budget: sum('budget'),
  };
  if (opts.json) {
    console.log(JSON.stringify({ tokenizer: counter.name, rows, total }, null, 2));
    return;
  }
  const pad = (s, n) => String(s).padStart(n);
  console.log(`tokenizer: ${counter.name}`);
  console.log(`${'page'.padEnd(14)}${pad('snapshot', 10)}${pad('-i', 8)}${pad('--fold', 8)}${pad('--budget', 10)}`);
  for (const r of rows) {
    console.log(`${r.page.padEnd(14)}${pad(r.chrome_cdp, 10)}${pad(r.interactive, 8)}${pad(r.fold, 8)}${pad(r.budget, 10)}`);
  }
  console.log(`${'total'.padEnd(14)}${pad(total.chrome_cdp, 10)}${pad(total.interactive, 8)}${pad(total.fold, 8)}${pad(total.budget, 10)}`);
}

main().catch((e) => {
  console.error(`error: ${e.message}`);
  process.exit(1);
});
