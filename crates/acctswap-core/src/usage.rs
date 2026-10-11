//! Usage windows (5-hour and weekly) and the rule for "does this account have room left".

use serde_json::{Value, json};

use crate::time::{format_rfc3339, parse_rfc3339, until};

/// A window reads full from this percentage on, until its reset time passes. The usage endpoint
/// rounds to whole percents, and an account that reads 100% can still answer, so a full reading
/// alone never makes sw treat an account as used up: only a refused request does (see `decision`).
pub const FULL_PCT: f64 = 99.5;

/// One rate-limit window: percent used and when it resets (Unix seconds).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Window {
    /// Percent used, 0–100.
    pub pct: f64,
    /// When the window resets (Unix seconds), when known.
    pub reset: Option<i64>,
}

/// Usage of one account at one moment (`at` is Unix seconds).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    /// When this was read (Unix seconds).
    pub at: i64,
    /// The rolling 5-hour window.
    pub five_hour: Window,
    /// The weekly window.
    pub seven_day: Window,
}

impl Window {
    /// Percent used at `now`: 0 once the reset has passed; not a number counts as 100.
    #[must_use]
    pub fn pct_at(&self, now: i64) -> f64 {
        if self.reset.is_some_and(|r| r <= now) {
            0.0
        } else if self.pct.is_finite() {
            self.pct
        } else {
            100.0
        }
    }

    /// Reads full: at or above [`FULL_PCT`] at `now`.
    #[must_use]
    pub fn is_full(&self, now: i64) -> bool {
        self.pct_at(now) >= FULL_PCT
    }

    fn describe(&self, now: i64) -> String {
        match self.reset {
            Some(r) => format!("{:.0}% (resets {})", self.pct, until(r, now)),
            None => format!("{:.0}%", self.pct),
        }
    }

    fn from_json(v: &Value, pct_key: &str) -> Window {
        Window {
            pct: v[pct_key].as_f64().unwrap_or(0.0),
            reset: v["resets_at"].as_str().and_then(parse_rfc3339),
        }
    }

    fn to_json(self) -> Value {
        json!({"pct": self.pct, "resets_at": self.reset.map(format_rfc3339)})
    }
}

impl Usage {
    /// Reads the usage endpoint's JSON (`{"five_hour": {"utilization": 36.0, "resets_at": "…"}, …}`).
    #[must_use]
    pub fn from_api(json: &Value, now: i64) -> Usage {
        Usage {
            at: now,
            five_hour: Window::from_json(&json["five_hour"], "utilization"),
            seven_day: Window::from_json(&json["seven_day"], "utilization"),
        }
    }

    /// Reads usage stored inside a saved login (also the shape the first, Python version wrote).
    #[must_use]
    pub fn from_saved(v: &Value) -> Option<Usage> {
        if !v.is_object() {
            return None;
        }
        Some(Usage {
            #[allow(
                clippy::cast_possible_truncation,
                reason = "seconds: the fraction is dropped on purpose"
            )]
            at: v["at"]
                .as_f64()
                .filter(|a| a.is_finite())
                .map_or(0, |a| a as i64),
            five_hour: Window::from_json(&v["five_hour"], "pct"),
            seven_day: Window::from_json(&v["seven_day"], "pct"),
        })
    }

    /// The JSON stored inside a saved login.
    #[must_use]
    pub fn to_saved(&self) -> Value {
        json!({"at": self.at, "five_hour": self.five_hour.to_json(), "seven_day": self.seven_day.to_json()})
    }

    /// True when neither window reads full at `now`.
    #[must_use]
    pub fn has_room(&self, now: i64) -> bool {
        !self.five_hour.is_full(now) && !self.seven_day.is_full(now)
    }

    /// "5h 36% (resets in 1h 12m), week 96% (resets in 1d 0h)".
    #[must_use]
    pub fn describe(&self, now: i64) -> String {
        format!(
            "5h {}, week {}",
            self.five_hour.describe(now),
            self.seven_day.describe(now)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_791_550_000; // 2026-10-09 ~12:46 UTC

    #[test]
    fn reads_the_api_shape() {
        let api = json!({
            "five_hour": {"utilization": 36.0, "resets_at": "2026-10-09T15:30:00.050249+00:00"},
            "seven_day": {"utilization": 96.0, "resets_at": "2026-10-10T15:00:00.050276+00:00"},
            "seven_day_opus": null
        });
        let u = Usage::from_api(&api, NOW);
        assert_eq!((u.five_hour.pct, u.seven_day.pct), (36.0, 96.0));
        assert_eq!(u.five_hour.reset, Some(1_791_559_800));
        assert!(u.has_room(NOW));
    }

    #[test]
    fn a_full_window_has_no_room_until_it_resets() {
        let week = Window {
            pct: 100.0,
            reset: Some(1_791_691_199),
        };
        let u = Usage {
            at: NOW,
            five_hour: Window::default(),
            seven_day: week,
        };
        assert!(!u.has_room(NOW));
        assert!(u.has_room(1_791_691_200));
        let five = Usage {
            at: NOW,
            five_hour: Window {
                pct: 100.0,
                reset: None,
            },
            seven_day: Window::default(),
        };
        assert!(!five.has_room(NOW), "no reset known: stays full");
    }

    #[test]
    fn a_window_that_resets_right_now_has_room_again() {
        assert!(
            !Window {
                pct: 100.0,
                reset: Some(NOW)
            }
            .is_full(NOW)
        );
        assert!(
            Window {
                pct: 100.0,
                reset: Some(NOW + 1)
            }
            .is_full(NOW)
        );
    }

    #[test]
    fn a_window_reads_zero_once_it_has_reset() {
        let w = Window {
            pct: 100.0,
            reset: Some(NOW),
        };
        assert_eq!((w.pct_at(NOW - 1), w.pct_at(NOW)), (100.0, 0.0));
        let unknown = Window {
            pct: 42.0,
            reset: None,
        };
        assert_eq!(unknown.pct_at(NOW), 42.0);
    }

    #[test]
    fn not_a_number_counts_as_full() {
        assert!(
            Window {
                pct: f64::NAN,
                reset: None
            }
            .is_full(NOW)
        );
    }

    #[test]
    fn missing_window_counts_as_empty() {
        let u = Usage::from_api(
            &json!({"five_hour": {"utilization": 0.0, "resets_at": null}}),
            NOW,
        );
        assert!(u.has_room(NOW));
        assert_eq!(u.seven_day, Window::default());
    }

    #[test]
    fn saved_usage_round_trips_and_reads_the_python_shape() {
        let python = json!({"at": 1_791_550_000.5, "five_hour": {"pct": 37.0, "resets_at": "2026-10-09T15:30:00+00:00"},
                            "seven_day": {"pct": 97.0, "resets_at": "2026-10-10T15:00:00.050276+00:00"}});
        let u = Usage::from_saved(&python).unwrap();
        assert_eq!(
            (u.at, u.seven_day.pct, u.five_hour.reset),
            (1_791_550_000, 97.0, Some(1_791_559_800))
        );
        assert_eq!(Usage::from_saved(&u.to_saved()), Some(u));
        assert_eq!(Usage::from_saved(&Value::Null), None);
    }

    #[test]
    fn describes_windows() {
        let u = Usage {
            at: NOW,
            five_hour: Window {
                pct: 36.4,
                reset: Some(NOW + 3600),
            },
            seven_day: Window {
                pct: 96.0,
                reset: None,
            },
        };
        assert_eq!(u.describe(NOW), "5h 36% (resets in 1h 00m), week 96%");
    }
}
