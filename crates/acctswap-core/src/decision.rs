//! The one real decision in sw: which account `sw active` should use. Pure, so it can be proven.
//!
//! The usage endpoint rounds to whole percents, and an account that reads 100% can still answer.
//! So a reading only ranks accounts. What makes sw treat an account as used up is a refusal: the
//! rate-limit hook reports that a request was turned away, and sw then skips that account until
//! [`limit_ends`]. sw never switches to an account it saw refused, so it cannot bounce between two
//! accounts that are both turned away.

use crate::usage::{Usage, Window};

/// An account and what is known about it.
#[derive(Clone, Copy, Debug)]
pub struct Candidate<'a> {
    /// The account (its email).
    pub id: &'a str,
    /// Its usage, when known.
    pub usage: Option<Usage>,
    /// Until when sw skips it, after a request to it was refused (Unix seconds).
    pub limited_until: Option<i64>,
}

impl Candidate<'_> {
    /// True while a refusal seen earlier still holds.
    #[must_use]
    pub fn is_limited(&self, now: i64) -> bool {
        self.limited_until.is_some_and(|t| t > now)
    }
}

/// What `sw active` should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision<'a> {
    /// Keep the active account: it has room, or its usage is unknown, or it reads full but was
    /// not refused and no other account has room.
    Stay,
    /// Move to this account.
    SwitchTo(&'a str),
    /// The active account was refused and so was every other one.
    NoneLeft,
}

/// Rank of an account whose usage is unknown: in the middle.
const UNKNOWN_SCORE: f64 = 50.0;

/// Rank of an account that reads full; every account with room ranks better.
pub const FULL_SCORE: f64 = 100.0;

/// When no window of a refused account reads full (a limit of one model, or no reading at all),
/// it is tried again this many seconds later.
pub const LIMIT_FALLBACK_SECS: i64 = 30 * 60;

/// Rank of an account (lower is better): its weekly percent while it has room, [`FULL_SCORE`]
/// when it reads full. None while a refusal holds.
#[must_use]
pub fn score(c: &Candidate<'_>, now: i64) -> Option<f64> {
    if c.is_limited(now) {
        return None;
    }
    Some(match c.usage {
        None => UNKNOWN_SCORE,
        Some(u) if u.has_room(now) => u.seven_day.pct_at(now),
        Some(_) => FULL_SCORE,
    })
}

/// When an account refused at `now` is tried again: the first reset among its windows that read
/// full, else [`LIMIT_FALLBACK_SECS`] on.
#[must_use]
pub fn limit_ends(usage: Option<Usage>, now: i64) -> i64 {
    let full_reset = |w: Window| w.reset.filter(|_| w.is_full(now));
    match usage.map(|u| (full_reset(u.five_hour), full_reset(u.seven_day))) {
        Some((Some(a), Some(b))) => a.min(b),
        Some((Some(t), None) | (None, Some(t))) => t,
        _ => now.saturating_add(LIMIT_FALLBACK_SECS),
    }
}

/// Stay while the active account was not refused and has room (or its usage is unknown).
/// When it was refused, take the best-ranked account that was not (the first one on a tie), or
/// report that none is left. When it only reads full, move only to an account with room.
#[must_use]
pub fn choose<'a>(current: &Candidate<'_>, others: &[Candidate<'a>], now: i64) -> Decision<'a> {
    let refused = current.is_limited(now);
    if !refused && current.usage.is_none_or(|u| u.has_room(now)) {
        return Decision::Stay;
    }
    let mut best: Option<(f64, &'a str)> = None;
    for c in others {
        if let Some(s) = score(c, now)
            && best.is_none_or(|(b, _)| s < b)
        {
            best = Some((s, c.id));
        }
    }
    match best {
        Some((_, id)) if refused => Decision::SwitchTo(id),
        Some((s, id)) if s < FULL_SCORE => Decision::SwitchTo(id),
        _ if refused => Decision::NoneLeft,
        _ => Decision::Stay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000;

    fn u(five: f64, week: f64) -> Usage {
        Usage {
            at: NOW,
            five_hour: Window {
                pct: five,
                reset: Some(NOW + 10),
            },
            seven_day: Window {
                pct: week,
                reset: Some(NOW + 99),
            },
        }
    }

    fn c(id: &str, usage: Option<Usage>) -> Candidate<'_> {
        Candidate {
            id,
            usage,
            limited_until: None,
        }
    }

    fn refused(id: &str, usage: Option<Usage>) -> Candidate<'_> {
        Candidate {
            limited_until: Some(NOW + 1),
            ..c(id, usage)
        }
    }

    #[test]
    fn stays_with_room_or_when_unknown() {
        assert_eq!(
            choose(&c("me", Some(u(10.0, 96.0))), &[], NOW),
            Decision::Stay
        );
        assert_eq!(
            choose(&c("me", None), &[c("b", Some(u(0.0, 0.0)))], NOW),
            Decision::Stay
        );
    }

    #[test]
    fn a_full_reading_alone_moves_only_to_an_account_with_room() {
        let me = c("me", Some(u(0.0, 100.0)));
        assert_eq!(
            choose(&me, &[c("b", Some(u(0.0, 100.0)))], NOW),
            Decision::Stay,
            "both read full: the active one may still answer"
        );
        assert_eq!(
            choose(&me, &[c("b", Some(u(0.0, 30.0)))], NOW),
            Decision::SwitchTo("b")
        );
        assert_eq!(
            choose(&me, &[refused("b", Some(u(0.0, 0.0)))], NOW),
            Decision::Stay,
            "never to a refused account"
        );
        assert_eq!(choose(&me, &[], NOW), Decision::Stay);
    }

    #[test]
    fn a_refused_account_moves_to_one_that_reads_full_but_was_not_refused() {
        let me = refused("me", Some(u(0.0, 100.0)));
        assert_eq!(
            choose(&me, &[c("b", Some(u(0.0, 100.0)))], NOW),
            Decision::SwitchTo("b")
        );
    }

    #[test]
    fn picks_most_weekly_room_and_ranks_full_ones_last() {
        let others = [
            c("full", Some(u(0.0, 100.0))),
            c("a", Some(u(0.0, 70.0))),
            c("b", Some(u(0.0, 20.0))),
        ];
        assert_eq!(
            choose(&c("me", Some(u(100.0, 10.0))), &others, NOW),
            Decision::SwitchTo("b")
        );
    }

    #[test]
    fn a_full_five_hour_window_ranks_last_even_with_weekly_room() {
        let five_full = c("a", Some(u(100.0, 10.0)));
        assert_eq!(score(&five_full, NOW), Some(FULL_SCORE));
        assert_eq!(
            choose(
                &refused("me", None),
                &[five_full, c("b", Some(u(0.0, 50.0)))],
                NOW
            ),
            Decision::SwitchTo("b")
        );
        assert_eq!(
            choose(&c("me", Some(u(0.0, 100.0))), &[five_full], NOW),
            Decision::Stay,
            "no room there either"
        );
    }

    #[test]
    fn unknown_ranks_between() {
        let others = [c("a", Some(u(0.0, 60.0))), c("x", None)];
        assert_eq!(
            choose(&c("me", Some(u(0.0, 100.0))), &others, NOW),
            Decision::SwitchTo("x")
        );
    }

    #[test]
    fn a_tie_goes_to_the_first_candidate() {
        let others = [c("a", Some(u(0.0, 30.0))), c("b", Some(u(0.0, 30.0)))];
        assert_eq!(
            choose(&c("me", Some(u(100.0, 1.0))), &others, NOW),
            Decision::SwitchTo("a")
        );
    }

    #[test]
    fn none_left_only_when_every_account_was_refused() {
        let me = refused("me", Some(u(0.0, 100.0)));
        assert_eq!(
            choose(&me, &[refused("a", Some(u(0.0, 1.0)))], NOW),
            Decision::NoneLeft
        );
        assert_eq!(choose(&me, &[], NOW), Decision::NoneLeft);
    }

    #[test]
    fn a_refusal_ends_at_its_time() {
        let mut me = refused("me", Some(u(0.0, 0.0)));
        me.limited_until = Some(NOW);
        assert!(!me.is_limited(NOW));
        assert_eq!(choose(&me, &[], NOW), Decision::Stay);
        assert_eq!(score(&me, NOW), Some(0.0));
    }

    #[test]
    fn a_reset_in_the_past_frees_the_account() {
        let mut full = u(0.0, 100.0);
        full.seven_day.reset = Some(NOW - 1);
        assert_eq!(choose(&c("me", Some(full)), &[], NOW), Decision::Stay);
    }

    #[test]
    fn a_refusal_lasts_until_the_first_full_window_resets() {
        // Weekly full: until the weekly reset.
        assert_eq!(limit_ends(Some(u(0.0, 100.0)), NOW), NOW + 99);
        // Both full: the 5-hour window frees first; if the week still blocks, the next refusal
        // marks the account again.
        assert_eq!(limit_ends(Some(u(100.0, 100.0)), NOW), NOW + 10);
        // Nothing reads full, or nothing is known: the fallback, 30 minutes.
        assert_eq!(limit_ends(Some(u(10.0, 20.0)), NOW), NOW + 1800);
        assert_eq!(limit_ends(None, NOW), NOW + LIMIT_FALLBACK_SECS);
        // A full window without a known reset gives no time.
        let mut no_reset = u(100.0, 0.0);
        no_reset.five_hour.reset = None;
        assert_eq!(limit_ends(Some(no_reset), NOW), NOW + LIMIT_FALLBACK_SECS);
    }
}

/// Formal proofs, checked by the Kani model checker for every input within the bounds.
/// Run with `cargo kani -p acctswap-core`.
#[cfg(kani)]
mod proofs {
    use super::*;

    fn any_window() -> Window {
        Window {
            pct: kani::any(),
            reset: kani::any(),
        }
    }

    fn any_usage() -> Usage {
        Usage {
            at: kani::any(),
            five_hour: any_window(),
            seven_day: any_window(),
        }
    }

    fn any_candidate(id: &'static str) -> Candidate<'static> {
        Candidate {
            id,
            usage: if kani::any() { Some(any_usage()) } else { None },
            limited_until: kani::any(),
        }
    }

    /// An account that was not refused and has room (or unknown usage) is never left.
    #[kani::proof]
    fn never_leaves_an_account_with_room() {
        let now: i64 = kani::any();
        let current = any_candidate("me");
        kani::assume(!current.is_limited(now));
        kani::assume(current.usage.is_none_or(|u| u.has_room(now)));
        let others = [any_candidate("a"), any_candidate("b"), any_candidate("c")];
        assert_eq!(choose(&current, &others, now), Decision::Stay);
    }

    /// A switch only ever goes to an account that was not refused, and to the best-ranked one.
    /// Away from an account that was not refused, it goes only to an account with room.
    #[kani::proof]
    fn only_switches_to_the_best_account_not_refused() {
        let now: i64 = kani::any();
        let current = any_candidate("me");
        let others = [any_candidate("a"), any_candidate("b"), any_candidate("c")];
        if let Decision::SwitchTo(id) = choose(&current, &others, now) {
            let chosen = others
                .iter()
                .find(|c| c.id == id)
                .expect("chosen is a candidate");
            let s = score(chosen, now).expect("chosen account was not refused");
            for c in &others {
                if let Some(o) = score(c, now) {
                    assert!(s <= o, "no other candidate ranks better");
                }
            }
            if !current.is_limited(now) {
                assert!(s < FULL_SCORE, "a full reading alone moves only to room");
            }
        }
    }

    /// A refused account is always left when some other account was not refused.
    #[kani::proof]
    fn a_refused_account_is_left_when_another_is_free() {
        let now: i64 = kani::any();
        let current = any_candidate("me");
        kani::assume(current.is_limited(now));
        let others = [any_candidate("a"), any_candidate("b")];
        kani::assume(others.iter().any(|c| !c.is_limited(now)));
        assert!(matches!(
            choose(&current, &others, now),
            Decision::SwitchTo(_)
        ));
    }

    /// "Every account was refused" is only said when it is true.
    #[kani::proof]
    fn none_left_is_only_said_when_true() {
        let now: i64 = kani::any();
        let current = any_candidate("me");
        let others = [any_candidate("a"), any_candidate("b")];
        if choose(&current, &others, now) == Decision::NoneLeft {
            assert!(current.is_limited(now));
            for c in &others {
                assert!(c.is_limited(now));
            }
        }
    }

    /// A refusal always holds for a while, and ends either at the reset of a window that read
    /// full or after the fallback.
    #[kani::proof]
    fn a_refusal_ends_in_the_future() {
        let now: i64 = kani::any();
        kani::assume(now < i64::MAX - LIMIT_FALLBACK_SECS);
        let usage = if kani::any() { Some(any_usage()) } else { None };
        let ends = limit_ends(usage, now);
        assert!(ends > now);
        let full_at = |w: Window| w.is_full(now) && w.reset == Some(ends);
        assert!(
            ends == now + LIMIT_FALLBACK_SECS
                || usage.is_some_and(|u| full_at(u.five_hour) || full_at(u.seven_day))
        );
    }
}
