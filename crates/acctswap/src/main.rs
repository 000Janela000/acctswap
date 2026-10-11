//! acctswap: switch the Claude account every Claude Code session on this machine uses,
//! without a browser login. Sessions share one saved login and re-read it, so they follow.

#![forbid(unsafe_code)]

mod adapters;
mod hook;

use std::fmt::Write as _;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use acctswap_core::ports::CredentialStore;
use acctswap_core::time::until;
use acctswap_core::{ActiveOutcome, Reading, SwError, Switcher};
use adapters::{
    ClaudeJson, CurlApi, FileStore, KeychainStore, NAME, NamesFile, Paths, RenewalLock,
};

const HELP: &str = "\
acctswap (short: sw) — switch the Claude account every Claude Code session on this machine uses

  sw                  switch to the other saved account (when there are two)
  sw <name>           switch to the account with that short name, or part of its email
  sw active           when the active account reads full, switch to the one with the most room
  sw status           every saved account with its usage; saves a login sw has not seen
  sw save             save the active login (after a /login of an account sw has not seen)
  sw name <name>      give the active account a short name, e.g. `sw name me`
  sw hook install     switch automatically when Claude refuses a request on a usage limit
  sw hook uninstall   remove that hook
  sw --version

Open sessions follow a switch on their next message. Remote Control drops on a switch:
run /remote-control in tabs that talk to other machines. Inside a session: `! sw`.

Usage readings are rounded, and an account that reads 100% can still answer. So sw treats an
account as used up only after Claude refused a request to it (the hook reports that), and then
skips it until its full window resets.";

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn describe(r: &Reading, now: i64) -> String {
    match &r.usage {
        Some(u) if r.live => u.describe(now),
        Some(u) => {
            let secs = now.saturating_sub(u.at);
            let when = if secs < 60 {
                "moments ago".to_owned()
            } else {
                let age = until(now.saturating_add(secs), now);
                format!("{} ago", age.trim_start_matches("in "))
            };
            format!("{} (last seen {when})", u.describe(now))
        }
        None => {
            "usage unknown (its saved token expired; it refreshes when the account is active again)"
                .into()
        }
    }
}

/// A warning when the installed hook can no longer find its program, e.g. after a Node or
/// Python change moved an npm or pip install.
fn broken_hook(paths: &Paths) -> Option<String> {
    let command = hook::installed(&paths.claude_dir.join("settings.json"))?;
    let path_var = std::env::var_os("PATH");
    (!hook::finds_program(&command, path_var.as_deref()))
        .then(|| format!("! the hook can't find its program ({command}); run: sw hook install"))
}

fn hook_cmd(paths: &Paths, action: &str) -> Result<String, SwError> {
    let settings = paths.claude_dir.join("settings.json");
    match action {
        "install" => {
            // By name when it is on PATH, so the hook survives npm, pip or Homebrew moving it.
            let program = if hook::on_path(NAME, std::env::var_os("PATH").as_deref()).is_some() {
                NAME.to_owned()
            } else {
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                format!("\"{}\"", exe.display())
            };
            hook::install(&settings, &program)?;
            Ok(format!(
                "hook installed in {}: it runs {program} active --from-hook.\n\
                 New sessions use it; open ones after /hooks or a restart.",
                settings.display()
            ))
        }
        "uninstall" if hook::uninstall(&settings)? => Ok("hook removed".into()),
        "uninstall" => Ok("no sw hook was installed".into()),
        "status" => Ok(match (hook::installed(&settings), broken_hook(paths)) {
            (None, _) => "hook not installed".into(),
            (Some(_), Some(warning)) => warning,
            (Some(command), None) => format!("hook installed: {command}"),
        }),
        other => Err(SwError::Port(format!(
            "unknown hook action {other:?}; use install, uninstall or status"
        ))),
    }
}

fn adopted_note(sw: &Switcher<'_>, email: Option<&String>) -> String {
    email.map_or_else(String::new, |e| {
        format!(
            "new account added: {}. Give it a short name: sw name <name>\n",
            sw.name_of(e)
        )
    })
}

fn status_cmd(sw: &Switcher<'_>, paths: &Paths) -> Result<String, SwError> {
    let s = sw.status()?;
    let mut out = adopted_note(sw, s.adopted.as_ref());
    for row in &s.rows {
        let warn = match row.refresh_days_left {
            Some(d) if d < 3.0 => {
                format!("  ! login expires in {d:.0} days unless used: switch to it once")
            }
            _ => String::new(),
        };
        let limit = row.limited_until.map_or_else(String::new, |t| {
            format!("  refused: skipped until it frees up {}", until(t, sw.now))
        });
        let mark = if row.active { "*" } else { " " };
        let name = sw.name_of(&row.email);
        let _ = writeln!(
            out,
            "{mark} {name:<8} {}{limit}{warn}",
            describe(&row.reading, sw.now)
        );
    }
    if let Some(warning) = broken_hook(paths) {
        let _ = writeln!(out, "{warning}");
    }
    Ok(out + "* = active")
}

fn active_cmd(sw: &Switcher<'_>, paths: &Paths, from_hook: bool) -> Result<String, SwError> {
    let result = sw.active(from_hook).map(|outcome| match outcome {
        ActiveOutcome::Stayed { email, reading } => {
            let reads_full = reading.usage.is_some_and(|u| !u.has_room(sw.now));
            let note = if reads_full {
                "\nIt reads full, but no request to it has been refused, and no other account \
                 has room. To move anyway: sw <name>"
            } else {
                ""
            };
            format!(
                "staying on {}: {}{note}",
                sw.name_of(&email),
                describe(&reading, sw.now)
            )
        }
        ActiveOutcome::Switched {
            from,
            from_reading,
            from_limited_until,
            to,
            to_reading,
        } => {
            let why = from_limited_until.map_or_else(
                || "reads full".to_owned(),
                |t| format!("hit its limit; sw skips it until it frees up {}", until(t, sw.now)),
            );
            format!(
                "{} {why} ({}).\nswitched to {}: {}\n\
                 Open sessions follow on their next message. Tabs that talk to other machines: /remote-control.",
                sw.name_of(&from),
                describe(&from_reading, sw.now),
                sw.name_of(&to),
                describe(&to_reading, sw.now)
            )
        }
        ActiveOutcome::LateReport { email, left } => format!(
            "this refusal came in right after sw left {left}, so it belongs to {left}; staying on {}",
            sw.name_of(&email),
            left = sw.name_of(&left)
        ),
    });
    if from_hook {
        let line = result
            .as_ref()
            .map_or_else(|e| format!("sw: {e}"), String::clone);
        hook::log(&paths.claude_dir, sw.now, &line);
    }
    result
}

fn switch_cmd(sw: &Switcher<'_>, hint: Option<&str>) -> Result<String, SwError> {
    let s = sw.switch(hint)?;
    let from = s
        .from
        .as_deref()
        .map_or_else(|| "(logged out)".into(), |e| sw.name_of(e));
    Ok(format!(
        "{}switched: {from} -> {}. Open sessions follow on their next message.\n\
         Tabs that talk to other machines: run /remote-control there.",
        adopted_note(sw, s.adopted.as_ref()),
        sw.name_of(&s.to)
    ))
}

fn run(args: &[String]) -> Result<String, SwError> {
    let paths = Paths::detect()?;
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    if let ["hook", action] = args.as_slice() {
        return hook_cmd(&paths, action);
    }

    if paths.login_file_in_use() {
        return Err(SwError::Port(format!(
            "Claude Code saved this login to {} instead of the Keychain (it does that when the \
             Keychain refuses a write, for example over SSH). sw switches the Keychain, so it stops \
             here. Make the Keychain writable (`claude doctor` shows how), then run /logout and \
             /login in Claude Code.",
            paths.claude_dir.join(".credentials.json").display()
        )));
    }
    let store: Box<dyn CredentialStore> = match &paths.keychain {
        Some(service) => Box::new(KeychainStore::new(&paths, service)),
        None => Box::new(FileStore::new(&paths)),
    };
    let config = ClaudeJson(paths.claude_json.clone());
    let names = NamesFile(paths.sw_dir.join("names.json"));
    let api = CurlApi::default();
    let renewal = RenewalLock::new(&paths);
    let _lock = adapters::lock(&paths.sw_dir)?;

    // A /login half-way through, or a renewal between check and commit, clears up in moments.
    let mut attempt = 0;
    loop {
        let sw = Switcher {
            store: store.as_ref(),
            config: &config,
            api: &api,
            names: &names,
            swap_lock: &renewal,
            now: now(),
        };
        match command(&sw, &paths, &args) {
            Err(e) if e.is_transient() && attempt < 4 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
            other => return other,
        }
    }
}

fn command(sw: &Switcher<'_>, paths: &Paths, args: &[&str]) -> Result<String, SwError> {
    match args {
        ["status"] => status_cmd(sw, paths),
        ["save"] => {
            let (email, is_new) = sw.save_current()?;
            let note = if is_new {
                " (new account; name it with: sw name <name>)"
            } else {
                ""
            };
            Ok(format!("saved: {}{note}", sw.name_of(&email)))
        }
        ["name", new] => {
            let email = sw.name(new)?;
            let domain = email.rsplit('@').next().unwrap_or_default();
            Ok(format!(
                "the active account ({domain}) is now called {new:?}"
            ))
        }
        ["active"] => active_cmd(sw, paths, false),
        ["active", "--from-hook"] => active_cmd(sw, paths, true),
        [] | [_] => switch_cmd(sw, args.first().copied()),
        _ => Err(SwError::Port(format!("unknown command\n\n{HELP}"))),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("help" | "-h" | "--help") => {
            println!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Some("--version" | "-V") => {
            println!("{NAME} {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    match run(&args) {
        Ok(out) => {
            println!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("sw: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acctswap_core::usage::Usage;

    #[test]
    fn a_saved_reading_says_how_old_it_is() {
        let saved = |age: i64| Reading {
            usage: Some(Usage {
                at: 1_000_000 - age,
                ..Usage::default()
            }),
            live: false,
        };
        assert!(describe(&saved(0), 1_000_000).ends_with("(last seen moments ago)"));
        assert!(describe(&saved(59), 1_000_000).ends_with("(last seen moments ago)"));
        assert!(describe(&saved(3_600), 1_000_000).ends_with("(last seen 1h 00m ago)"));
    }
}
