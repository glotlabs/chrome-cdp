#!/usr/bin/env node
// Runs the chrome-cdp binary from the optional dependency npm installed for
// this platform (chrome-cdp-<os>-<cpu>), passing arguments, stdio, the
// exit code and signals through. CHROME_CDP_BINARY overrides the binary.
"use strict";

const { spawnSync } = require("child_process");
const path = require("path");

const pkg = `chrome-cdp-${process.platform}-${process.arch}`;

function binary() {
  if (process.env.CHROME_CDP_BINARY) {
    return process.env.CHROME_CDP_BINARY;
  }
  try {
    return require.resolve(`${pkg}/bin/chrome-cdp`);
  } catch {
    return null;
  }
}

const bin = binary();
if (!bin) {
  const supported = ["linux-x64", "linux-arm64", "darwin-x64", "darwin-arm64"];
  const here = `${process.platform}-${process.arch}`;
  process.stderr.write(
    supported.includes(here)
      ? `error: ${pkg} is not installed, so chrome-cdp has no binary\n` +
          "  hint: reinstall without --no-optional / --omit=optional, e.g. npm install chrome-cdp\n"
      : `error: chrome-cdp has no build for ${here}\n` +
          `  hint: it runs on ${supported.join(", ")}\n`,
  );
  process.exit(4);
}

const result = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  process.stderr.write(`error: cannot run ${bin}: ${result.error.message}\n`);
  process.exit(4);
}
if (result.signal) {
  process.kill(process.pid, result.signal);
}
process.exit(result.status ?? 1);
