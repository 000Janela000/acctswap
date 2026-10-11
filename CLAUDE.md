@AGENTS.md

# Claude Code — extra notes

The shared rules are in `AGENTS.md` (imported above). This part is only for Claude Code.

- **Cargo is not on the default PATH of Claude's shell on the Mac.** Start commands with `export PATH="$HOME/.cargo/bin:$PATH"`.
- **Kani is slow on iterator chains.** Code that a proof checks uses plain `match` and short fixed loops; one `flat_map`/`filter`/`min` chain took over an hour where a `match` takes 0.3 seconds (2026-10-10).
- **Long checks** (Kani, `cargo mutants`, CI): run them in the background and wait for the notification.
- **Cross-machine work** (PC, server) goes through `SendMessage` to that machine's session, in the format of `../_shared/standards/agent-coordination.md`.
