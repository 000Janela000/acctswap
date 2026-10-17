#!/usr/bin/env node
// npm's `acctswap` command: runs the program for this system from native/<os>-<cpu>/ in this
// package, which carries the programs for all six systems.
"use strict";
const { spawnSync } = require("child_process");
const { existsSync } = require("fs");
const path = require("path");

const system = process.platform + "-" + process.arch;
const exe = process.platform === "win32" ? "acctswap.exe" : "acctswap";
const bin = path.join(__dirname, "..", "native", system, exe);
if (!existsSync(bin)) {
  console.error(
    "acctswap: no build for " + system + ". There are builds for macOS, Linux and Windows " +
      "on x64 and arm64.",
  );
  process.exit(1);
}
const run = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
if (run.error) {
  console.error("acctswap: " + run.error.message);
  process.exit(1);
}
process.exit(run.status === null ? 1 : run.status);
