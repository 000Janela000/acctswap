//! acctswap-core: the rules for switching the Claude account that every Claude Code session on a machine uses.
//!
//! No file, Keychain or network access lives here. The CLI crate implements the [`ports`] for each
//! system, so every rule is tested with in-memory fakes, and the pure parts ([`decision`], [`time`])
//! carry Kani proofs (`cargo kani -p acctswap-core`).

#![forbid(unsafe_code)]

pub mod decision;
pub mod login;
pub mod ports;
pub mod switcher;
pub mod time;
pub mod usage;

pub use switcher::{
    ActiveOutcome, Reading, Saved, Status, StatusRow, SwError, SwResult, Switched, Switcher,
};

#[cfg(test)]
mod tests;
