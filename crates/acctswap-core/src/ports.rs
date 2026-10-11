//! What the core needs from the machine. Each system implements these in the CLI crate:
//! the macOS Keychain or a login file, `~/.claude.json`, the usage endpoint, the names file.
//!
//! Every method returns [`PortResult`]; the error is a human-readable reason, shown as is.

use std::collections::BTreeMap;

use serde_json::Value;

/// Result of a port call; the error says what failed, in words.
pub type PortResult<T> = Result<T, String>;

/// Where logins are kept: the active one every session reads, and one saved copy per account.
/// Values are the raw JSON strings Claude Code stores.
pub trait CredentialStore {
    /// The active login, or `None` when nobody is logged in.
    ///
    /// # Errors
    /// When the store cannot be read.
    fn read_active(&self) -> PortResult<Option<String>>;

    /// Replaces the active login.
    ///
    /// # Errors
    /// When the store cannot be written.
    fn write_active(&self, value: &str) -> PortResult<()>;

    /// The saved copy for `email`, or `None` when there is none.
    ///
    /// # Errors
    /// When the store cannot be read.
    fn read_saved(&self, email: &str) -> PortResult<Option<String>>;

    /// Creates or replaces the saved copy for `email`.
    ///
    /// # Errors
    /// When the store cannot be written.
    fn write_saved(&self, email: &str, value: &str) -> PortResult<()>;

    /// Emails of every saved copy, sorted.
    ///
    /// # Errors
    /// When the store cannot be listed.
    fn list_saved(&self) -> PortResult<Vec<String>>;
}

/// `~/.claude.json` (or `<CLAUDE_CONFIG_DIR>/.claude.json`).
pub trait ConfigFile {
    /// The whole file; an empty object when it does not exist.
    ///
    /// # Errors
    /// When the file cannot be read or is not JSON.
    fn load(&self) -> PortResult<Value>;

    /// Replaces the whole file.
    ///
    /// # Errors
    /// When the file cannot be written.
    fn save(&self, value: &Value) -> PortResult<()>;
}

/// The usage and profile endpoints. Both answer `None` when the request fails; the core then
/// falls back to the last saved usage, and skips the identity check.
pub trait AccountApi {
    /// Usage JSON for the account behind `access_token`.
    fn usage(&self, access_token: &str) -> Option<Value>;

    /// Email of the account `access_token` belongs to.
    fn email(&self, access_token: &str) -> Option<String>;

    /// A hint listing the calls a command is about to make, so an implementation can make them
    /// at the same time. Answers must stay the same as without the hint.
    fn prefetch(&self, calls: &[ApiCall<'_>]);
}

/// One call the core is about to make, for [`AccountApi::prefetch`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiCall<'a> {
    /// [`AccountApi::usage`] with this token.
    Usage(&'a str),
    /// [`AccountApi::email`] with this token.
    Email(&'a str),
}

/// Claude Code's token-renewal lock. While it is held, Claude Code does not renew tokens, so a
/// switch's read and write of the active login cannot be split by a renewal.
pub trait SwapLock {
    /// Takes the lock; dropping the returned guard releases it.
    ///
    /// # Errors
    /// When the lock stays taken for too long.
    fn hold(&self) -> PortResult<Box<dyn std::any::Any>>;
}

/// Short names for accounts: name -> email.
pub trait NamesStore {
    /// All names; empty when none were given.
    ///
    /// # Errors
    /// When the names cannot be read.
    fn load(&self) -> PortResult<BTreeMap<String, String>>;

    /// Replaces all names.
    ///
    /// # Errors
    /// When the names cannot be written.
    fn save(&self, names: &BTreeMap<String, String>) -> PortResult<()>;
}
