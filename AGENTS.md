# acctswap — Agent Instructions

A Rust command-line tool that switches the Claude account every Claude Code session on one machine uses, from saved copies of each login, without a browser. Saba runs it on his Mac, PC and server. The repo is public (`000Janela000/acctswap`); releases are built on GitHub Actions.

**This file is shared by every AI agent.** Claude Code reads it through `CLAUDE.md`. Codex reads it directly. Keep tool-specific notes out of here.

**Shared rules for all projects:** `../_shared/README.md`. Before social media, Codex, video or browser work, read the matching file in `_shared/playbooks/`. 🔒 Locks there can't be overridden here.

---

## Start of every session
1. Read `STATUS.md`.
2. Run `git status`. It should be clean, or match what `STATUS.md` says.
3. Read `README.md` for how the tool works and what is verified.

## End of every session
1. Update `STATUS.md` (log + open problems + next steps).
2. Commit with the repo's commit style (see "Rules").
3. Push to `main` freely: it runs CI and deploys nothing. Push a `v*` tag only after CI on `main` is green: the tag builds a release.

## Where things live
| Path | What it is |
|---|---|
| `crates/acctswap-core/` | The rules: no file, Keychain or network access. Ports, the account decision, the switch protocol, Kani proofs, scenario tests with fakes |
| `crates/acctswap/` | The `acctswap` binary: Keychain, file and curl adapters, the rate-limit hook, the commands |
| `scripts/update-tap.sh` | Writes the Homebrew formula for a release into the tap, from its `SHA256SUMS` |
| `scripts/npm-packages.mjs` | Writes and packs the npm packages of a release from its archives: `@janela/acctswap-<os>-<cpu>` per system, and `acctswap` |
| `npm/acctswap.js` | The launcher in the `acctswap` npm package: runs the program from the package for the system |
| `pyproject.toml` | maturin settings for the PyPI wheels (the program only, no Python code) |
| `spec/` | The TLA+ model of the switch protocol and its configurations (`spec/check.sh`) |
| `.github/workflows/` | `ci.yml` (lint, tests on 3 systems, coverage, Kani, TLA+, cargo-deny); `release.yml` (on a `v*` tag: archives, wheels and npm packages, the GitHub release, then crates.io, PyPI and npm; run by hand it builds and publishes nothing) |
| `README.md` | Usage, how it works, what is verified, limits |
| `STATUS.md` | Current state and log; kept out of git, on Saba's machines only (Syncthing) |
| `private/` | Personal notes, out of git: the publishing research (`private/publishing.md`) and a backup of the old claude-sw history |

## Run, check, deploy
- `cargo build`, `cargo test`, `cargo clippy --all-targets`, `cargo fmt --all --check`: run them before saying a code task is done.
- Core coverage must stay at 100%: `cargo llvm-cov -p acctswap-core --fail-under-lines 100 --fail-under-functions 100 --fail-under-regions 100`.
- Mutation testing must miss nothing: `cargo mutants -p acctswap-core` (scope with `-f <file>` while iterating).
- Proofs: `cargo kani -p acctswap-core` (about 6 minutes). Model check: `spec/check.sh` (needs Java 11+ and the TLA+ jar it names).
- Install from source: `cargo install --locked --path crates/acctswap`, then add the `sw` script (README, "Install").
- **Deploy:** a `v*` tag builds release archives for macOS (arm64, x86_64), Linux (x86_64, arm64; musl) and Windows (x86_64, arm64), with `SHA256SUMS` and an attestation; the release is created as a draft and then published (immutable). Then the same run publishes to crates.io, PyPI and npm through trusted publishing: no tokens are stored, and each registry trusts only `release.yml` in its GitHub environment (`crates-io`, `pypi`, `npm`, which accept only `v*` tags). The tag must equal the version in `Cargo.toml`. Before a tag, run the release workflow by hand on `main` (`gh workflow run release.yml`): it builds and packs everything and publishes nothing. After a release, the session updates the Homebrew formula in `000Janela000/homebrew-acctswap`. Installed machines update only through a request to that machine's Claude session, which asks Saba there (`../_shared/standards/agent-coordination.md`).

## Rules
- **Commits:** one line `type(scope): summary`, no body, no trailers.
- **Never run a real switch** (`sw`, `sw <name>`, `sw active`) without Saba: it changes the account of every session on that machine and drops their Remote Control.
- Every change to the decision or the protocol keeps the proofs, the model check, 100% core coverage and zero missed mutants.
- No `unsafe`. A new dependency needs a reason; `deny.toml` checks licences and sources.
- The licence files live at the root and, as copies, in each crate folder (crates.io packages each crate alone). Keep the copies equal.
- **Plain, simple words** in all docs and notes.

## Overrides of the shared rules
- **Commit types:** `feat`, `fix`, `perf`, `style`, `chore` instead of `feature` / `bugfix`, the habit since the first commit (2026-10-09).
- **Pushing:** no need to ask before pushing `main`, because it publishes nothing (2026-10-10).
