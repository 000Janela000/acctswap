//! Reading and rebuilding the two places a Claude Code login lives:
//! the credential (`claudeAiOauth`, next to MCP logins) and `~/.claude.json`'s `oauthAccount`.

use serde_json::{Map, Value};

/// Keys in `~/.claude.json` that describe the signed-in account and must not survive a switch.
/// Claude Code refetches them for the new account.
pub const ACCOUNT_CACHES: &[&str] = &[
    "cachedUsageUtilization",
    "cachedExtraUsageDisabledReason",
    "overageCreditGrantCache",
    "passesEligibilityCache",
    "modelAccessCache",
    "orgModelDefaultCache",
    "s1mAccessCache",
    "groveConfigCache",
    "additionalModelOptionsCache",
    "additionalModelCostsCache",
];

/// The account part of a credential. Everything next to it (MCP logins) stays on the machine.
pub const OAUTH_KEY: &str = "claudeAiOauth";

/// Puts `oauth` into the active credential and keeps everything else (MCP server logins).
/// A missing or non-object active credential becomes `{}` first (after a logout).
#[must_use]
pub fn merge_oauth(active_credential: Option<&Value>, oauth: &Value) -> Value {
    let mut out = match active_credential {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    };
    out.insert(OAUTH_KEY.into(), oauth.clone());
    Value::Object(out)
}

/// Sets the signed-in account in `~/.claude.json` and drops that account's cached data.
/// Anything that is not a JSON object is replaced by one.
pub fn apply_profile(config: &mut Value, profile: &Value) {
    let mut obj = match std::mem::take(config) {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    obj.insert("oauthAccount".into(), profile.clone());
    for key in ACCOUNT_CACHES {
        obj.remove(*key);
    }
    *config = Value::Object(obj);
}

/// Email of the signed-in account, from `~/.claude.json`.
#[must_use]
pub fn active_email(config: &Value) -> Option<String> {
    config["oauthAccount"]["emailAddress"]
        .as_str()
        .filter(|e| !e.is_empty())
        .map(str::to_owned)
}

/// Unix seconds at which the access token stops working (`expiresAt` is in milliseconds).
#[must_use]
pub fn access_expires(oauth: &Value) -> Option<i64> {
    oauth["expiresAt"].as_i64().map(|ms| ms / 1000)
}

/// Days until the refresh token expires; after that the account needs one /login.
#[must_use]
pub fn refresh_days_left(oauth: &Value, now: i64) -> Option<f64> {
    let secs = oauth["refreshTokenExpiresAt"].as_i64()? / 1000 - now;
    #[allow(
        clippy::cast_precision_loss,
        reason = "days for display; exact below 2^52 seconds"
    )]
    Some(secs as f64 / 86_400.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn swap_keeps_mcp_logins() {
        let active = json!({"claudeAiOauth": {"accessToken": "old"}, "mcpOAuth": {"plugin:figma": {"t": 1}}});
        let out = merge_oauth(Some(&active), &json!({"accessToken": "new"}));
        assert_eq!(out["claudeAiOauth"]["accessToken"], "new");
        assert_eq!(out["mcpOAuth"], active["mcpOAuth"]);
    }

    #[test]
    fn swap_after_logout_starts_from_empty() {
        assert_eq!(
            merge_oauth(None, &json!({"t": 1})),
            json!({"claudeAiOauth": {"t": 1}})
        );
        assert_eq!(
            merge_oauth(Some(&json!("garbage")), &json!({"t": 1})),
            json!({"claudeAiOauth": {"t": 1}})
        );
    }

    #[test]
    fn profile_replaces_account_and_drops_caches() {
        let mut cfg = json!({"oauthAccount": {"emailAddress": "a@x"}, "cachedUsageUtilization": {}, "projects": {"/p": {}}});
        apply_profile(&mut cfg, &json!({"emailAddress": "b@y"}));
        assert_eq!(active_email(&cfg).as_deref(), Some("b@y"));
        assert!(cfg.get("cachedUsageUtilization").is_none());
        assert!(cfg["projects"]["/p"].is_object());
        let mut not_object = json!([1, 2]);
        apply_profile(&mut not_object, &json!({"emailAddress": "c@z"}));
        assert_eq!(active_email(&not_object).as_deref(), Some("c@z"));
    }

    #[test]
    fn empty_email_is_no_email() {
        assert_eq!(
            active_email(&json!({"oauthAccount": {"emailAddress": ""}})),
            None
        );
        assert_eq!(active_email(&json!({})), None);
    }

    #[test]
    fn token_times() {
        let o = json!({"expiresAt": 2_000_000_000_000_i64, "refreshTokenExpiresAt": 1_000_000_000_000_i64 + 86_400_000 * 3});
        assert_eq!(access_expires(&o), Some(2_000_000_000));
        assert_eq!(refresh_days_left(&o, 1_000_000_000), Some(3.0));
        assert_eq!(refresh_days_left(&json!({}), 0), None);
    }
}
