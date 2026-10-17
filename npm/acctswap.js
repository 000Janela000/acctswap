#!/usr/bin/env node
// npm's `acctswap` command: runs the acctswap program from the package for this system
// (`@janela/acctswap-<os>-<cpu>`), which npm installed as an optional dependency.
"use strict";
const { spawnSync } = require("child_process");

const pkg = "@janela/acctswap-" + process.platform + "-" + process.arch;
const exe = process.platform === "win32" ? "acctswap.exe" : "acctswap";
let bin;
try {
  bin = require.resolve(pkg + "/" + exe);
} catch {
  console.error(
    "acctswap: " + pkg + " is not installed. There are builds for macOS, Linux and Windows " +
      "on x64 and arm64; on one of those, reinstall without --omit=optional.",
  );
  process.exit(1);
}
const run = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
if (run.error) {
  console.error("acctswap: " + run.error.message);
  process.exit(1);
}
process.exit(run.status === null ? 1 : run.status);
