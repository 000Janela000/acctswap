//! The ports implemented for real machines: macOS Keychain, login files, `~/.claude.json`,
//! the usage endpoint, the names file and the lock.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use acctswap_core::ports::{
    AccountApi, ApiCall, ConfigFile, CredentialStore, NamesStore, PortResult, SwapLock,
};
use serde_json::Value;

/// The program's name: its command, its folder in `~/.config` and its Keychain items.
pub const NAME: &str = "acctswap";
/// The name before 0.2.0. Its folder and Keychain items move to the new name on first use.
pub const OLD_NAME: &str = "claude-sw";

/// Where things live on this machine. Overridable for tests:
/// `SW_CONFIG_DIR` (a Claude config folder), `SW_HOME` (sw's own folder), `SW_STORE=file`.
pub struct Paths {
    pub claude_dir: PathBuf,
    pub claude_json: PathBuf,
    pub sw_dir: PathBuf,
    /// The Keychain item holding Claude Code's login (macOS), or `None` for the file store.
    pub keychain: Option<String>,
}

impl Paths {
    pub fn detect() -> PortResult<Paths> {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .ok_or("cannot find the home folder (HOME / USERPROFILE)")?;
        let claude_config_dir = std::env::var_os("CLAUDE_CONFIG_DIR");
        let custom_dir = std::env::var_os("SW_CONFIG_DIR").or_else(|| claude_config_dir.clone());
        let (claude_dir, claude_json) = match custom_dir {
            Some(d) => {
                let d = PathBuf::from(d);
                (d.clone(), d.join(".claude.json"))
            }
            None => (home.join(".claude"), home.join(".claude.json")),
        };
        let sw_dir = if let Some(d) = std::env::var_os("SW_HOME") {
            PathBuf::from(d)
        } else {
            let config = home.join(".config");
            adopt_old_folder(&config.join(OLD_NAME), &config.join(NAME))?;
            config.join(NAME)
        };
        let keychain = if cfg!(target_os = "macos")
            && std::env::var_os("SW_CONFIG_DIR").is_none()
            && std::env::var("SW_STORE").as_deref() != Ok("file")
        {
            Some(keychain_service(claude_config_dir.as_deref())?)
        } else {
            None
        };
        Ok(Paths {
            claude_dir,
            claude_json,
            sw_dir,
            keychain,
        })
    }

    /// True when Claude Code keeps the login in `.credentials.json` although this Mac uses the
    /// Keychain: it does that when the Keychain refuses a write, for example over SSH. Which of
    /// the two it then reads is not documented, so sw must not switch only one of them.
    pub fn login_file_in_use(&self) -> bool {
        self.keychain.is_some()
            && read_optional(&self.claude_dir.join(".credentials.json"))
                .ok()
                .flatten()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .is_some_and(|v| v["claudeAiOauth"].is_object())
    }
}

/// Moves the folder of the version called claude-sw to the new name, once.
fn adopt_old_folder(old: &Path, new: &Path) -> PortResult<()> {
    if new.exists() || !old.exists() {
        return Ok(());
    }
    match fs::rename(old, new) {
        Err(_) if new.exists() => Ok(()), // another run moved it a moment ago
        r => r.map_err(|e| format!("moving {} to {}: {e}", old.display(), new.display())),
    }
}

/// Claude Code's Keychain item: `Claude Code-credentials` for the default folder, and for a
/// custom `CLAUDE_CONFIG_DIR` the same name plus `-` and the first 8 hex digits of the SHA-256
/// of the folder as given.
fn keychain_service(custom_dir: Option<&std::ffi::OsStr>) -> PortResult<String> {
    let Some(dir) = custom_dir else {
        return Ok(ACTIVE_SERVICE.to_owned());
    };
    let mut child = Command::new("shasum")
        .args(["-a", "256"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("running shasum: {e}"))?;
    child
        .stdin
        .take()
        .expect("piped")
        .write_all(dir.as_encoded_bytes())
        .map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let hex = String::from_utf8_lossy(&out.stdout);
    match hex.get(..8) {
        Some(h) if out.status.success() && h.bytes().all(|b| b.is_ascii_hexdigit()) => {
            Ok(format!("{ACTIVE_SERVICE}-{h}"))
        }
        _ => Err("could not name the Keychain item for CLAUDE_CONFIG_DIR (shasum failed)".into()),
    }
}

/// Writes through a temporary file and a rename, so a crash never leaves half a file.
/// On Unix the file is readable by its owner only.
pub fn write_atomic(path: &Path, data: &[u8]) -> PortResult<()> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("no parent folder for {}", path.display()))?;
    fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let tmp = dir.join(format!(
        ".{}.sw-tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file")
    ));
    {
        let mut f = File::create(&tmp).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
        }
        f.write_all(data)
            .and_then(|()| f.sync_all())
            .map_err(|e| format!("writing {}: {e}", tmp.display()))?;
    }
    fs::rename(&tmp, path).map_err(|e| format!("replacing {}: {e}", path.display()))
}

fn read_optional(path: &Path) -> PortResult<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("reading {}: {e}", path.display())),
    }
}

// ---------- logins kept in files (Linux, Windows, tests) ----------

pub struct FileStore {
    active: PathBuf,
    saved_dir: PathBuf,
}

impl FileStore {
    pub fn new(p: &Paths) -> FileStore {
        FileStore {
            active: p.claude_dir.join(".credentials.json"),
            saved_dir: p.sw_dir.join("saved"),
        }
    }

    fn saved_path(&self, email: &str) -> PathBuf {
        self.saved_dir.join(format!("{email}.json"))
    }
}

impl CredentialStore for FileStore {
    fn read_active(&self) -> PortResult<Option<String>> {
        read_optional(&self.active)
    }
    fn write_active(&self, value: &str) -> PortResult<()> {
        write_atomic(&self.active, value.as_bytes())
    }
    fn read_saved(&self, email: &str) -> PortResult<Option<String>> {
        read_optional(&self.saved_path(email))
    }
    fn write_saved(&self, email: &str, value: &str) -> PortResult<()> {
        write_atomic(&self.saved_path(email), value.as_bytes())
    }
    fn list_saved(&self) -> PortResult<Vec<String>> {
        let Ok(entries) = fs::read_dir(&self.saved_dir) else {
            return Ok(vec![]);
        };
        let mut out: Vec<String> = entries
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter_map(|n| n.strip_suffix(".json").map(str::to_owned))
            .filter(|n| n.contains('@'))
            .collect();
        out.sort();
        Ok(out)
    }
}

// ---------- logins kept in the macOS Keychain ----------

/// Uses Apple's `security` command, like Claude Code itself. Reading Claude Code's item through the
/// Keychain API from a new program would show an "Allow access?" prompt; `security` does not.
/// Saved accounts are listed from a small index file (emails only), so sw never dumps the Keychain
/// in normal use; the dump runs once, to build the index after an upgrade from the Python version.
pub struct KeychainStore {
    user: String,
    active: String,
    index: PathBuf,
}

const ACTIVE_SERVICE: &str = "Claude Code-credentials";
const SAVED_SERVICE: &str = NAME;
const OLD_SAVED_SERVICE: &str = OLD_NAME;

impl KeychainStore {
    pub fn new(p: &Paths, active: &str) -> KeychainStore {
        KeychainStore {
            user: std::env::var("USER").unwrap_or_default(),
            active: active.to_owned(),
            index: p.sw_dir.join("accounts.json"),
        }
    }

    fn load_index(&self) -> PortResult<Option<BTreeSet<String>>> {
        match read_optional(&self.index)? {
            Some(s) => serde_json::from_str(&s)
                .map(Some)
                .map_err(|e| format!("{} is not valid JSON: {e}", self.index.display())),
            None => Ok(None),
        }
    }

    fn save_index(&self, emails: &BTreeSet<String>) -> PortResult<()> {
        write_atomic(
            &self.index,
            serde_json::to_string_pretty(emails)
                .map_err(|e| e.to_string())?
                .as_bytes(),
        )
    }

    fn read(service: &str, account: &str) -> PortResult<Option<String>> {
        let out = Command::new("security")
            .args(["find-generic-password", "-s", service, "-a", account, "-w"])
            .output()
            .map_err(|e| format!("running security: {e}"))?;
        match out.status.code() {
            Some(0) => Ok(Some(
                String::from_utf8_lossy(&out.stdout)
                    .trim_end_matches('\n')
                    .to_owned(),
            )),
            Some(44) => Ok(None), // errSecItemNotFound
            _ => Err(format!(
                "reading Keychain item {service:?}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )),
        }
    }

    fn delete(service: &str, account: &str) -> PortResult<()> {
        let out = Command::new("security")
            .args(["delete-generic-password", "-s", service, "-a", account])
            .output()
            .map_err(|e| format!("running security: {e}"))?;
        match out.status.code() {
            Some(0 | 44) => Ok(()),
            _ => Err(format!(
                "deleting Keychain item {service:?}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )),
        }
    }

    /// The secret goes as hex on stdin (`security -i`), so it never shows in a process list.
    fn write(service: &str, account: &str, value: &str) -> PortResult<()> {
        let hex = value
            .bytes()
            .fold(String::with_capacity(value.len() * 2), |mut s, b| {
                let _ = write!(s, "{b:02x}");
                s
            });
        let cmd = format!("add-generic-password -U -s \"{service}\" -a \"{account}\" -X {hex}\n");
        let mut child = Command::new("security")
            .arg("-i")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("running security: {e}"))?;
        child
            .stdin
            .take()
            .expect("piped")
            .write_all(cmd.as_bytes())
            .map_err(|e| e.to_string())?;
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if !out.status.success() || Self::read(service, account)?.as_deref() != Some(value) {
            return Err(format!(
                "could not write Keychain item {service:?}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(())
    }
}

impl CredentialStore for KeychainStore {
    fn read_active(&self) -> PortResult<Option<String>> {
        Self::read(&self.active, &self.user)
    }
    fn write_active(&self, value: &str) -> PortResult<()> {
        Self::write(&self.active, &self.user, value)
    }
    fn read_saved(&self, email: &str) -> PortResult<Option<String>> {
        if let Some(v) = Self::read(SAVED_SERVICE, email)? {
            return Ok(Some(v));
        }
        // Saved before the rename: move it to the new name.
        let Some(v) = Self::read(OLD_SAVED_SERVICE, email)? else {
            return Ok(None);
        };
        Self::write(SAVED_SERVICE, email, &v)?;
        Self::delete(OLD_SAVED_SERVICE, email)?;
        Ok(Some(v))
    }
    fn write_saved(&self, email: &str, value: &str) -> PortResult<()> {
        Self::write(SAVED_SERVICE, email, value)?;
        // list_saved builds the index from the Keychain when it is missing, so no account is lost.
        let mut index: BTreeSet<String> = self.list_saved()?.into_iter().collect();
        if index.insert(email.to_owned()) {
            self.save_index(&index)?;
        }
        Ok(())
    }
    fn list_saved(&self) -> PortResult<Vec<String>> {
        if let Some(index) = self.load_index()? {
            return Ok(index.into_iter().collect());
        }
        let out = Command::new("security")
            .arg("dump-keychain")
            .output()
            .map_err(|e| format!("running security: {e}"))?;
        let found = saved_accounts_in_dump(&String::from_utf8_lossy(&out.stdout));
        self.save_index(&found.iter().cloned().collect())?;
        Ok(found)
    }
}

/// Accounts of sw's items (under the new or the old name) in `security dump-keychain` output
/// (attributes only, no secrets).
pub fn saved_accounts_in_dump(dump: &str) -> Vec<String> {
    let mut out = Vec::new();
    for block in dump.split("keychain: ") {
        let attr = |name: &str| {
            block.lines().map(str::trim).find_map(|l| {
                l.strip_prefix(&format!("\"{name}\"<blob>=\""))
                    .map(|v| v.trim_end_matches('"').to_owned())
            })
        };
        if attr("svce").is_some_and(|s| s == SAVED_SERVICE || s == OLD_SAVED_SERVICE)
            && let Some(acct) = attr("acct")
        {
            out.push(acct);
        }
    }
    out.sort();
    out.dedup();
    out
}

// ---------- ~/.claude.json ----------

pub struct ClaudeJson(pub PathBuf);

impl ConfigFile for ClaudeJson {
    fn load(&self) -> PortResult<Value> {
        match read_optional(&self.0)? {
            Some(s) => serde_json::from_str(&s)
                .map_err(|e| format!("{} is not valid JSON: {e}", self.0.display())),
            None => Ok(Value::Object(serde_json::Map::default())),
        }
    }
    fn save(&self, value: &Value) -> PortResult<()> {
        let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
        write_atomic(&self.0, text.as_bytes())
    }
}

// ---------- the usage endpoint ----------

/// The API Claude Code itself calls for the usage meter and the signed-in account.
pub const API_BASE: &str = "https://api.anthropic.com/api/oauth";

/// Calls the API through the system's `curl` (built into macOS, Linux distributions and
/// Windows 10+), so sw carries no TLS code of its own. The token goes in a curl config on stdin,
/// never on the command line. Answers are remembered for the rest of the run, so each account is
/// asked at most once per endpoint.
#[derive(Default)]
pub struct CurlApi {
    seen: RefCell<HashMap<(&'static str, String), Option<Value>>>,
}

impl CurlApi {
    fn call(path: &str, access_token: &str) -> Option<Value> {
        let base = std::env::var("SW_API_BASE").unwrap_or_else(|_| API_BASE.to_owned());
        let url = format!("{base}/{path}");
        if [access_token, url.as_str()]
            .iter()
            .any(|v| v.contains(['"', '\\', '\n', '\r']))
        {
            return None; // never let a value break out of its quoted curl config line
        }
        let config = format!(
            "url = \"{url}\"\nheader = \"Authorization: Bearer {access_token}\"\n\
             header = \"anthropic-beta: oauth-2025-04-20\"\nsilent\nfail\nmax-time = 15\n"
        );
        let mut child = Command::new("curl")
            .args(["--disable", "--config", "-"]) // --disable: ignore any ~/.curlrc
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        child.stdin.take()?.write_all(config.as_bytes()).ok()?;
        let out = child.wait_with_output().ok()?;
        out.status
            .success()
            .then(|| serde_json::from_slice(&out.stdout).ok())
            .flatten()
    }

    fn cached(&self, path: &'static str, access_token: &str) -> Option<Value> {
        let key = (path, access_token.to_owned());
        if let Some(v) = self.seen.borrow().get(&key) {
            return v.clone();
        }
        let v = Self::call(path, access_token);
        self.seen.borrow_mut().insert(key, v.clone());
        v
    }
}

impl AccountApi for CurlApi {
    fn usage(&self, access_token: &str) -> Option<Value> {
        self.cached("usage", access_token)
    }

    fn email(&self, access_token: &str) -> Option<String> {
        self.cached("profile", access_token)?["account"]["email"]
            .as_str()
            .map(str::to_owned)
    }
    /// Makes every call not answered yet at the same time, one curl each.
    fn prefetch(&self, calls: &[ApiCall<'_>]) {
        let todo: Vec<(&'static str, String)> = calls
            .iter()
            .map(|c| match c {
                ApiCall::Usage(t) => ("usage", (*t).to_owned()),
                ApiCall::Email(t) => ("profile", (*t).to_owned()),
            })
            .filter(|k| !self.seen.borrow().contains_key(k))
            .collect();
        if todo.len() < 2 {
            return; // nothing to overlap
        }
        let answers: Vec<Option<Value>> = std::thread::scope(|s| {
            let handles: Vec<_> = todo
                .iter()
                .map(|(path, token)| s.spawn(move || Self::call(path, token)))
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().ok().flatten())
                .collect()
        });
        self.seen.borrow_mut().extend(todo.into_iter().zip(answers));
    }
}

// ---------- Claude Code's token-renewal lock ----------

/// Takes `~/.claude/.oauth_refresh.lock`, the path Claude Code locks while it renews tokens.
/// Creating it fails while Claude Code holds it (whether Claude Code made a file or a folder
/// there), and Claude Code waits while sw holds it. sw holds it for a few Keychain or file writes,
/// well under a second, and never removes a lock it did not create.
pub struct RenewalLock {
    path: PathBuf,
}

impl RenewalLock {
    pub fn new(p: &Paths) -> RenewalLock {
        RenewalLock {
            path: p.claude_dir.join(".oauth_refresh.lock"),
        }
    }
}

struct RenewalGuard(PathBuf);

impl Drop for RenewalGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}

impl SwapLock for RenewalLock {
    fn hold(&self) -> PortResult<Box<dyn std::any::Any>> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match fs::create_dir(&self.path) {
                Ok(()) => return Ok(Box::new(RenewalGuard(self.path.clone()))),
                Err(e)
                    if e.kind() == std::io::ErrorKind::AlreadyExists
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Err(format!(
                        "Claude Code has been renewing its login for 10 s ({} exists); try again in a moment",
                        self.path.display()
                    ));
                }
                Err(e) => return Err(format!("taking {}: {e}", self.path.display())),
            }
        }
    }
}

// ---------- names and the lock ----------

pub struct NamesFile(pub PathBuf);

impl NamesStore for NamesFile {
    fn load(&self) -> PortResult<BTreeMap<String, String>> {
        match read_optional(&self.0)? {
            Some(s) => serde_json::from_str(&s)
                .map_err(|e| format!("{} is not valid JSON: {e}", self.0.display())),
            None => Ok(BTreeMap::new()),
        }
    }
    fn save(&self, names: &BTreeMap<String, String>) -> PortResult<()> {
        write_atomic(
            &self.0,
            serde_json::to_string_pretty(names)
                .map_err(|e| e.to_string())?
                .as_bytes(),
        )
    }
}

/// One sw at a time: several tabs can hit a limit together and run `sw active` at once.
/// Also keeps sw's folder private on Unix: the saved copies inside are named by email.
pub fn lock(sw_dir: &Path) -> PortResult<File> {
    fs::create_dir_all(sw_dir).map_err(|e| format!("creating {}: {e}", sw_dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(sw_dir, fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("protecting {}: {e}", sw_dir.display()))?;
    }
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(sw_dir.join(".lock"))
        .map_err(|e| e.to_string())?;
    f.lock()
        .map_err(|e| format!("waiting for the sw lock: {e}"))?;
    Ok(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_saved_accounts_in_a_keychain_dump() {
        let dump = r#"keychain: "/Users/x/Library/Keychains/login.keychain-db"
version: 512
class: "genp"
attributes:
    "acct"<blob>="saba"
    "svce"<blob>="Claude Code-credentials"
keychain: "/Users/x/Library/Keychains/login.keychain-db"
class: "genp"
attributes:
    "acct"<blob>="me@home.example"
    "svce"<blob>="claude-sw"
keychain: "/Users/x/Library/Keychains/login.keychain-db"
class: "genp"
attributes:
    "acct"<blob>="me@corp.example"
    "svce"<blob>="acctswap"
"#;
        assert_eq!(
            saved_accounts_in_dump(dump),
            vec!["me@corp.example", "me@home.example"],
            "items saved under the old name count too"
        );
    }

    #[test]
    fn the_old_folder_moves_to_the_new_name_once() {
        let dir = std::env::temp_dir().join(format!("sw-adopt-{}", std::process::id()));
        let (old, new) = (dir.join(OLD_NAME), dir.join(NAME));
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("names.json"), "{}").unwrap();
        adopt_old_folder(&old, &new).unwrap();
        assert!(new.join("names.json").exists() && !old.exists());
        // Once the new folder exists, an old one is left alone.
        fs::create_dir_all(&old).unwrap();
        adopt_old_folder(&old, &new).unwrap();
        assert!(old.exists() && new.join("names.json").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_login_file_next_to_the_keychain_is_noticed() {
        let dir = std::env::temp_dir().join(format!("sw-file-{}", std::process::id()));
        let mut p = Paths {
            claude_dir: dir.clone(),
            claude_json: dir.join(".claude.json"),
            sw_dir: dir.join("sw"),
            keychain: Some(ACTIVE_SERVICE.into()),
        };
        assert!(!p.login_file_in_use(), "no file");
        write_atomic(&dir.join(".credentials.json"), b"{}").unwrap();
        assert!(!p.login_file_in_use(), "a file emptied by /logout");
        write_atomic(
            &dir.join(".credentials.json"),
            br#"{"claudeAiOauth":{"accessToken":"t"}}"#,
        )
        .unwrap();
        assert!(p.login_file_in_use());
        p.keychain = None;
        assert!(
            !p.login_file_in_use(),
            "the file is the store on Linux and Windows"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn names_the_keychain_item_of_a_custom_config_folder() {
        assert_eq!(keychain_service(None).unwrap(), "Claude Code-credentials");
        assert_eq!(
            keychain_service(Some(std::ffi::OsStr::new("/Users/x/.claude-work"))).unwrap(),
            "Claude Code-credentials-74ed04d5"
        );
    }

    #[test]
    fn file_store_round_trip() {
        let dir = std::env::temp_dir().join(format!("sw-test-{}", std::process::id()));
        let p = Paths {
            claude_dir: dir.join("claude"),
            claude_json: dir.join(".claude.json"),
            sw_dir: dir.join("sw"),
            keychain: None,
        };
        let s = FileStore::new(&p);
        assert_eq!(s.read_active().unwrap(), None);
        s.write_active("{\"a\":1}").unwrap();
        s.write_saved("me@x.com", "{}").unwrap();
        assert_eq!(s.read_active().unwrap().as_deref(), Some("{\"a\":1}"));
        assert_eq!(s.list_saved().unwrap(), vec!["me@x.com"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join("claude/.credentials.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_lock_keeps_the_sw_folder_private() {
        let dir = std::env::temp_dir().join(format!("sw-lock-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // A folder an earlier version created readable by everyone is tightened.
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
            drop(lock(&dir).unwrap());
            let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
        #[cfg(not(unix))]
        drop(lock(&dir).unwrap());
        fs::remove_dir_all(&dir).unwrap();
    }
}
