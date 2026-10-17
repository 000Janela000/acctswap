#!/usr/bin/env node
// Writes and packs the npm package of one release from its archives: `@cj-ways/acctswap`, carrying the
// program for each of the six systems in native/<os>-<cpu>/ and a launcher that runs the right
// one. No install scripts.
//   node scripts/npm-package.mjs <version> <archives-dir> <out-dir>
// <out-dir> gets cj-ways-acctswap-<version>.tgz.
import { execFileSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const [version, archivesArg, outArg] = process.argv.slice(2);
if (!version || !archivesArg || !outArg) {
  console.error("usage: node scripts/npm-package.mjs <version> <archives-dir> <out-dir>");
  process.exit(2);
}
const root = resolve(import.meta.dirname, "..");
const archives = resolve(archivesArg);
const out = resolve(outArg);
const repo = "https://github.com/cj-ways/acctswap";
// npm adds a licence file by itself only when it is named LICENSE or LICENCE.
const licences = ["LICENSE-MIT", "LICENSE-APACHE"];

// Rust target, and Node's process.platform and process.arch for it.
const systems = [
  { target: "aarch64-apple-darwin", os: "darwin", cpu: "arm64" },
  { target: "x86_64-apple-darwin", os: "darwin", cpu: "x64" },
  { target: "aarch64-unknown-linux-musl", os: "linux", cpu: "arm64" },
  { target: "x86_64-unknown-linux-musl", os: "linux", cpu: "x64" },
  { target: "aarch64-pc-windows-msvc", os: "win32", cpu: "arm64" },
  { target: "x86_64-pc-windows-msvc", os: "win32", cpu: "x64" },
];

const pkg = mkdtempSync(join(tmpdir(), "acctswap-npm-"));
for (const s of systems) {
  const dir = join(pkg, "native", `${s.os}-${s.cpu}`);
  mkdirSync(dir, { recursive: true });
  if (s.os === "win32") {
    execFileSync("unzip", ["-q", join(archives, `acctswap-${s.target}.zip`), "acctswap.exe", "-d", dir]);
  } else {
    execFileSync("tar", ["-xzf", join(archives, `acctswap-${s.target}.tar.gz`), "-C", dir, "acctswap"]);
    chmodSync(join(dir, "acctswap"), 0o755);
  }
}
mkdirSync(join(pkg, "bin"));
copyFileSync(join(root, "npm", "acctswap.js"), join(pkg, "bin", "acctswap.js"));
chmodSync(join(pkg, "bin", "acctswap.js"), 0o755);
for (const f of ["README.md", ...licences]) copyFileSync(join(root, f), join(pkg, f));
const manifest = {
  name: "@cj-ways/acctswap",
  version,
  description:
    "Switch the account every Claude Code session on a machine uses, from saved logins, without a browser",
  license: "MIT OR Apache-2.0",
  repository: { type: "git", url: `git+${repo}.git` },
  homepage: `${repo}#readme`,
  keywords: ["claude-code", "account", "switch", "cli"],
  bin: { acctswap: "bin/acctswap.js" },
  files: ["bin", "native", ...licences],
};
writeFileSync(join(pkg, "package.json"), `${JSON.stringify(manifest, null, 2)}\n`);

mkdirSync(out, { recursive: true });
const args = ["pack", pkg, "--pack-destination", out, "--ignore-scripts", "--json"];
const [packed] = JSON.parse(execFileSync("npm", args, { encoding: "utf8" }));
rmSync(pkg, { recursive: true, force: true });
console.log(`${packed.filename}: ${packed.size} bytes, ${packed.unpackedSize} unpacked`);
