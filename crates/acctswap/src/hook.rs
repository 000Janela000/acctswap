//! `sw hook install|uninstall|status`: the `StopFailure` hook that runs `acctswap active` when a
//! turn ends on a rate-limit error, and the log it writes.

use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use acctswap_core::ports::PortResult;
use acctswap_core::time::format_utc;
use serde_json::{Value, json};

use crate::adapters::{NAME, OLD_NAME, write_atomic};

const EVENT: &str = "StopFailure";

fn load(settings: &Path) -> PortResult<Value> {
    match std::fs::read_to_string(settings) {
        Ok(s) => serde_json::from_str(&s)
            .map_err(|e| format!("{} is not valid JSON: {e}", settings.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("reading {}: {e}", settings.display())),
    }
}

/// The command of a hook entry when it is ours: it mentions acctswap, or claude-sw (the old name,
/// also older shell versions).
fn our_command(entry: &Value) -> Option<&str> {
    entry["hooks"].as_array()?.iter().find_map(|h| {
        h["command"]
            .as_str()
            .filter(|c| c.contains(NAME) || c.contains(OLD_NAME))
    })
}

/// Removes every `StopFailure` entry that is ours.
fn without_ours(list: &[Value]) -> Vec<Value> {
    list.iter()
        .filter(|entry| our_command(entry).is_none())
        .cloned()
        .collect()
}

/// Installs the hook. `program` is how the hook starts sw: the bare name when it is on `PATH`, so
/// the hook keeps working when npm, pip or Homebrew move the program; otherwise its quoted path.
pub fn install(settings: &Path, program: &str) -> PortResult<()> {
    let mut v = load(settings)?;
    let hooks = v
        .as_object_mut()
        .ok_or("settings.json is not an object")?
        .entry("hooks")
        .or_insert_with(|| json!({}));
    let list = hooks[EVENT].as_array().cloned().unwrap_or_default();
    let mut list = without_ours(&list);
    list.push(json!({
        "matcher": "rate_limit",
        "hooks": [{"type": "command", "command": format!("{program} active --from-hook"), "timeout": 60}]
    }));
    hooks[EVENT] = Value::Array(list);
    save(settings, &v)
}

/// Writes settings.json pretty-printed and ending in a newline, as an editor leaves it.
fn save(settings: &Path, v: &Value) -> PortResult<()> {
    let mut text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    text.push('\n');
    write_atomic(settings, text.as_bytes())
}

pub fn uninstall(settings: &Path) -> PortResult<bool> {
    let mut v = load(settings)?;
    let Some(list) = v["hooks"][EVENT].as_array().cloned() else {
        return Ok(false);
    };
    let kept = without_ours(&list);
    let removed = kept.len() != list.len();
    if kept.is_empty() {
        v["hooks"].as_object_mut().map(|h| h.remove(EVENT));
    } else {
        v["hooks"][EVENT] = Value::Array(kept);
    }
    save(settings, &v)?;
    Ok(removed)
}

/// The command of the installed hook, if there is one.
pub fn installed(settings: &Path) -> Option<String> {
    let v = load(settings).ok()?;
    v["hooks"][EVENT]
        .as_array()?
        .iter()
        .find_map(our_command)
        .map(str::to_owned)
}

/// Whether the program a hook command starts can be found: a quoted path must exist, a bare name
/// must be on `path_var` (the `PATH` the hook will see).
pub fn finds_program(command: &str, path_var: Option<&OsStr>) -> bool {
    match command.strip_prefix('"') {
        Some(rest) => rest
            .split_once('"')
            .is_some_and(|(path, _)| Path::new(path).is_file()),
        None => command
            .split_whitespace()
            .next()
            .is_some_and(|name| on_path(name, path_var).is_some()),
    }
}

/// The first file called `name` (on Windows also with an extension from `PATHEXT`) in `path_var`.
pub fn on_path(name: &str, path_var: Option<&OsStr>) -> Option<PathBuf> {
    let exts: Vec<String> = if cfg!(windows) {
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        std::iter::once(String::new())
            .chain(pathext.split(';').map(str::to_owned))
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(path_var?).find_map(|dir| {
        exts.iter()
            .map(|ext| dir.join(format!("{name}{ext}")))
            .find(|p| p.is_file())
    })
}

/// Reads the hook's input from stdin and appends it with the result to `<claude dir>/logs/acctswap.log`.
pub fn log(claude_dir: &Path, now: i64, result: &str) {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let dir = claude_dir.join("logs");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(format!("{NAME}.log")))
    {
        let _ = writeln!(
            f,
            "\n== {} UTC rate-limit hook\n{}\n{result}",
            format_utc(now),
            input.trim()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_replaces_older_entries_and_keeps_other_hooks() {
        let dir = std::env::temp_dir().join(format!("sw-hook-{}", std::process::id()));
        let settings = dir.join("settings.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &settings,
            json!({"hooks": {
                "SessionStart": [{"hooks": [{"type": "command", "command": "node sync-check.mjs"}]}],
                "StopFailure": [{"matcher": "rate_limit", "hooks": [{"type": "command", "command": "sh -c '~/.local/bin/claude-sw active'"}]}]
            }, "theme": "dark"})
            .to_string(),
        )
        .unwrap();
        install(&settings, "acctswap").unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        let list = v["hooks"]["StopFailure"].as_array().unwrap();
        assert_eq!(
            list.len(),
            1,
            "the old shell entry is replaced, not duplicated"
        );
        assert_eq!(
            list[0]["hooks"][0]["command"],
            "acctswap active --from-hook"
        );
        assert!(v["hooks"]["SessionStart"].is_array());
        assert_eq!(v["theme"], "dark");
        let text = std::fs::read_to_string(&settings).unwrap();
        assert!(
            text.find("\"hooks\"") < text.find("\"theme\""),
            "the user's key order is kept"
        );
        assert!(text.ends_with("}\n"), "ends in a newline");
        assert_eq!(
            installed(&settings).as_deref(),
            Some("acctswap active --from-hook")
        );
        assert!(uninstall(&settings).unwrap());
        assert_eq!(installed(&settings), None);
        assert!(std::fs::read_to_string(&settings).unwrap().ends_with("}\n"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_hook_command_finds_its_program_by_path_or_by_name() {
        let dir = std::env::temp_dir().join(format!("sw-path-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("acctswap");
        let path_var = std::env::join_paths([dir.clone()]).unwrap();
        let path_var = Some(path_var.as_os_str());
        assert!(!finds_program("acctswap active --from-hook", path_var));
        std::fs::write(&exe, "").unwrap();
        assert!(finds_program("acctswap active --from-hook", path_var));
        assert!(!finds_program("acctswap active --from-hook", None));
        assert_eq!(on_path("acctswap", path_var), Some(exe.clone()));
        let quoted = format!("\"{}\" active --from-hook", exe.display());
        assert!(finds_program(&quoted, None), "a quoted path needs no PATH");
        std::fs::remove_file(&exe).unwrap();
        assert!(
            !finds_program(&quoted, None),
            "the path is gone, e.g. after a Node change"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
