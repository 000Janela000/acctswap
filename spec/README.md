# The switch protocol, model-checked

`SwSwitch.tla` models sw switching the active Claude login while three other writers touch the same two places (the credential and `~/.claude.json`): a second sw run, Claude Code renewing tokens inside its renewal lock, and the user's `/login`, which takes no lock. The protocol it models is in `crates/acctswap-core/src/switcher.rs`.

Each protection is a constant, so one model shows both that the design holds and that every protection is needed.

| Configuration | Expected | Meaning |
|---|---|---|
| `Design_Refresh` | pass | Online, with renewals: no saved copy ever holds another account's tokens, saved copies of inactive accounts are never stale, tokens and email agree whenever nothing is in flight. |
| `Design_Login` | pass | Online, with a concurrent `/login`: no saved copy ever holds another account's tokens. |
| `Design_Both` | pass | Both at once (about 46,000 states). |
| `Offline_Refresh` | pass | Without the identity check, renewals are still safe. |
| `Offline_OnlyCcLock` | pass | Claude Code's renewal lock alone keeps two sw runs safe; sw's own lock is defence in depth. |
| `Offline_NoLocks` | `IdentityOK` violated | Without any lock or identity check, two sw runs corrupt a saved copy. |
| `NoCcLock_Refresh` | `FreshSaves` violated | Without Claude Code's renewal lock, a renewal during a switch leaves a stale saved copy. |
| `Offline_Login` | `IdentityOK` violated | Without the identity check, a `/login` half-way through is saved under the old email. This is the one case left offline. |

Run: `spec/check.sh [path/to/tla2tools.jar]` (needs Java 11+; TLA+ tools v1.7.4, sha256 `936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`). It prints `ok` per configuration when the result matches, and exits non-zero otherwise.

Assumptions the model makes about Claude Code, from how it behaves rather than from documentation: it renews tokens under the lock path `~/.claude/.oauth_refresh.lock` and does not renew while that path exists; a renewal makes the previous tokens stop working; `/login` writes the tokens before the email.
