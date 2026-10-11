# acctswap

Switch the Claude account that every Claude Code session on a machine uses, without a browser login.

> Not affiliated with or endorsed by Anthropic. Claude and Claude Code are trademarks of Anthropic, PBC. Read [Risks](#risks) before you use it.

Claude Code keeps one shared saved login per machine, and every open session re-reads it. That is why one `/login` moves every open tab. acctswap does the same swap from saved copies of each account's login, so switching takes a second, needs no browser, and keeps background work running.

The examples use the short name `sw` (see [Install](#install)).

```
sw                  switch to the other saved account (when there are two)
sw <name>           switch to the account with that short name, or part of its email
sw active           when the active account reads full, switch to the one with the most room
sw status           every saved account with its usage; saves a login sw has not seen
sw save             save the active login (after a /login of an account sw has not seen)
sw name <name>      give the active account a short name, e.g. `sw name me`
sw hook install     switch automatically when Claude refuses a request on a usage limit
sw hook uninstall   remove that hook
```

Inside a session, run it as `! sw`; it works even when the account is at its limit, because it does not use the model. Open sessions follow on their next message. Remote Control drops on any account change (Claude Code's own behaviour, also after `/login`), so run `/remote-control` in tabs that message other machines.

## Install

| With | Command |
|---|---|
| Homebrew (macOS, Linux) | `brew install 000Janela000/acctswap/acctswap` |
| npm | `npm install -g acctswap` |
| pip or uv | `pip install acctswap`, or `uv tool install acctswap` |
| cargo | `cargo binstall acctswap` (downloads the release) or `cargo install acctswap` (builds it) |
| By hand | download the archive for your system from the [releases](https://github.com/000Janela000/acctswap/releases), check it against `SHA256SUMS`, and put `acctswap` on your `PATH` |

Releases are built on GitHub Actions; `gh attestation verify <file> -R 000Janela000/acctswap` shows which workflow and commit built a file.

**The short name `sw`:** add a small script named `sw` next to your other commands, so it works in every shell and inside Claude Code (`! sw`), wherever the package manager put acctswap:

- macOS, Linux: `printf '#!/bin/sh\nexec acctswap "$@"\n' > ~/.local/bin/sw && chmod +x ~/.local/bin/sw`
- Windows: a copy of `acctswap.exe` named `sw.exe`, in the same folder. (Claude Code runs commands through Git Bash, which does not run a `sw.cmd` by the bare name `sw`.) Copy it again after each update.

**Then, once per account:** `/login` to it in Claude Code and run `sw save`; give each a short name with `sw name <name>`. To switch automatically on a usage limit: `sw hook install`.

**Coming from claude-sw** (the old name): the first run moves `~/.config/claude-sw` to `~/.config/acctswap`, and each login saved in the macOS Keychain moves to the new name the next time it is used. Run `sw hook install` once to replace the old hook.

From source: `cargo install --locked --path crates/acctswap`.

## How it works

| | macOS | Linux, Windows |
|---|---|---|
| Active login | Keychain item `Claude Code-credentials`; with a custom `CLAUDE_CONFIG_DIR`, `Claude Code-credentials-<first 8 hex digits of the folder's SHA-256>` | `~/.claude/.credentials.json` (or in `CLAUDE_CONFIG_DIR`) |
| Saved logins | Keychain items `acctswap` (account = email) | `~/.config/acctswap/saved/<email>.json`, owner-only (the folder too, on Linux) |
| Account record | `~/.claude.json` → `oauthAccount` | same |

A switch replaces only the Claude account part of the active login (`claudeAiOauth`); MCP server logins stored next to it stay. It also sets `oauthAccount` in `~/.claude.json` and drops that account's cached data.

Usage comes from `https://api.anthropic.com/api/oauth/usage`, the endpoint Claude Code's own usage meter calls, through the system's `curl` (built into macOS, Linux distributions and Windows 10+). The token goes to curl in a config on stdin, never on the command line. The active account is asked live; others reuse a reading younger than two minutes, because the endpoint rate-limits frequent callers.

### The switch protocol

1. Read the signed-in email and tokens; ask `https://api.anthropic.com/api/oauth/profile` whose tokens they are. A mismatch means a `/login` is half-way through: nothing is saved, and sw retries a moment later.
2. Do every network call without holding a lock.
3. Take Claude Code's token-renewal lock (`~/.claude/.oauth_refresh.lock`), re-read, and commit only if nothing changed since step 1: save the tokens being left, write the target's tokens, then its account record. If writing the account record fails, the previous login is put back.

The whole run also holds sw's own lock, so tabs that hit a limit together switch once.

### When an account counts as used up

The usage endpoint rounds to whole percents, and an account that reads 100% can still answer for a while. Two accounts can both read "weekly 100%, critical" while only one of them is refused. So a reading only ranks accounts; it never makes sw call an account used up.

1. The hook (`sw hook install`) runs `sw active --from-hook` when Claude refuses a request on a rate limit. That refusal is the signal: sw skips the active account until the first of its windows that reads full resets (30 minutes when none reads full, for example a limit on one model), and switches to the best account it has not seen refused, even one that reads 100%.
2. sw never switches to an account it has seen refused, so it cannot bounce between two accounts that are both turned away. When every account was refused, it stays and says which one frees up first.
3. A refusal reported within two minutes of a switch belongs to the account just left, when that account reads full or its usage is unknown: a request sent before the switch came back refused after it. sw marks that account and stays. When the account just left still has room, the refusal is the active account's, as in rule 1.
4. `sw active` by hand, without a refusal, moves off an account that reads full only to an account with room; `sw <name>` always switches.

## What is verified

| Check | What it covers | Where |
|---|---|---|
| Model checking (TLA+ / TLC) | The protocol against a second sw run, token renewals and `/login`, in every interleaving; and that removing any one protection breaks it | `spec/`, `spec/check.sh` |
| Proofs (Kani) | For every input within bounds: the account choice never leaves an account with room that was not refused; never picks a refused account and always picks the best-ranked one; leaves a refused account whenever another was not refused; says "every account was refused" only when true; a refusal always ends in the future, at a full window's reset or after the fallback. Timestamp parsing never panics; dates and timestamps round-trip exactly | `#[cfg(kani)]` in `crates/acctswap-core` |
| Tests | 100% of lines, functions and branches of the core, including every port failing on every call | `cargo llvm-cov -p acctswap-core` |
| Mutation testing | Bugs planted in the core are caught by the tests | `cargo mutants -p acctswap-core` |
| Lints | Clippy pedantic, no `unsafe`, no `unwrap` outside tests | `Cargo.toml` `[workspace.lints]` |
| Dependencies | One JSON library and its three helpers; licences, sources and advisories checked | `deny.toml` |

## What acctswap does with your login

- It reads and writes Claude Code's saved login on this machine, and keeps a copy of each account's login: in the macOS Keychain, or in owner-only files elsewhere.
- The tokens go to one place only: `api.anthropic.com`, to read each account's usage and to ask whose tokens they are. They go through the system's `curl`, in a config on stdin, never on a command line.
- Nothing else is sent anywhere. There is no telemetry and no server of ours.

## Risks

Anthropic's rules may not allow a tool like this. Use it at your own risk.

- Claude Code's [legal page](https://code.claude.com/docs/en/legal-and-compliance) says: "developers may not collect, store, or intermediate Claude.ai credentials or session tokens". acctswap keeps copies of your login tokens on your own machine.
- Anthropic's [Usage Policy](https://www.anthropic.com/legal/aup), effective 2026-11-12, forbids to "intentionally bypass or interfere with capabilities, restrictions, or guardrails established within our products, including by circumvention of the account sign-up or log-in process". Switching to another account when one hits its usage limit may be read that way.
- Anthropic says it may enforce its rules "without prior notice". That could mean limits on, or suspension of, the accounts you use acctswap with.

Having several accounts is allowed: Claude Code's own docs show [logging in with multiple accounts](https://code.claude.com/docs/en/authentication#log-in-with-multiple-accounts), one config folder per account. That is the documented way, if you prefer it.

## Limits

- It relies on how Claude Code stores the login today. Anthropic does not document that, so a Claude Code update can change it. If `sw status` starts failing, fall back to `/login`. Fixes are best effort.
- Offline, sw cannot ask who owns the tokens. If you are offline and run `/login` at the same moment as a switch, the half-written login could be saved under the old email (`spec/` configuration `Offline_Login`). Online, this cannot happen.
- A saved login renews itself for about 28 days after its last use. An account unused for longer needs one `/login`; `sw status` warns three days ahead.
- On macOS, a custom `CLAUDE_CONFIG_DIR` is switched through its own Keychain item, named the way Claude Code names it today (not documented).
- On macOS, when the Keychain refused a write (for example over SSH), Claude Code saves the login to `.credentials.json` instead. acctswap then stops and says how to move the login back, because it can't tell which of the two Claude Code reads.

## Layout

```
crates/acctswap-core   the rules: no file, Keychain or network access; ports, decision, protocol, proofs
crates/acctswap        the acctswap binary: Keychain / file / curl adapters, the hook, the commands
spec/                  the TLA+ model and its configurations
```

## Licence

Either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
