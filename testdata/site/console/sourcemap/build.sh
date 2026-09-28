#!/bin/sh
# Rebuilds the bundles in dist/ from src/ (committed, so tests need no
# Node): one with an external map, one with an inline (data: URL) map.
set -e
cd "$(dirname "$0")"
npx --yes esbuild@0.28.2 src/main.ts --bundle --minify --sourcemap --outfile=dist/app.js
npx --yes esbuild@0.28.2 src/main.ts --bundle --minify --sourcemap=inline --outfile=dist/inline.js
