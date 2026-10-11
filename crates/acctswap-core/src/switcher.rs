//! The commands themselves (save, switch, active, status, name), written against the ports only.
//!
//! A switch follows a check-then-commit protocol, model-checked in `spec/SwSwitch.tla`:
//! 1. Read the signed-in account (email + tokens) and confirm with the API that the tokens belong
//!    to that email. A /login half-way through fails this check, so nothing is saved under the
//!    wrong email.
//! 2. Do every network call (usage, the decision) without holding any lock.
//! 3. Take Claude Code's token-renewal lock, re-read, and commit only if nothing changed since
//!    step 1: save the tokens being left, write the target's tokens, then its account record.
//!    A renewal can therefore never fall between reading and replacing the tokens.
//!
//! `sw active` decides with [`choose`]: a usage reading only ranks accounts, and only a refused
//! request (reported by the rate-limit hook) makes sw skip an account.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::{Value, json};

use crate::decision::{Candidate, Decision, choose, limit_ends};
use crate::login::{
    OAUTH_KEY, access_expires, active_email, apply_profile, merge_oauth, refresh_days_left,
};
use crate::ports::{AccountApi, ApiCall, ConfigFile, CredentialStore, NamesStore, SwapLock};
use crate::time::until;
use crate::usage::Usage;

/// How long a saved usage reading of an inactive account is reused instead of asking again;
/// the usage endpoint rate-limits frequent callers.
pub const USAGE_TTL_SECS: i64 = 120;

/// A refusal reported this soon after sw switched away from an account belongs to that account,
/// when that account reads full or its usage is unknown: a request sent before the switch came
/// back refused after it. When the account left still has room, the refusal is the active one's.
pub const LATE_REPORT_SECS: i64 = 120;

/// What one saved copy holds: the Claude login, the account record, and what sw knows about
/// the account.
#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    /// The `claudeAiOauth` part of the credential (tokens and their expiry).
    pub oauth: Value,
    /// The `oauthAccount` record from `~/.claude.json`.
    pub profile: Value,
    /// The last usage seen for this account, if any.
    pub usage: Option<Usage>,
    /// Until when sw skips this account after a request to it was refused (Unix seconds).
    pub limited_until: Option<i64>,
    /// When sw last switched away from this account (Unix seconds).
    pub left_at: Option<i64>,
}

impl Saved {
    /// Reads a saved copy; `None` when the text is not one.
    #[must_use]
    pub fn from_json(text: &str) -> Option<Saved> {
        let v: Value = serde_json::from_str(text).ok()?;
        let oauth = v.get(OAUTH_KEY)?.clone();
        Some(Saved {
            oauth,
            profile: v["oauthAccount"].clone(),
            usage: Usage::from_saved(&v["usage"]),
            limited_until: v["limited_until"].as_i64(),
            left_at: v["left_at"].as_i64(),
        })
    }

    /// The JSON stored for this copy (the shape the Python version wrote, plus the two times).
    #[must_use]
    pub fn to_json(&self) -> String {
        json!({OAUTH_KEY: self.oauth, "oauthAccount": self.profile, "usage": self.usage.map(|u| u.to_saved()),
               "limited_until": self.limited_until, "left_at": self.left_at})
            .to_string()
    }
}

/// Why a command could not do what was asked.
#[derive(Debug, PartialEq, Eq)]
pub enum SwError {
    /// Nobody is logged in, so there is nothing to save or compare.
    NoActiveLogin,
    /// The tokens in use belong to another account than the one `~/.claude.json` names:
    /// a /login is half-way through. Retrying a moment later succeeds.
    LoginInProgress {
        /// The account `~/.claude.json` names.
        named: String,
        /// The account the tokens belong to.
        actual: String,
    },
    /// The login changed between the check and the commit (Claude Code renewed it, or another
    /// writer ran). Nothing was written; retrying succeeds.
    Changed,
    /// No other saved account (matching `hint`, when one was given).
    NoOtherAccount {
        /// The name or email part asked for.
        hint: Option<String>,
    },
    /// More than one saved account matches; holds their names.
    Ambiguous(Vec<String>),
    /// Every saved account was refused, and none has freed up yet.
    AllLimited {
        /// Name of the account that frees up first.
        next: String,
        /// When, e.g. "in 23h 14m".
        free_in: String,
    },
    /// A port failed; holds its reason.
    Port(String),
}

impl SwError {
    /// True for the errors a short wait and a retry resolve.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        matches!(self, SwError::LoginInProgress { .. } | SwError::Changed)
    }
}

impl fmt::Display for SwError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SwError::NoActiveLogin => write!(
                f,
                "no active Claude login; run claude, /login once, then: sw save"
            ),
            SwError::LoginInProgress { named, actual } => write!(
                f,
                "a /login seems half-way through (settings name {named}, tokens belong to {actual}); nothing was saved, try again"
            ),
            SwError::Changed => write!(
                f,
                "the login changed while switching; nothing was written, try again"
            ),
            SwError::NoOtherAccount { hint: Some(h) } => {
                write!(f, "no other saved account called {h:?}")
            }
            SwError::NoOtherAccount { hint: None } => {
                write!(
                    f,
                    "no other saved account; log in to it once with /login, then run: sw save"
                )
            }
            SwError::Ambiguous(names) => write!(
                f,
                "more than one match: {}; use its short name",
                names.join(", ")
            ),
            SwError::AllLimited { next, free_in } => write!(
                f,
                "every saved account has hit its limit; the first to free up is {next}, {free_in}"
            ),
            SwError::Port(e) => write!(f, "{e}"),
        }
    }
}

impl From<String> for SwError {
    fn from(e: String) -> Self {
        SwError::Port(e)
    }
}

/// Result of a command.
pub type SwResult<T> = Result<T, SwError>;

/// A usage reading and whether it is live (fetched now) or the last saved one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    /// The usage, when known.
    pub usage: Option<Usage>,
    /// True when fetched now, false when it is the last saved value.
    pub live: bool,
}

/// Result of `sw` / `sw <name>`.
#[derive(Debug)]
pub struct Switched {
    /// The account switched away from; `None` after a logout.
    pub from: Option<String>,
    /// The account now active.
    pub to: String,
    /// Set when the save before switching found an account it had not seen (a /login made outside sw).
    pub adopted: Option<String>,
}

/// Result of `sw active`.
#[derive(Debug)]
pub enum ActiveOutcome {
    /// The active account stays: it has room, its usage is unknown, or it reads full but was not
    /// refused and no other account has room.
    Stayed {
        /// The active account.
        email: String,
        /// Its usage.
        reading: Reading,
    },
    /// Another account is active now.
    Switched {
        /// The account left.
        from: String,
        /// Its usage.
        from_reading: Reading,
        /// Until when sw skips the account left, when it was refused.
        from_limited_until: Option<i64>,
        /// The account now active.
        to: String,
        /// Its usage.
        to_reading: Reading,
    },
    /// A refusal came in right after sw switched away from `left`: it belongs to `left`, which is
    /// skipped from now on, and the active account stays.
    LateReport {
        /// The active account.
        email: String,
        /// The account the refusal belongs to.
        left: String,
    },
}

/// One account in `sw status`.
#[derive(Debug)]
pub struct StatusRow {
    /// The account.
    pub email: String,
    /// True for the active account.
    pub active: bool,
    /// Its usage.
    pub reading: Reading,
    /// Days until its saved login can no longer renew itself.
    pub refresh_days_left: Option<f64>,
    /// Until when sw skips it after a refusal, while that holds.
    pub limited_until: Option<i64>,
}

/// Result of `sw status`.
#[derive(Debug)]
pub struct Status {
    /// Set when the active account was new to sw and has just been saved.
    pub adopted: Option<String>,
    /// Every saved account.
    pub rows: Vec<StatusRow>,
}

/// The signed-in account as read in step 1: who, its tokens, its account record.
#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    email: String,
    credential: Value,
    profile: Value,
}

impl Snapshot {
    fn oauth(&self) -> &Value {
        &self.credential[OAUTH_KEY]
    }
}

/// The commands, bound to one machine's ports and one moment.
pub struct Switcher<'a> {
    /// Where logins are kept.
    pub store: &'a dyn CredentialStore,
    /// `~/.claude.json`.
    pub config: &'a dyn ConfigFile,
    /// The usage and profile endpoints.
    pub api: &'a dyn AccountApi,
    /// Short names.
    pub names: &'a dyn NamesStore,
    /// Claude Code's token-renewal lock, held only while committing a switch.
    pub swap_lock: &'a dyn SwapLock,
    /// Unix seconds; fixed for the whole command so every decision sees the same time.
    pub now: i64,
}

impl Switcher<'_> {
    fn active_credential(&self) -> SwResult<Option<Value>> {
        Ok(self
            .store
            .read_active()?
            .and_then(|s| serde_json::from_str(&s).ok()))
    }

    fn saved(&self, email: &str) -> SwResult<Option<Saved>> {
        Ok(self
            .store
            .read_saved(email)?
            .as_deref()
            .and_then(Saved::from_json))
    }

    /// The signed-in account, or None after a logout.
    fn read_signed_in(&self) -> SwResult<Option<Snapshot>> {
        let cfg = self.config.load()?;
        let cred = self.active_credential()?;
        Ok(match (active_email(&cfg), cred) {
            (Some(email), Some(credential)) if credential.get(OAUTH_KEY).is_some() => {
                Some(Snapshot {
                    email,
                    credential,
                    profile: cfg["oauthAccount"].clone(),
                })
            }
            _ => None,
        })
    }

    /// Tells the API which calls are coming: the identity check and usage of the signed-in
    /// account, and usage of `others` whose saved reading is too old to reuse.
    fn prefetch(&self, others: &[String]) -> SwResult<()> {
        let snap = self.read_signed_in()?;
        let saved: Vec<Saved> = others
            .iter()
            .filter_map(|e| self.saved(e).ok().flatten())
            .collect();
        let mut calls = Vec::new();
        if let Some(token) = snap
            .as_ref()
            .and_then(|s| s.oauth()["accessToken"].as_str())
        {
            calls.extend([ApiCall::Email(token), ApiCall::Usage(token)]);
        }
        for s in &saved {
            let fresh = s
                .usage
                .is_some_and(|u| self.now.saturating_sub(u.at) < USAGE_TTL_SECS);
            if let (false, Some(token)) = (fresh, s.oauth["accessToken"].as_str()) {
                calls.push(ApiCall::Usage(token));
            }
        }
        self.api.prefetch(&calls);
        Ok(())
    }

    /// Step 1: the signed-in account, with its tokens confirmed to belong to it. When the API
    /// cannot be reached the check is skipped: sw keeps working offline.
    fn checked_signed_in(&self) -> SwResult<Option<Snapshot>> {
        let Some(snap) = self.read_signed_in()? else {
            return Ok(None);
        };
        if let Some(token) = snap.oauth()["accessToken"].as_str()
            && access_expires(snap.oauth()).is_some_and(|e| e > self.now)
            && let Some(actual) = self.api.email(token)
            && !actual.eq_ignore_ascii_case(&snap.email)
        {
            return Err(SwError::LoginInProgress {
                named: snap.email,
                actual,
            });
        }
        Ok(Some(snap))
    }

    /// Live usage, when the access token is still good for at least a minute.
    fn live_usage(&self, oauth: &Value) -> Option<Usage> {
        let expires = access_expires(oauth)?;
        if expires <= self.now.saturating_add(60) {
            return None;
        }
        let token = oauth["accessToken"].as_str()?;
        self.api
            .usage(token)
            .map(|json| Usage::from_api(&json, self.now))
    }

    /// Saves a checked snapshot under its email. The last usage and a refusal are kept unless
    /// given anew; `left_at` is set only when sw is switching away from it. Returns true when the
    /// account was new to sw.
    fn save(
        &self,
        snap: &Snapshot,
        usage: Option<Usage>,
        limited_until: Option<i64>,
        left_at: Option<i64>,
    ) -> SwResult<bool> {
        let old = self.saved(&snap.email)?;
        let saved = Saved {
            oauth: snap.oauth().clone(),
            profile: snap.profile.clone(),
            usage: usage.or_else(|| old.as_ref().and_then(|o| o.usage)),
            limited_until: limited_until.or_else(|| old.as_ref().and_then(|o| o.limited_until)),
            left_at,
        };
        self.store.write_saved(&snap.email, &saved.to_json())?;
        Ok(old.is_none())
    }

    /// Step 3: under the renewal lock, confirm nothing changed since `checked`, save the tokens
    /// being left (with its usage, a refusal, and when it was left), then write the target's
    /// tokens and account record.
    fn commit(
        &self,
        checked: Option<&Snapshot>,
        usage: Option<Usage>,
        limited_until: Option<i64>,
        target: &str,
    ) -> SwResult<bool> {
        let target_saved = self.saved(target)?.ok_or_else(|| SwError::NoOtherAccount {
            hint: Some(target.into()),
        })?;
        let _held = self.swap_lock.hold()?;
        let now = self.read_signed_in()?;
        if now.as_ref() != checked {
            return Err(SwError::Changed);
        }
        let adopted = match checked {
            Some(snap) => self.save(snap, usage, limited_until, Some(self.now))?,
            None => false,
        };
        let credential = merge_oauth(checked.map(|s| &s.credential), &target_saved.oauth);
        let previous = self.store.read_active()?;
        self.store.write_active(&credential.to_string())?;
        let profile_written = self.config.load().and_then(|mut cfg| {
            apply_profile(&mut cfg, &target_saved.profile);
            self.config.save(&cfg)
        });
        if let Err(e) = profile_written {
            // Never leave tokens of one account next to another account's email: put the
            // previous login back, then report the failure.
            if let Some(prev) = previous {
                self.store.write_active(&prev)?;
            }
            return Err(SwError::Port(e));
        }
        Ok(adopted)
    }

    /// Saves the active login under its email. A /login made outside sw lands here: it refreshes
    /// that account's saved copy, or adds the account. Returns (email, newly added).
    ///
    /// # Errors
    /// [`SwError::NoActiveLogin`] after a logout; [`SwError::LoginInProgress`] when the tokens
    /// belong to another account; [`SwError::Port`] when a port fails.
    pub fn save_current(&self) -> SwResult<(String, bool)> {
        self.prefetch(&[])?;
        let snap = self.checked_signed_in()?.ok_or(SwError::NoActiveLogin)?;
        let usage = self.live_usage(snap.oauth());
        let is_new = self.save(&snap, usage, None, None)?;
        Ok((snap.email, is_new))
    }

    /// Short name of an account, or its email's domain when it has none.
    #[must_use]
    pub fn name_of(&self, email: &str) -> String {
        let names = self.names.load().unwrap_or_default();
        names.iter().find(|(_, e)| e.as_str() == email).map_or_else(
            || email.rsplit('@').next().unwrap_or(email).to_owned(),
            |(n, _)| n.clone(),
        )
    }

    fn resolve(&self, hint: &str, candidates: &[String]) -> SwResult<Vec<String>> {
        if let Some(email) = self
            .names
            .load()?
            .get(hint)
            .filter(|e| candidates.contains(e))
        {
            return Ok(vec![email.clone()]);
        }
        let h = hint.to_lowercase();
        Ok(candidates
            .iter()
            .filter(|e| e.to_lowercase().contains(&h))
            .cloned()
            .collect())
    }

    /// `sw` / `sw <name>`: switch to the other (or the named) saved account.
    ///
    /// # Errors
    /// [`SwError::NoOtherAccount`] or [`SwError::Ambiguous`] when the target is not clear;
    /// [`SwError::LoginInProgress`] or [`SwError::Changed`] (retry); [`SwError::Port`].
    pub fn switch(&self, hint: Option<&str>) -> SwResult<Switched> {
        self.prefetch(&[])?;
        let checked = self.checked_signed_in()?;
        let current = checked.as_ref().map(|s| s.email.clone());
        let usage = checked.as_ref().and_then(|s| self.live_usage(s.oauth()));
        let mut others: Vec<String> = self
            .store
            .list_saved()?
            .into_iter()
            .filter(|e| Some(e) != current.as_ref())
            .collect();
        if let Some(h) = hint {
            others = self.resolve(h, &others)?;
        }
        let target = match others.as_slice() {
            [] => {
                return Err(SwError::NoOtherAccount {
                    hint: hint.map(str::to_owned),
                });
            }
            [one] => one.clone(),
            many => {
                return Err(SwError::Ambiguous(
                    many.iter().map(|e| self.name_of(e)).collect(),
                ));
            }
        };
        let is_new = self.commit(checked.as_ref(), usage, None, &target)?;
        Ok(Switched {
            adopted: current.clone().filter(|_| is_new),
            from: current,
            to: target,
        })
    }

    /// Usage of one account, and its saved copy. The active account is asked live; others reuse
    /// a saved reading younger than [`USAGE_TTL_SECS`].
    fn reading(&self, email: &str, current: &Snapshot) -> SwResult<(Reading, Option<Saved>)> {
        let saved = self.saved(email)?;
        let oauth = if current.email == email {
            current.oauth()
        } else {
            if let Some(u) = saved.as_ref().and_then(|s| s.usage)
                && self.now.saturating_sub(u.at) < USAGE_TTL_SECS
            {
                let reading = Reading {
                    usage: Some(u),
                    live: false,
                };
                return Ok((reading, saved));
            }
            saved.as_ref().map_or(&Value::Null, |s| &s.oauth)
        };
        let reading = match self.live_usage(oauth) {
            Some(u) => Reading {
                usage: Some(u),
                live: true,
            },
            None => Reading {
                usage: saved.as_ref().and_then(|s| s.usage),
                live: false,
            },
        };
        Ok((reading, saved))
    }

    /// `sw active`: see [`choose`] for the rule. `refused` says a request to the active account
    /// was just turned away (the rate-limit hook passes it); sw then skips that account until
    /// [`limit_ends`]. A refusal reported within [`LATE_REPORT_SECS`] of a switch belongs to the
    /// account switched away from instead, unless that account still has room.
    ///
    /// # Errors
    /// [`SwError::NoActiveLogin`], [`SwError::AllLimited`], [`SwError::LoginInProgress`] or
    /// [`SwError::Changed`] (retry), or [`SwError::Port`].
    pub fn active(&self, refused: bool) -> SwResult<ActiveOutcome> {
        self.prefetch(&self.store.list_saved()?)?;
        let snap = self.checked_signed_in()?.ok_or(SwError::NoActiveLogin)?;
        let (here, here_saved) = self.reading(&snap.email, &snap)?;
        // The other accounts; a copy that cannot be read is no account to move to.
        let mut known: Vec<(String, Reading, Saved)> = Vec::new();
        for email in self.store.list_saved()? {
            if email != snap.email
                && let (reading, Some(saved)) = self.reading(&email, &snap)?
            {
                known.push((email, reading, saved));
            }
        }
        if refused
            && let Some((left, reading, saved)) = known
                .iter()
                .filter(|(_, r, s)| {
                    s.left_at
                        .is_some_and(|t| self.now.saturating_sub(t) < LATE_REPORT_SECS)
                        && r.usage.is_none_or(|u| !u.has_room(self.now))
                })
                .max_by_key(|(_, _, s)| s.left_at)
        {
            if saved.limited_until.is_none_or(|t| t <= self.now) {
                let marked = Saved {
                    limited_until: Some(limit_ends(reading.usage, self.now)),
                    ..saved.clone()
                };
                self.store.write_saved(left, &marked.to_json())?;
            }
            return Ok(ActiveOutcome::LateReport {
                email: snap.email,
                left: left.clone(),
            });
        }
        let new_limit = refused.then(|| limit_ends(here.usage, self.now));
        let current = Candidate {
            id: &snap.email,
            usage: here.usage,
            limited_until: new_limit.or_else(|| here_saved.and_then(|s| s.limited_until)),
        };
        let candidates: Vec<Candidate<'_>> = known
            .iter()
            .map(|(id, r, s)| Candidate {
                id,
                usage: r.usage,
                limited_until: s.limited_until,
            })
            .collect();
        match choose(&current, &candidates, self.now) {
            Decision::Stay => {
                self.save(&snap, here.usage, new_limit, None)?;
                Ok(ActiveOutcome::Stayed {
                    email: snap.email,
                    reading: here,
                })
            }
            Decision::NoneLeft => {
                self.save(&snap, here.usage, new_limit, None)?;
                // Every account here was refused, so each has a time; the active one starts.
                let (next, at) = candidates.iter().fold(
                    (current.id, current.limited_until.unwrap_or(self.now)),
                    |best, c| match c.limited_until {
                        Some(t) if t < best.1 => (c.id, t),
                        _ => best,
                    },
                );
                Err(SwError::AllLimited {
                    next: self.name_of(next),
                    free_in: until(at, self.now),
                })
            }
            Decision::SwitchTo(to) => {
                // `choose` only returns ids it was given, so the account is always found.
                let to_reading = known
                    .iter()
                    .find(|(e, _, _)| e == to)
                    .map_or(here, |(_, r, _)| *r);
                let from_limited_until = current.limited_until.filter(|t| *t > self.now);
                let to = to.to_owned();
                self.commit(Some(&snap), here.usage, new_limit, &to)?;
                Ok(ActiveOutcome::Switched {
                    from: snap.email,
                    from_reading: here,
                    from_limited_until,
                    to,
                    to_reading,
                })
            }
        }
    }

    /// `sw status`: every saved account with its usage; also picks up a /login made outside sw.
    ///
    /// # Errors
    /// [`SwError::NoActiveLogin`], [`SwError::LoginInProgress`] (retry), or [`SwError::Port`].
    pub fn status(&self) -> SwResult<Status> {
        self.prefetch(&self.store.list_saved()?)?;
        let snap = self.checked_signed_in()?.ok_or(SwError::NoActiveLogin)?;
        let (here, _) = self.reading(&snap.email, &snap)?;
        let is_new = self.save(&snap, here.usage.filter(|_| here.live), None, None)?;
        // `save` above wrote the active account, so the list always contains it.
        let emails = self.store.list_saved()?;
        let mut rows = Vec::with_capacity(emails.len());
        for email in emails {
            let active = email == snap.email;
            let (reading, saved) = if active {
                (here, self.saved(&email)?)
            } else {
                self.reading(&email, &snap)?
            };
            let refresh = if active {
                refresh_days_left(snap.oauth(), self.now)
            } else {
                saved
                    .as_ref()
                    .and_then(|s| refresh_days_left(&s.oauth, self.now))
            };
            rows.push(StatusRow {
                active,
                refresh_days_left: refresh,
                limited_until: saved
                    .and_then(|s| s.limited_until)
                    .filter(|t| *t > self.now),
                reading,
                email,
            });
        }
        Ok(Status {
            adopted: is_new.then_some(snap.email),
            rows,
        })
    }

    /// `sw name <name>`: give the active account a short name.
    ///
    /// # Errors
    /// [`SwError::NoActiveLogin`], or [`SwError::Port`] when a port fails.
    pub fn name(&self, new: &str) -> SwResult<String> {
        let snap = self.read_signed_in()?.ok_or(SwError::NoActiveLogin)?;
        let mut names: BTreeMap<String, String> = self.names.load()?;
        // A map holds one email per name, so inserting below also takes the name from anyone else.
        names.retain(|_, e| *e != snap.email);
        names.insert(new.to_owned(), snap.email.clone());
        self.names.save(&names)?;
        Ok(snap.email)
    }
}
