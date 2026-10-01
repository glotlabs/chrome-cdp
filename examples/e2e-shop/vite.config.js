import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The files dist/ is built from. dist/ is committed (the tests need no
// Node), so `npm run build` writes a hash of them to dist/sources.txt and
// crates/chrome-cdp/tests/e2e_shop.rs checks it is current.
const SOURCES = ["index.html", "package-lock.json", "public", "src", "vite.config.js"];

function files(path) {
  try {
    return readdirSync(path, { withFileTypes: true })
      .map((entry) => join(path, entry.name))
      .flatMap(files);
  } catch (e) {
    if (e.code === "ENOTDIR") return [path];
    throw e;
  }
}

// FNV-1a (64-bit) over each file's path, a NUL, its bytes and a NUL, in
// path order.
function sourcesHash() {
  let hash = 0xcbf29ce484222325n;
  const paths = SOURCES.flatMap(files).map((p) => p.split("\\").join("/"));
  for (const path of paths.sort()) {
    const bytes = [
      ...Buffer.from(path),
      0,
      ...readFileSync(path),
      0,
    ];
    for (const byte of bytes) {
      hash ^= BigInt(byte);
      hash = (hash * 0x100000001b3n) & 0xffffffffffffffffn;
    }
  }
  return hash.toString(16).padStart(16, "0");
}

export default defineConfig({
  plugins: [
    react(),
    {
      name: "sources-hash",
      apply: "build",
      closeBundle() {
        writeFileSync("dist/sources.txt", `${sourcesHash()}\n`);
      },
    },
  ],
  // `npm run dev` sends /api to the real API on port 4174, if one runs
  // (see README.md). The tests answer /api themselves.
  server: {
    proxy: { "/api": { target: "http://localhost:4174", ws: true } },
  },
});
