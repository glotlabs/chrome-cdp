// Compiles a component of this folder and bundles it with Svelte's runtime
// into ../svelte-<name>.js, an IIFE that mounts it on #svelte-app.
// Run from this folder: see ../README.md for the pinned versions.
import { compile } from 'svelte/compiler';
import { build } from 'esbuild';
import { readFileSync, writeFileSync, mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const [component, out] = process.argv.slice(2);
const name = component.replace(/\.svelte$/, '');
const { js } = compile(readFileSync(component, 'utf8'), { generate: 'client', name });
const dir = mkdtempSync(join(tmpdir(), 'svelte-'));
writeFileSync(join(dir, `${name}.js`), js.code);
writeFileSync(
  join(dir, 'entry.js'),
  `import { mount } from 'svelte';\nimport C from './${name}.js';\nmount(C, { target: document.getElementById('svelte-app') });\n`,
);
await build({
  entryPoints: [join(dir, 'entry.js')],
  bundle: true,
  minify: true,
  format: 'iife',
  target: 'chrome120',
  conditions: ['browser', 'production'],
  nodePaths: [resolve('node_modules')],
  outfile: out,
  legalComments: 'none',
  logLevel: 'warning',
});
