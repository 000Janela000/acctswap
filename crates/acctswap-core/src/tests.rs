//! Scenario tests for the commands, run against in-memory fakes of every port.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use serde_json::{Value, json};

use crate::ports::{AccountApi, ConfigFile, CredentialStore, NamesStore, PortResult, SwapLock};
use crate::{ActiveOutcome, SwError, Switcher};

const NOW: i64 = 1_791_550_000; // 2026-10-09 ~12:46 UTC
const LATER_MS: i64 = (NOW + 7 * 3600) * 1000; // access token still valid
const EXPIRED_MS: i64 = (NOW - 60) * 1000;

/// A writer that slips in just before the swap lock is taken.
type OnLock = Box<dyn Fn(&Fake)>;

#[derive(Default)]
struct Fake {
    active: RefCell<Option<String>>,
    saved: RefCell<BTreeMap<String, String>>,
    config: RefCell<Value>,
    names: RefCell<BTreeMap<String, String>>,
    usage_by_token: RefCell<BTreeMap<String, Value>>,
    owner_by_token: RefCell<BTreeMap<String, String>>,
    usage_calls: Cell<u32>,
    prefetched: RefCell<Vec<String>>,
    lock_held: Rc<Cell<bool>>,
    writes_under_lock: Cell<u32>,
    writes_without_lock: Cell<u32>,
    /// Runs when the swap lock is taken: simulates a writer that slipped in before it.
    on_lock: RefCell<Option<OnLock>>,
    /// Port methods that fail, by name, for the failure-injection test.
    failing: RefCell<Vec<&'static str>>,
    /// Fails one method on its n-th call only (1-based), to reach later error paths.
    fail_nth: Cell<Option<(&'static str, u32)>>,
    calls: RefCell<BTreeMap<&'static str, u32>>,
}

impl Fake {
    fn check(&self, method: &'static str) -> PortResult<()> {
        let n = {
            let mut calls = self.calls.borrow_mut();
            let n = calls.entry(method).or_insert(0);
            *n += 1;
            *n
        };
        if self.failing.borrow().contains(&method) || self.fail_nth.get() == Some((method, n)) {
            return Err(format!("{method} failed"));
        }
        Ok(())
    }

    fn note_write(&self) {
        if self.lock_held.get() {
            self.writes_under_lock.set(self.writes_under_lock.get() + 1);
        } else {
            self.writes_without_lock
                .set(self.writes_without_lock.get() + 1);
        }
    }
}

impl CredentialStore for Fake {
    fn read_active(&self) -> PortResult<Option<String>> {
        self.check("read_active")?;
        Ok(self.active.borrow().clone())
    }
    fn write_active(&self, v: &str) -> PortResult<()> {
        self.check("write_active")?;
        self.note_write();
        *self.active.borrow_mut() = Some(v.to_owned());
        Ok(())
    }
    fn read_saved(&self, e: &str) -> PortResult<Option<String>> {
        self.check("read_saved")?;
        Ok(self.saved.borrow().get(e).cloned())
    }
    fn write_saved(&self, e: &str, v: &str) -> PortResult<()> {
        self.check("write_saved")?;
        self.saved.borrow_mut().insert(e.to_owned(), v.to_owned());
        Ok(())
    }
    fn list_saved(&self) -> PortResult<Vec<String>> {
        self.check("list_saved")?;
        Ok(self.saved.borrow().keys().cloned().collect())
    }
}

impl ConfigFile for Fake {
    fn load(&self) -> PortResult<Value> {
        self.check("config_load")?;
        Ok(self.config.borrow().clone())
    }
    fn save(&self, v: &Value) -> PortResult<()> {
        self.check("config_save")?;
        self.note_write();
        *self.config.borrow_mut() = v.clone();
        Ok(())
    }
}

impl AccountApi for Fake {
    fn usage(&self, token: &str) -> Option<Value> {
        self.usage_calls.set(self.usage_calls.get() + 1);
        self.usage_by_token.borrow().get(token).cloned()
    }
    fn email(&self, token: &str) -> Option<String> {
        self.owner_by_token.borrow().get(token).cloned()
    }
    fn prefetch(&self, calls: &[crate::ports::ApiCall<'_>]) {
        self.prefetched
            .borrow_mut()
            .extend(calls.iter().map(|c| format!("{c:?}")));
    }
}

/// Releases the fake swap lock when dropped.
struct Guard(Rc<Cell<bool>>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl SwapLock for Fake {
    fn hold(&self) -> PortResult<Box<dyn std::any::Any>> {
        self.check("hold")?;
        if let Some(f) = self.on_lock.borrow().as_ref() {
            f(self);
        }
        assert!(!self.lock_held.get(), "the swap lock is never taken twice");
        self.lock_held.set(true);
        Ok(Box::new(Guard(Rc::clone(&self.lock_held))))
    }
}

impl NamesStore for Fake {
    fn load(&self) -> PortResult<BTreeMap<String, String>> {
        self.check("names_load")?;
        Ok(self.names.borrow().clone())
    }
    fn save(&self, n: &BTreeMap<String, String>) -> PortResult<()> {
        self.check("names_save")?;
        *self.names.borrow_mut() = n.clone();
        Ok(())
    }
}

fn oauth(token: &str, expires_ms: i64) -> Value {
    json!({"accessToken": token, "refreshToken": format!("r-{token}"), "expiresAt": expires_ms,
           "refreshTokenExpiresAt": (NOW + 27 * 86_400) * 1000})
}

/// The weekly reset `api` reports.
const WEEK_RESET: i64 = 1_791_691_199; // 2026-10-11T03:59:59Z

fn api(five: f64, week: f64) -> Value {
    json!({"five_hour": {"utilization": five, "resets_at": "2026-10-09T15:30:00+00:00"},
           "seven_day": {"utilization": week, "resets_at": "2026-10-11T03:59:59+00:00"}})
}

/// Like `api`, with the week resetting a day earlier (2026-10-10T15:00:00Z).
const EARLY_WEEK_RESET: i64 = 1_791_644_400;
fn api_early_week(five: f64, week: f64) -> Value {
    json!({"five_hour": {"utilization": five, "resets_at": "2026-10-09T15:30:00+00:00"},
           "seven_day": {"utilization": week, "resets_at": "2026-10-10T15:00:00+00:00"}})
}

impl Fake {
    /// Signed in as `email` with `token`; MCP logins sit next to the Claude login.
    fn signed_in(email: &str, token: &str) -> Fake {
        let f = Fake::default();
        *f.active.borrow_mut() = Some(
            json!({"claudeAiOauth": oauth(token, LATER_MS), "mcpOAuth": {"figma": {"t": 1}}})
                .to_string(),
        );
        *f.config.borrow_mut() = json!({"oauthAccount": {"emailAddress": email}, "cachedUsageUtilization": {"x": 1}, "projects": {}});
        f.owner_by_token
            .borrow_mut()
            .insert(token.to_owned(), email.to_owned());
        f
    }

    fn add_saved(&self, email: &str, token: &str, expires_ms: i64, usage: Option<Value>) {
        let mut v = json!({"claudeAiOauth": oauth(token, expires_ms), "oauthAccount": {"emailAddress": email}});
        if let Some(u) = usage {
            v["usage"] = u;
        }
        self.saved
            .borrow_mut()
            .insert(email.to_owned(), v.to_string());
        self.owner_by_token
            .borrow_mut()
            .insert(token.to_owned(), email.to_owned());
    }

    fn usage(&self, token: &str, value: Value) {
        self.usage_by_token
            .borrow_mut()
            .insert(token.to_owned(), value);
    }

    fn sw(&self) -> Switcher<'_> {
        self.sw_at(NOW)
    }

    fn sw_at(&self, now: i64) -> Switcher<'_> {
        Switcher {
            store: self,
            config: self,
            api: self,
            names: self,
            swap_lock: self,
            now,
        }
    }

    /// The saved copy of `email`, parsed.
    fn saved_copy(&self, email: &str) -> crate::Saved {
        crate::Saved::from_json(&self.saved.borrow()[email]).unwrap()
    }

    /// Records that sw skips `email` until `until`.
    fn mark(&self, email: &str, until: i64) {
        let mut v: Value = serde_json::from_str(&self.saved.borrow()[email]).unwrap();
        v["limited_until"] = json!(until);
        self.saved
            .borrow_mut()
            .insert(email.to_owned(), v.to_string());
    }

    fn active_token(&self) -> String {
        let v: Value = serde_json::from_str(self.active.borrow().as_ref().unwrap()).unwrap();
        v["claudeAiOauth"]["accessToken"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn active_mcp(&self) -> Value {
        let v: Value = serde_json::from_str(self.active.borrow().as_ref().unwrap()).unwrap();
        v["mcpOAuth"].clone()
    }

    fn signed_in_email(&self) -> String {
        self.config.borrow()["oauthAccount"]["emailAddress"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn saved_token(&self, email: &str) -> String {
        let v: Value = serde_json::from_str(&self.saved.borrow()[email]).unwrap();
        v["claudeAiOauth"]["accessToken"]
            .as_str()
            .unwrap()
            .to_owned()
    }
}

#[test]
fn a_login_made_outside_sw_is_adopted_once() {
    let f = Fake::signed_in("me@home.example", "t-me");
    assert_eq!(
        f.sw().save_current().unwrap(),
        ("me@home.example".into(), true)
    );
    assert_eq!(
        f.sw().save_current().unwrap(),
        ("me@home.example".into(), false)
    );
}

#[test]
fn switch_goes_to_the_other_account_and_back() {
    let f = Fake::signed_in("me@home.example", "t-me");
    f.add_saved("me@corp.example", "t-work", LATER_MS, None);

    let s = f.sw().switch(None).unwrap();
    assert_eq!(
        (s.from.as_deref(), s.to.as_str()),
        (Some("me@home.example"), "me@corp.example")
    );
    assert_eq!(
        s.adopted.as_deref(),
        Some("me@home.example"),
        "first sight of home"
    );
    assert_eq!(f.active_token(), "t-work");
    assert_eq!(f.signed_in_email(), "me@corp.example");
    assert_eq!(
        f.active_mcp(),
        json!({"figma": {"t": 1}}),
        "MCP logins survive the switch"
    );
    assert!(
        f.config.borrow().get("cachedUsageUtilization").is_none(),
        "account caches dropped"
    );

    let back = f.sw().switch(None).unwrap();
    assert_eq!(back.adopted, None);
    assert_eq!(
        f.active_token(),
        "t-me",
        "the saved home copy is the one we left"
    );
}

#[test]
fn every_write_to_the_active_login_happens_under_the_swap_lock() {
    let f = Fake::signed_in("me@home.example", "t-me");
    f.add_saved("me@corp.example", "t-work", LATER_MS, None);
    f.sw().switch(None).unwrap();
    assert_eq!(
        f.writes_under_lock.get(),
        2,
        "the credential and ~/.claude.json"
    );
    assert_eq!(f.writes_without_lock.get(), 0);
    assert!(!f.lock_held.get(), "released after the commit");
}

#[test]
fn switching_saves_the_newest_tokens_of_the_account_we_leave() {
    let f = Fake::signed_in("me@home.example", "t-me-v1");
    f.add_saved("me@corp.example", "t-work", LATER_MS, None);
    f.sw().switch(None).unwrap();
    // Claude Code renews corp's tokens while it is active.
    *f.active.borrow_mut() =
        Some(json!({"claudeAiOauth": oauth("t-work-v2", LATER_MS)}).to_string());
    f.owner_by_token
        .borrow_mut()
        .insert("t-work-v2".into(), "me@corp.example".into());
    f.sw().switch(None).unwrap();
    assert_eq!(f.saved_token("me@corp.example"), "t-work-v2");
}

#[test]
fn a_half_done_login_is_never_saved_under_the_wrong_email() {
    // /login wrote the new tokens but not yet the new email.
    let f = Fake::signed_in("me@home.example", "t-me");
    f.add_saved("me@home.example", "t-me-old", LATER_MS, None);
    f.add_saved("me@corp.example", "t-work", LATER_MS, None);
    *f.active.borrow_mut() = Some(json!({"claudeAiOauth": oauth("t-new", LATER_MS)}).to_string());
    f.owner_by_token
        .borrow_mut()
        .insert("t-new".into(), "x@proton.me".into());

    let err = f.sw().switch(None).unwrap_err();
    assert_eq!(
        err,
        SwError::LoginInProgress {
            named: "me@home.example".into(),
            actual: "x@proton.me".into()
        }
    );
    assert!(err.is_transient());
    assert_eq!(
        f.saved_token("me@home.example"),
        "t-me-old",
        "home's saved copy untouched"
    );
    assert_eq!(f.active_token(), "t-new", "nothing switched");
    assert!(matches!(
        f.sw().save_current(),
        Err(SwError::LoginInProgress { .. })
    ));
    assert!(matches!(
        f.sw().active(false),
        Err(SwError::LoginInProgress { .. })
    ));
}

#[test]
fn offline_the_identity_check_is_skipped() {
    let f = Fake::signed_in("me@home.example", "t-me");
    f.owner_by_token.borrow_mut().clear(); // the profile endpoint cannot be reached
    f.add_saved("me@corp.example", "t-work", LATER_MS, None);
    assert_eq!(f.sw().switch(None).unwrap().to, "me@corp.example");
}

#[test]
fn a_change_between_check_and_commit_aborts_without_writing() {
    let f = Fake::signed_in("me@home.example", "t-me");
    f.add_saved("me@corp.example", "t-work", LATER_MS, None);
    // Claude Code renews home's tokens just before sw takes the lock.
    *f.on_lock.borrow_mut() = Some(Box::new(|f: &Fake| {
        *f.active.borrow_mut() =
            Some(json!({"claudeAiOauth": oauth("t-me-v2", LATER_MS)}).to_string());
    }));
    let err = f.sw().switch(None).unwrap_err();
    assert_eq!(err, SwError::Changed);
    assert!(err.is_transient());
    assert_eq!(
        f.active_token(),
        "t-me-v2",
        "the renewed tokens stay active"
    );
    assert!(
        !f.saved.borrow().contains_key("me@home.example"),
        "nothing saved from the stale read"
    );
    assert_eq!(f.writes_under_lock.get() + f.writes_without_lock.get(), 0);
}

#[test]
fn three_accounts_need_a_name() {
    let f = Fake::signed_in("me@home.example", "t-me");
    f.add_saved("me@corp.example", "t-work", LATER_MS, None);
    f.add_saved("third@proton.me", "t-3", LATER_MS, None);
    f.names
        .borrow_mut()
        .insert("corp".into(), "me@corp.example".into());

    assert!(matches!(f.sw().switch(None), Err(SwError::Ambiguous(_))));
    assert_eq!(f.sw().switch(Some("corp")).unwrap().to, "me@corp.example");
    assert_eq!(
        f.sw().switch(Some("proton")).unwrap().to,
        "third@proton.me",
        "part of the email works too"
    );
    assert_eq!(
        f.sw().switch(Some("nobody")).unwrap_err(),
        SwError::NoOtherAccount {
            hint: Some("nobody".into())
        }
    );
}

#[test]
fn switch_after_a_logout_loads_the_saved_login() {
    let f = Fake::default();
    *f.config.borrow_mut() = json!({});
    f.add_saved("me@home.example", "t-me", LATER_MS, None);
    let s = f.sw().switch(Some("home")).unwrap();
    assert_eq!(s.from, None);
    assert_eq!(f.active_token(), "t-me");
    assert_eq!(f.signed_in_email(), "me@home.example");
}

#[test]
fn switch_with_nobody_else_saved() {
    let f = Fake::signed_in("me@home.example", "t-me");
    assert_eq!(
        f.sw().switch(None).unwrap_err(),
        SwError::NoOtherAccount { hint: None }
    );
}

#[test]
fn active_stays_while_there_is_room() {
    let f = Fake::signed_in("me@home.example", "t-me");
    f.usage("t-me", api(36.0, 96.0));
    f.add_saved("me@corp.example", "t-work", LATER_MS, None);
    assert!(matches!(
        f.sw().active(false).unwrap(),
        ActiveOutcome::Stayed { .. }
    ));
    assert_eq!(f.active_token(), "t-me");
    assert!(
        f.saved.borrow().contains_key("me@home.example"),
        "staying still saves the newest tokens"
    );
}

#[test]
fn active_moves_to_the_account_with_room() {
    let f = Fake::signed_in("me@home.example", "t-me");
    f.usage("t-me", api(100.0, 80.0));
    f.add_saved("me@corp.example", "t-work", LATER_MS, None);
    f.usage("t-work", api(5.0, 40.0));
    match f.sw().active(false).unwrap() {
        ActiveOutcome::Switched {
            to,
            from_limited_until,
            ..
        } => {
            assert_eq!(to, "me@corp.example");
            assert_eq!(from_limited_until, None, "it read full; it was not refused");
        }
        other => panic!("expected a switch, got {other:?}"),
    }
    assert_eq!(f.active_token(), "t-work");
}

#[test]
fn active_prefers_the_most_weekly_room() {
    let f = Fake::signed_in("me@home.example", "t-me");
    f.usage("t-me", api(0.0, 100.0));
    f.add_saved("a@x.com", "t-a", LATER_MS, None);
    f.usage("t-a", api(0.0, 70.0));
    f.add_saved("b@y.com", "t-b", LATER_MS, None);
    f.usage("t-b", api(0.0, 20.0));
    assert!(
        matches!(f.sw().active(false).unwrap(), ActiveOutcome::Switched { to, .. } if to == "b@y.com")
    );
}

#[test]
fn active_uses_the_last_saved_usage_when_a_token_has_expired() {
    let f = Fake::signed_in("me@home.example", "t-me");
    f.usage("t-me", api(0.0, 100.0));
    // corp's saved access token expired: no live call; its last saved usage says full.
    f.add_saved(
        "me@corp.example",
        "t-work",
        EXPIRED_MS,
        Some(
            json!({"at": 0, "five_hour": {"pct": 0.0, "resets_at": null},
                    "seven_day": {"pct": 100.0, "resets_at": "2026-10-11T03:59:59+00:00"}}),
        ),
    );
    f.usage("t-work", api(0.0, 0.0)); // would say empty, but must not be called
    assert!(matches!(
        f.sw().active(false).unwrap(),
        ActiveOutcome::Stayed { .. }
    ));
    assert_eq!(f.active_token(), "t-me", "nothing switched");
}

#[test]
fn active_tries_an_account_whose_usage_is_unknown() {
    let f = Fake::signed_in("me@home.example", "t-me");
    f.usage("t-me", api(100.0, 50.0));
    f.add_saved("me@corp.example", "t-work", EXPIRED_MS, None);
    assert!(matches!(
        f.sw().active(false).unwrap(),
        ActiveOutcome::Switched { .. }
    ));
}

#[test]
fn a_recent_saved_reading_of_another_account_is_reused() {
    let f = Fake::signed_in("me@home.example", "t-me");
    f.usage("t-me", api(100.0, 50.0));
    let fresh = json!({"at": NOW - 30, "five_hour": {"pct": 1.0, "resets_at": null},
                       "seven_day": {"pct": 10.0, "resets_at": null}});
    f.add_saved("me@corp.example", "t-work", LATER_MS, Some(fresh));
    f.usage("t-work", api(0.0, 0.0));
    let _ = f.sw().active(false).unwrap();
    assert_eq!(f.usage_calls.get(), 1, "only the active account was asked");
}

#[test]
fn status_lists_every_account_and_marks_the_active_one() {
    let f = Fake::signed_in("me@home.example", "t-me");
    f.usage("t-me", api(36.0, 96.0));
    f.add_saved("me@corp.example", "t-work", LATER_MS, None);
    f.usage("t-work", api(0.0, 100.0));
    let s = f.sw().status().unwrap();
    assert_eq!(s.adopted.as_deref(), Some("me@home.example"));
    assert_eq!(s.rows.len(), 2);
    let me = s.rows.iter().find(|r| r.active).unwrap();
    assert_eq!(me.email, "me@home.example");
    assert!(me.reading.live);
    assert_eq!(me.refresh_days_left.map(f64::round), Some(27.0));
    let work = s.rows.iter().find(|r| !r.active).unwrap();
    assert_eq!(work.refresh_days_left.map(f64::round), Some(27.0));
    assert_eq!(f.usage_calls.get(), 2, "one call per account");
}

#[test]
fn names_point_at_one_account_each() {
    let f = Fake::signed_in("me@home.example", "t-me");
    f.sw().name("me").unwrap();
    f.sw().name("personal").unwrap();
    let names = f.names.borrow().clone();
    assert_eq!(names.len(), 1, "renaming replaces the old name");
    assert_eq!(names["personal"], "me@home.example");
    assert_eq!(f.sw().name_of("me@home.example"), "personal");
    assert_eq!(
        f.sw().name_of("x@corp.example"),
        "corp.example",
        "unnamed accounts show their domain"
    );
}

#[test]
fn nothing_to_save_when_logged_out() {
    let f = Fake::default();
    *f.config.borrow_mut() = json!({});
    assert_eq!(f.sw().save_current().unwrap_err(), SwError::NoActiveLogin);
    assert_eq!(f.sw().status().unwrap_err(), SwError::NoActiveLogin);
    assert_eq!(f.sw().name("x").unwrap_err(), SwError::NoActiveLogin);
}

#[test]
fn errors_explain_themselves() {
    let all = [
        SwError::NoActiveLogin,
        SwError::LoginInProgress {
            named: "a".into(),
            actual: "b".into(),
        },
        SwError::Changed,
        SwError::NoOtherAccount {
            hint: Some("x".into()),
        },
        SwError::NoOtherAccount { hint: None },
        SwError::Ambiguous(vec!["a".into(), "b".into()]),
        SwError::AllLimited {
            next: "me".into(),
            free_in: "in 3h 05m".into(),
        },
        SwError::Port("disk full".into()),
    ];
    for e in all {
        assert_ne!(e.to_string(), "");
    }
    assert_eq!(SwError::from("x".to_owned()), SwError::Port("x".into()));
}

/// Every port failure ends in an error, never a panic, and never leaves one account's tokens
/// next to another account's email.
#[test]
fn every_port_failure_is_an_error_and_never_half_switches() {
    const METHODS: [&str; 10] = [
        "read_active",
        "write_active",
        "read_saved",
        "write_saved",
        "list_saved",
        "config_load",
        "config_save",
        "names_load",
        "names_save",
        "hold",
    ];
    type Command = fn(&Switcher<'_>) -> bool;
    let commands: [(&str, Command); 7] = [
        ("switch", |sw| sw.switch(None).is_err()),
        ("switch named", |sw| sw.switch(Some("corp")).is_err()),
        ("active", |sw| sw.active(false).is_err()),
        ("active refused", |sw| sw.active(true).is_err()),
        ("status", |sw| sw.status().is_err()),
        ("save", |sw| sw.save_current().is_err()),
        ("name", |sw| sw.name("x").is_err()),
    ];
    // `active` switches, stays, finds every account refused, or takes a late report.
    // (here %, there %, corp refused, corp just left)
    let setups: [(f64, f64, bool, bool); 5] = [
        (100.0, 50.0, false, false),
        (10.0, 10.0, false, false),
        (100.0, 100.0, false, false),
        (100.0, 100.0, true, false),
        (100.0, 100.0, false, true),
    ];
    let cases = METHODS
        .iter()
        .flat_map(|m| setups.iter().map(move |s| (*m, *s)));
    for (method, (here, there, refused, just_left)) in cases {
        for (label, run) in commands {
            let f = Fake::signed_in("me@home.example", "t-me");
            f.usage("t-me", api(here, 50.0));
            f.add_saved("me@corp.example", "t-work", LATER_MS, None);
            f.usage("t-work", api(there, there));
            if refused {
                f.mark("me@corp.example", NOW + 60);
            }
            if just_left {
                let mut v: Value =
                    serde_json::from_str(&f.saved.borrow()["me@corp.example"]).unwrap();
                v["left_at"] = json!(NOW - 5);
                f.saved
                    .borrow_mut()
                    .insert("me@corp.example".into(), v.to_string());
            }
            f.names
                .borrow_mut()
                .insert("corp".into(), "me@corp.example".into());
            f.failing.borrow_mut().push(method);
            let _ = run(&f.sw());
            f.failing.borrow_mut().clear();
            let owner = &f.owner_by_token.borrow()[&f.active_token()];
            assert_eq!(
                *owner,
                f.signed_in_email(),
                "{label} with {method} failing left a half switch"
            );
            assert!(
                !f.lock_held.get(),
                "{label} with {method} failing kept the lock"
            );
        }
    }
}

#[test]
fn a_failed_profile_write_puts_the_previous_login_back() {
    let f = Fake::signed_in("me@home.example", "t-me");
    f.add_saved("me@corp.example", "t-work", LATER_MS, None);
    f.failing.borrow_mut().push("config_save");
    assert_eq!(
        f.sw().switch(None).unwrap_err(),
        SwError::Port("config_save failed".into())
    );
    assert_eq!(f.active_token(), "t-me");
    assert_eq!(f.signed_in_email(), "me@home.example");
}

#[test]
fn a_target_that_vanished_is_reported() {
    let f = Fake::signed_in("me@home.example", "t-me");
    f.saved
        .borrow_mut()
        .insert("gone@x.com".into(), "not a saved copy".into());
    let err = f.sw().switch(Some("gone")).unwrap_err();
    assert_eq!(
        err,
        SwError::NoOtherAccount {
            hint: Some("gone@x.com".into())
        }
    );
    assert_eq!(f.active_token(), "t-me");
}

#[test]
fn small_edges() {
    // Only these two clear up by retrying.
    assert!(!SwError::NoActiveLogin.is_transient());
    assert!(!SwError::Port("x".into()).is_transient());
    // A saved copy needs the Claude login part.
    assert_eq!(crate::Saved::from_json(r#"{"oauthAccount": {}}"#), None);
    // `active` needs someone logged in.
    let logged_out = Fake::default();
    *logged_out.config.borrow_mut() = json!({});
    assert_eq!(
        logged_out.sw().active(false).unwrap_err(),
        SwError::NoActiveLogin
    );
    // A login without an expiry, or without a token, gets no live usage and no identity check.
    let f = Fake::signed_in("me@home.example", "t-me");
    f.usage("t-me", api(0.0, 0.0));
    *f.active.borrow_mut() = Some(json!({"claudeAiOauth": {"accessToken": "t-me"}}).to_string());
    assert!(
        matches!(f.sw().active(false).unwrap(), ActiveOutcome::Stayed { reading, .. } if reading.usage.is_none())
    );
    *f.active.borrow_mut() = Some(json!({"claudeAiOauth": {"expiresAt": LATER_MS}}).to_string());
    assert!(matches!(
        f.sw().active(false).unwrap(),
        ActiveOutcome::Stayed { .. }
    ));
}

#[test]
fn a_failed_profile_write_after_a_logout_reports_and_names_nobody() {
    let f = Fake::default();
    *f.config.borrow_mut() = json!({});
    f.add_saved("me@home.example", "t-me", LATER_MS, None);
    f.failing.borrow_mut().push("config_save");
    assert_eq!(
        f.sw().switch(None).unwrap_err(),
        SwError::Port("config_save failed".into())
    );
    assert!(
        f.config.borrow().get("oauthAccount").is_none(),
        "no account named"
    );
}

/// Later calls of each port fail too (the re-read under the lock, the second account, …).
#[test]
fn a_port_failing_on_a_later_call_is_still_an_error_and_never_half_switches() {
    const METHODS: [&str; 5] = [
        "read_active",
        "read_saved",
        "config_load",
        "list_saved",
        "write_active",
    ];
    type Command = fn(&Switcher<'_>) -> bool;
    let commands: [(&str, Command); 4] = [
        ("switch", |sw| sw.switch(None).is_err()),
        ("active", |sw| sw.active(false).is_err()),
        ("active refused", |sw| sw.active(true).is_err()),
        ("status", |sw| sw.status().is_err()),
    ];
    for method in METHODS {
        for nth in 1..=6 {
            for (here, (label, run)) in [100.0, 10.0]
                .into_iter()
                .flat_map(|h| commands.map(|c| (h, c)))
            {
                let f = Fake::signed_in("me@home.example", "t-me");
                f.usage("t-me", api(here, 50.0));
                f.add_saved("me@corp.example", "t-work", LATER_MS, None);
                f.usage("t-work", api(10.0, 10.0));
                f.fail_nth.set(Some((method, nth)));
                let _ = run(&f.sw());
                f.fail_nth.set(None);
                let owner = &f.owner_by_token.borrow()[&f.active_token()];
                assert_eq!(
                    *owner,
                    f.signed_in_email(),
                    "{label}: {method} failing on call {nth} left a half switch"
                );
            }
        }
    }
}

#[test]
fn a_failed_rollback_still_reports_an_error() {
    let f = Fake::signed_in("me@home.example", "t-me");
    f.add_saved("me@corp.example", "t-work", LATER_MS, None);
    f.failing.borrow_mut().push("config_save");
    f.fail_nth.set(Some(("write_active", 2))); // the rollback write
    assert_eq!(
        f.sw().switch(None).unwrap_err(),
        SwError::Port("write_active failed".into())
    );
}

#[test]
fn a_credential_without_the_claude_part_is_nobody_signed_in() {
    let f = Fake::signed_in("me@home.example", "t-me");
    *f.active.borrow_mut() = Some(json!({"mcpOAuth": {}}).to_string());
    assert_eq!(f.sw().save_current().unwrap_err(), SwError::NoActiveLogin);
}

#[test]
fn an_expired_token_is_not_identity_checked() {
    // The API would refuse an expired token anyway, so sw does not ask.
    let f = Fake::signed_in("me@home.example", "t-me");
    *f.active.borrow_mut() = Some(json!({"claudeAiOauth": oauth("t-me", NOW * 1000)}).to_string());
    f.owner_by_token
        .borrow_mut()
        .insert("t-me".into(), "someone@else.com".into());
    assert_eq!(f.sw().save_current().unwrap().0, "me@home.example");
}

#[test]
fn a_saved_reading_exactly_at_the_reuse_limit_is_asked_again() {
    let f = Fake::signed_in("me@home.example", "t-me");
    f.usage("t-me", api(100.0, 50.0));
    let old = json!({"at": NOW - crate::switcher::USAGE_TTL_SECS, "five_hour": {"pct": 1.0, "resets_at": null},
                     "seven_day": {"pct": 10.0, "resets_at": null}});
    f.add_saved("me@corp.example", "t-work", LATER_MS, Some(old));
    f.usage("t-work", api(0.0, 0.0));
    let _ = f.sw().active(false).unwrap();
    assert_eq!(f.usage_calls.get(), 2, "the other account was asked live");
}

#[test]
fn a_switch_reports_the_target_reading() {
    let f = Fake::signed_in("me@home.example", "t-me");
    f.usage("t-me", api(100.0, 80.0));
    f.add_saved("a@x.com", "t-a", LATER_MS, None);
    f.usage("t-a", api(5.0, 70.0));
    f.add_saved("b@y.com", "t-b", LATER_MS, None);
    f.usage("t-b", api(6.0, 40.0));
    match f.sw().active(false).unwrap() {
        ActiveOutcome::Switched { to, to_reading, .. } => {
            assert_eq!(to, "b@y.com");
            assert_eq!(to_reading.usage.map(|u| u.seven_day.pct), Some(40.0));
        }
        other => panic!("expected a switch, got {other:?}"),
    }
}

#[test]
fn a_name_moves_to_the_account_that_takes_it() {
    let f = Fake::signed_in("me@home.example", "t-me");
    f.add_saved("me@corp.example", "t-work", LATER_MS, None);
    f.sw().name("x").unwrap();
    f.sw().switch(None).unwrap();
    f.sw().name("work").unwrap();
    f.sw().name("x").unwrap(); // corp takes "x"; home loses it, corp loses "work"
    let names = f.names.borrow().clone();
    assert_eq!(names.len(), 1);
    assert_eq!(names["x"], "me@corp.example");
}

#[test]
fn prefetch_lists_exactly_the_calls_that_follow() {
    let f = Fake::signed_in("me@home.example", "t-me");
    let at = |age: i64| json!({"at": NOW - age, "five_hour": {"pct": 1.0, "resets_at": null}, "seven_day": {"pct": 1.0, "resets_at": null}});
    f.add_saved(
        "fresh@x.com",
        "t-fresh",
        LATER_MS,
        Some(at(crate::switcher::USAGE_TTL_SECS - 1)),
    );
    f.add_saved(
        "stale@x.com",
        "t-stale",
        LATER_MS,
        Some(at(crate::switcher::USAGE_TTL_SECS)),
    );
    f.add_saved("never@x.com", "t-never", LATER_MS, None);
    let _ = f.sw().status().unwrap();
    let got = f.prefetched.borrow().clone();
    assert_eq!(
        got,
        [
            "Email(\"t-me\")",
            "Usage(\"t-me\")",
            "Usage(\"t-never\")",
            "Usage(\"t-stale\")"
        ]
        .map(String::from),
        "a saved reading younger than the limit is reused, not fetched"
    );
}

// Refusals. The usage endpoint rounds, and an account that reads 100% can still answer; only a
// refused request makes sw skip an account.

#[test]
fn reading_full_is_not_used_up() {
    // Both read 100% and nothing was refused: the active one may still answer, so sw stays.
    let f = Fake::signed_in("me@home.example", "t-me");
    f.usage("t-me", api(3.0, 100.0));
    f.add_saved("me@corp.example", "t-work", LATER_MS, None);
    f.usage("t-work", api(0.0, 100.0));
    assert!(matches!(
        f.sw().active(false).unwrap(),
        ActiveOutcome::Stayed { .. }
    ));
    assert_eq!(f.active_token(), "t-me");
    assert_eq!(f.saved_copy("me@home.example").limited_until, None);
}

#[test]
fn a_refused_account_is_left_even_for_one_that_reads_full() {
    // The case seen on 2026-10-09: corp refused at week 100%, home reads 100% but answers.
    let f = Fake::signed_in("me@corp.example", "t-work");
    f.usage("t-work", api(0.0, 100.0));
    f.add_saved("me@home.example", "t-me", LATER_MS, None);
    f.usage("t-me", api(3.0, 100.0));
    match f.sw().active(true).unwrap() {
        ActiveOutcome::Switched {
            from,
            to,
            from_limited_until,
            ..
        } => {
            assert_eq!(
                (from.as_str(), to.as_str()),
                ("me@corp.example", "me@home.example")
            );
            assert_eq!(
                from_limited_until,
                Some(WEEK_RESET),
                "skipped until its week resets"
            );
        }
        other => panic!("expected a switch, got {other:?}"),
    }
    assert_eq!(f.active_token(), "t-me");
    let left = f.saved_copy("me@corp.example");
    assert_eq!(
        (left.limited_until, left.left_at),
        (Some(WEEK_RESET), Some(NOW))
    );
}

#[test]
fn a_refused_account_is_never_switched_back_to() {
    let f = Fake::signed_in("me@corp.example", "t-work");
    f.usage("t-work", api(0.0, 100.0));
    f.add_saved("me@home.example", "t-me", LATER_MS, None);
    f.usage("t-me", api_early_week(3.0, 100.0));
    f.sw().active(true).unwrap(); // corp refused: now on home
    // Later home is refused too; corp still is: nothing to move to.
    let later = NOW + crate::switcher::LATE_REPORT_SECS + 1;
    let err = f.sw_at(later).active(true).unwrap_err();
    assert_eq!(
        err,
        SwError::AllLimited {
            next: "home.example".into(),
            free_in: crate::time::until(EARLY_WEEK_RESET, later),
        },
        "home frees first"
    );
    assert_eq!(f.active_token(), "t-me", "nothing switched");
    assert_eq!(
        f.saved_copy("me@home.example").limited_until,
        Some(EARLY_WEEK_RESET)
    );
}

#[test]
fn all_refused_names_the_account_that_frees_first() {
    let f = Fake::signed_in("me@corp.example", "t-work");
    f.usage("t-work", api(0.0, 100.0)); // frees at WEEK_RESET
    f.add_saved("me@home.example", "t-me", LATER_MS, None);
    f.mark("me@home.example", NOW + 600);
    f.names
        .borrow_mut()
        .insert("me".into(), "me@home.example".into());
    assert_eq!(
        f.sw().active(true).unwrap_err(),
        SwError::AllLimited {
            next: "me".into(),
            free_in: "in 10m".into()
        }
    );
    assert_eq!(
        f.saved_copy("me@corp.example").limited_until,
        Some(WEEK_RESET),
        "the refusal is kept"
    );
}

#[test]
fn a_refusal_right_after_a_switch_belongs_to_the_account_left() {
    // sw left home (by hand, so not marked); a request sent before the switch comes back refused.
    let f = Fake::signed_in("me@home.example", "t-me");
    f.usage("t-me", api(3.0, 100.0));
    f.add_saved("me@corp.example", "t-work", LATER_MS, None);
    f.usage("t-work", api(0.0, 20.0));
    f.sw().switch(None).unwrap();
    let out = f.sw_at(NOW + 30).active(true).unwrap();
    assert!(
        matches!(&out, ActiveOutcome::LateReport { email, left } if email == "me@corp.example" && left == "me@home.example"),
        "{out:?}"
    );
    assert_eq!(f.active_token(), "t-work", "corp stays");
    assert_eq!(
        f.saved_copy("me@home.example").limited_until,
        Some(WEEK_RESET)
    );
    assert_eq!(f.saved_copy("me@corp.example").limited_until, None);
}

#[test]
fn a_late_report_keeps_an_existing_refusal_and_renews_an_expired_one() {
    let f = Fake::signed_in("me@corp.example", "t-work");
    f.usage("t-work", api(0.0, 20.0));
    f.add_saved(
        "me@home.example",
        "t-me",
        LATER_MS,
        Some(
            json!({"at": NOW, "five_hour": {"pct": 0.0, "resets_at": null},
                    "seven_day": {"pct": 100.0, "resets_at": "2026-10-11T03:59:59+00:00"}}),
        ),
    );
    let mut v: Value = serde_json::from_str(&f.saved.borrow()["me@home.example"]).unwrap();
    v["left_at"] = json!(NOW - 10);
    v["limited_until"] = json!(NOW + 5);
    f.saved
        .borrow_mut()
        .insert("me@home.example".into(), v.to_string());
    f.sw().active(true).unwrap();
    assert_eq!(
        f.saved_copy("me@home.example").limited_until,
        Some(NOW + 5),
        "still held: kept"
    );
    f.sw_at(NOW + 5).active(true).unwrap();
    assert_eq!(
        f.saved_copy("me@home.example").limited_until,
        Some(WEEK_RESET),
        "ended exactly now: marked again"
    );
}

#[test]
fn a_report_once_the_late_window_has_passed_is_the_active_accounts() {
    let f = Fake::signed_in("me@home.example", "t-me");
    f.usage("t-me", api(3.0, 100.0));
    f.add_saved("me@corp.example", "t-work", LATER_MS, None);
    f.usage("t-work", api(0.0, 20.0));
    f.sw().switch(None).unwrap(); // home left at NOW
    let at = NOW + crate::switcher::LATE_REPORT_SECS;
    assert!(matches!(
        f.sw_at(at).active(true).unwrap(),
        ActiveOutcome::Switched { from, .. } if from == "me@corp.example"
    ));
    assert_eq!(
        f.saved_copy("me@corp.example").limited_until,
        Some(at + crate::decision::LIMIT_FALLBACK_SECS),
        "corp reads 20%: tried again after the fallback"
    );
}

#[test]
fn a_refusal_survives_later_saves_and_switches() {
    let f = Fake::signed_in("me@corp.example", "t-work");
    f.usage("t-work", api(0.0, 100.0));
    f.add_saved("me@home.example", "t-me", LATER_MS, None);
    f.usage("t-me", api(3.0, 40.0));
    f.mark("me@home.example", NOW + 600);
    let _ = f.sw().active(true); // both refused: stays on corp
    let _ = f.sw().status().unwrap(); // saves corp again
    assert_eq!(
        f.saved_copy("me@corp.example").limited_until,
        Some(WEEK_RESET)
    );
    f.sw().switch(None).unwrap(); // by hand to home: corp saved as left
    let left = f.saved_copy("me@corp.example");
    assert_eq!(
        (left.limited_until, left.left_at),
        (Some(WEEK_RESET), Some(NOW))
    );
    f.sw().switch(None).unwrap(); // and back
    let _ = f.sw().status().unwrap(); // corp saved as the active account
    assert_eq!(f.saved_copy("me@corp.example").left_at, None);
}

#[test]
fn active_by_hand_leaves_an_account_still_refused() {
    // A refusal holds; `sw active` without a new one still moves off the account.
    let f = Fake::signed_in("me@corp.example", "t-work");
    f.usage("t-work", api(0.0, 100.0));
    f.sw().save_current().unwrap();
    f.mark("me@corp.example", NOW + 600);
    f.add_saved("me@home.example", "t-me", LATER_MS, None);
    f.usage("t-me", api(3.0, 100.0));
    assert!(matches!(
        f.sw().active(false).unwrap(),
        ActiveOutcome::Switched { from_limited_until: Some(t), .. } if t == NOW + 600
    ));
    // One that ended exactly now no longer counts.
    let g = Fake::signed_in("me@corp.example", "t-work");
    g.usage("t-work", api(100.0, 10.0));
    g.sw().save_current().unwrap();
    g.mark("me@corp.example", NOW);
    g.add_saved("me@home.example", "t-me", LATER_MS, None);
    g.usage("t-me", api(3.0, 20.0));
    assert!(matches!(
        g.sw().active(false).unwrap(),
        ActiveOutcome::Switched {
            from_limited_until: None,
            ..
        }
    ));
}

#[test]
fn status_shows_a_refusal_while_it_holds() {
    let f = Fake::signed_in("me@home.example", "t-me");
    f.usage("t-me", api(3.0, 10.0));
    f.add_saved("me@corp.example", "t-work", LATER_MS, None);
    f.usage("t-work", api(0.0, 100.0));
    f.mark("me@corp.example", NOW + 10);
    let row = |now| {
        f.sw_at(now)
            .status()
            .unwrap()
            .rows
            .into_iter()
            .find(|r| !r.active)
            .unwrap()
            .limited_until
    };
    assert_eq!(row(NOW), Some(NOW + 10));
    assert_eq!(row(NOW + 10), None);
}

#[test]
fn saved_copies_keep_the_two_times_and_read_older_ones() {
    let f = Fake::default();
    f.add_saved("me@home.example", "t-me", LATER_MS, None);
    let old = f.saved_copy("me@home.example");
    assert_eq!(
        (old.limited_until, old.left_at),
        (None, None),
        "Python-era copy"
    );
    let new = crate::Saved {
        limited_until: Some(7),
        left_at: Some(3),
        ..old
    };
    assert_eq!(crate::Saved::from_json(&new.to_json()), Some(new));
}

#[test]
fn a_saved_copy_that_cannot_be_read_is_never_a_target() {
    let f = Fake::signed_in("me@corp.example", "t-work");
    f.usage("t-work", api(0.0, 100.0));
    f.saved
        .borrow_mut()
        .insert("broken@x.com".into(), "not a saved copy".into());
    f.add_saved("me@home.example", "t-me", LATER_MS, None);
    f.usage("t-me", api(3.0, 100.0));
    // Unknown usage would rank the broken one first; it is skipped instead.
    assert!(matches!(
        f.sw().active(true).unwrap(),
        ActiveOutcome::Switched { to, .. } if to == "me@home.example"
    ));
}

#[test]
fn all_refused_with_a_tie_names_the_active_account() {
    let f = Fake::signed_in("me@corp.example", "t-work");
    f.usage("t-work", api(0.0, 100.0)); // frees at WEEK_RESET
    f.add_saved("me@home.example", "t-me", LATER_MS, None);
    f.mark("me@home.example", WEEK_RESET);
    assert!(matches!(
        f.sw().active(true).unwrap_err(),
        SwError::AllLimited { next, .. } if next == "corp.example"
    ));
}

#[test]
fn a_refusal_right_after_switching_to_a_blocked_account_is_its_own() {
    // By hand from home (room left) to corp (blocked): the refusal cannot be home's.
    let f = Fake::signed_in("me@home.example", "t-me");
    f.usage("t-me", api(3.0, 20.0));
    f.add_saved("me@corp.example", "t-work", LATER_MS, None);
    f.usage("t-work", api(0.0, 100.0));
    f.sw().switch(None).unwrap();
    match f.sw_at(NOW + 30).active(true).unwrap() {
        ActiveOutcome::Switched {
            from,
            to,
            from_limited_until,
            ..
        } => {
            assert_eq!(
                (from.as_str(), to.as_str()),
                ("me@corp.example", "me@home.example")
            );
            assert_eq!(from_limited_until, Some(WEEK_RESET));
        }
        other => panic!("expected a switch back, got {other:?}"),
    }
    assert_eq!(f.active_token(), "t-me");
    assert_eq!(f.saved_copy("me@home.example").limited_until, None);
}

#[test]
fn a_late_refusal_goes_to_the_account_left_when_its_usage_is_unknown() {
    let f = Fake::signed_in("me@home.example", "t-me"); // no usage answer for home
    f.add_saved("me@corp.example", "t-work", LATER_MS, None);
    f.usage("t-work", api(0.0, 20.0));
    f.sw().switch(None).unwrap();
    assert!(matches!(
        f.sw_at(NOW + 30).active(true).unwrap(),
        ActiveOutcome::LateReport { left, .. } if left == "me@home.example"
    ));
    assert_eq!(
        f.saved_copy("me@home.example").limited_until,
        Some(NOW + 30 + crate::decision::LIMIT_FALLBACK_SECS)
    );
}
