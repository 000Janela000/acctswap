#!/usr/bin/env node
// Writes and packs the npm packages of one release, from its archives: one package per system
// holding the program (`@janela/acctswap-<os>-<cpu>`; npm installs only the one whose `os` and
// `cpu` fit), and `acctswap`, whose launcher runs it. No install scripts.
//   node scripts/npm-packages.mjs <version> <archives-dir> <out-dir>
// <out-dir> gets one .tgz per package and publish-order.txt ("<file> <package>" per line, the
// system packages first, so `acctswap` never names a version that is not there yet).
import { execFileSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const [version, archivesArg, outArg] = process.argv.slice(2);
if (!version || !archivesArg || !outArg) {
  console.error("usage: node scripts/npm-packages.mjs <version> <archives-dir> <out-dir>");
  process.exit(2);
}
const root = resolve(import.meta.dirname, "..");
const archives = resolve(archivesArg);
const out = resolve(outArg);
const repo = "https://github.com/000Janela000/acctswap";
const common = {
  version,
  license: "MIT OR Apache-2.0",
  repository: { type: "git", url: `git+${repo}.git` },
};

// Rust target, and Node's process.platform and process.arch for it.
const systems = [
  { target: "aarch64-apple-darwin", os: "darwin", cpu: "arm64", label: "macOS on arm64" },
  { target: "x86_64-apple-darwin", os: "darwin", cpu: "x64", label: "macOS on x64" },
  { target: "aarch64-unknown-linux-musl", os: "linux", cpu: "arm64", label: "Linux on arm64" },
  { target: "x86_64-unknown-linux-musl", os: "linux", cpu: "x64", label: "Linux on x64" },
  { target: "aarch64-pc-windows-msvc", os: "win32", cpu: "arm64", label: "Windows on arm64" },
  { target: "x86_64-pc-windows-msvc", os: "win32", cpu: "x64", label: "Windows on x64" },
];
const packageName = (s) => `@janela/acctswap-${s.os}-${s.cpu}`;
// npm adds a licence file by itself only when it is named LICENSE or LICENCE.
const licences = ["LICENSE-MIT", "LICENSE-APACHE"];

const work = mkdtempSync(join(tmpdir(), "acctswap-npm-"));
mkdirSync(out, { recursive: true });
const order = [];

function pack(dir, manifest) {
  writeFileSync(join(dir, "package.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  for (const f of licences) copyFileSync(join(root, f), join(dir, f));
  const args = ["pack", dir, "--pack-destination", out, "--ignore-scripts", "--json"];
  const [packed] = JSON.parse(execFileSync("npm", args, { encoding: "utf8" }));
  order.push(`${packed.filename} ${manifest.name}`);
}

for (const s of systems) {
  const dir = join(work, `${s.os}-${s.cpu}`);
  mkdirSync(dir);
  const exe = s.os === "win32" ? "acctswap.exe" : "acctswap";
  if (s.os === "win32") {
    execFileSync("unzip", ["-q", join(archives, `acctswap-${s.target}.zip`), exe, "-d", dir]);
  } else {
    execFileSync("tar", ["-xzf", join(archives, `acctswap-${s.target}.tar.gz`), "-C", dir, exe]);
    chmodSync(join(dir, exe), 0o755);
  }
  writeFileSync(
    join(dir, "README.md"),
    `# ${packageName(s)}\n\nThe [acctswap](${repo}) program for ${s.label}. ` +
      "Install `acctswap` instead: npm adds this package on that system only.\n",
  );
  pack(dir, {
    name: packageName(s),
    description: `The acctswap program for ${s.label}. Install acctswap instead.`,
    ...common,
    os: [s.os],
    cpu: [s.cpu],
    files: [exe, ...licences],
  });
}

const main = join(work, "acctswap");
mkdirSync(join(main, "bin"), { recursive: true });
copyFileSync(join(root, "npm", "acctswap.js"), join(main, "bin", "acctswap.js"));
chmodSync(join(main, "bin", "acctswap.js"), 0o755);
copyFileSync(join(root, "README.md"), join(main, "README.md"));
pack(main, {
  name: "acctswap",
  description:
    "Switch the account every Claude Code session on a machine uses, from saved logins, without a browser",
  ...common,
  homepage: `${repo}#readme`,
  keywords: ["claude-code", "account", "switch", "cli"],
  bin: { acctswap: "bin/acctswap.js" },
  files: ["bin", ...licences],
  optionalDependencies: Object.fromEntries(systems.map((s) => [packageName(s), version])),
});

writeFileSync(join(out, "publish-order.txt"), `${order.join("\n")}\n`);
rmSync(work, { recursive: true, force: true });
console.log(order.join("\n"));
