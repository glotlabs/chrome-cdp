// The build every fixture app shares (plan 15, "Fixture apps"): a dev
// build, as a dev server would serve it, written to
// testdata/site/apps/dev/<name>/ and committed, so the tests need no Node.
//
// - development mode: React's development build, Vue's dev build,
//   Svelte's `dev: true` (set by each app's config);
// - no minification, and external source maps (`.map` files), so the
//   map-loading path is the real one;
// - stable file names, so a rebuild's diff is readable;
// - absolute paths under /build/apps-src, wherever it was built;
// - sources.txt: a hash of the sources, which a test checks
//   (crates/engine/tests/apps.rs), so a stale build fails.

import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const APPS = dirname(dirname(fileURLToPath(import.meta.url)));

function files(path) {
  try {
    return readdirSync(path, { withFileTypes: true })
      .filter((entry) => entry.name !== "node_modules")
      .map((entry) => join(path, entry.name))
      .flatMap(files);
  } catch (e) {
    if (e.code === "ENOTDIR") return [path];
    throw e;
  }
}

// FNV-1a (64-bit) over each file of the app and of shared/: its path
// relative to apps-src (`/` separators), a NUL, its bytes and a NUL, in
// path order.
export function sourcesHash(name) {
  let hash = 0xcbf29ce484222325n;
  const paths = [name, "shared"]
    .flatMap((dir) => files(join(APPS, dir)))
    .map((path) => relative(APPS, path).split("\\").join("/"));
  for (const path of paths.sort()) {
    const bytes = [...Buffer.from(path), 0, ...readFileSync(join(APPS, path)), 0];
    for (const byte of bytes) {
      hash ^= BigInt(byte);
      hash = (hash * 0x100000001b3n) & 0xffffffffffffffffn;
    }
  }
  return hash.toString(16).padStart(16, "0");
}

// A dev build: Vite and the framework plugins read NODE_ENV (not the
// mode) to pick development JSX, Vue's `__file` and the like.
process.env.NODE_ENV = "development";

// The Vite config of app `name`, with the framework's `plugins`.
export function devBuild(name, plugins) {
  const outDir = resolve(APPS, "../site/apps/dev", name);
  return {
    mode: "development",
    base: "./",
    publicDir: resolve(APPS, "shared/public"),
    plugins: [
      ...plugins,
      {
        // Dev builds name files by absolute path (React 18's
        // `_debugSource`, Vue's `__file`). The committed build must not
        // depend on where it was built, so this checkout's apps-src
        // becomes /build/apps-src, as if built in a container.
        name: "neutral-paths",
        apply: "build",
        renderChunk(code) {
          return { code: code.split(APPS).join("/build/apps-src"), map: null };
        },
      },
      {
        name: "sources-hash",
        apply: "build",
        closeBundle() {
          writeFileSync(join(outDir, "sources.txt"), `${sourcesHash(name)}\n`);
        },
      },
    ],
    define: { "process.env.NODE_ENV": JSON.stringify("development") },
    build: {
      outDir,
      emptyOutDir: true,
      minify: false,
      sourcemap: true,
      modulePreload: { polyfill: false },
      rollupOptions: {
        output: {
          entryFileNames: "assets/[name].js",
          chunkFileNames: "assets/[name].js",
          assetFileNames: "assets/[name][extname]",
          // The maps name their sources; the files are on disk.
          sourcemapExcludeSources: true,
        },
      },
    },
  };
}
