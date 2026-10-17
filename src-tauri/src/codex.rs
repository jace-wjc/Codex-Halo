use std::{fs, path::PathBuf};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION};
use serde_json::Value;

use crate::models::{ProviderSnapshot, UsageWindow};

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CREDITS_URL: &str = "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";
const PROFILE_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/profiles/me";
const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;
const MAX_AUTH_BYTES: u64 = 256 * 1024;

struct Auth {
    access_token: String,
    account_id: Option<String>,
}

#[derive(Clone, Copy)]
struct AuthFailure {
    status: &'static str,
    message: &'static str,
}

impl AuthFailure {
    const fn signed_out(message: &'static str) -> Self {
        Self {
            status: "signed_out",
            message,
        }
    }

    const fn unavailable(message: &'static str) -> Self {
        Self {
            status: "unavailable",
            message,
        }
    }
}

fn auth_metadata_failure(error: &std::io::Error) -> AuthFailure {
    if error.kind() == std::io::ErrorKind::NotFound {
        AuthFailure::signed_out("Please sign in to Codex Desktop first.")
    } else {
        AuthFailure::unavailable("Codex login data is temporarily unavailable.")
    }
}

fn auth_path() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
        .map(|home| home.join("auth.json"))
}

pub fn usage_auth_revision() -> Option<String> {
    let path = auth_path()?;
    let metadata = fs::metadata(&path).ok()?;
    Some(format!(
        "{}:{}:{:?}",
        path.display(),
        metadata.len(),
        metadata.modified().ok()?
    ))
}

fn pick_string<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| value.get(*key)?.as_str())
}

fn account_id_from_jwt(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    pick_string(
        &value,
        &[
            "https://api.openai.com/auth.chatgpt_account_id",
            "chatgpt_account_id",
        ],
    )
    .map(str::to_owned)
}

fn load_auth() -> Result<Auth, AuthFailure> {
    let path = auth_path().ok_or_else(|| {
        AuthFailure::unavailable("Codex login location is temporarily unavailable.")
    })?;
    let metadata = fs::metadata(&path).map_err(|error| auth_metadata_failure(&error))?;
    if !metadata.is_file() || metadata.len() > MAX_AUTH_BYTES {
        return Err(AuthFailure::unavailable(
            "Codex login data is temporarily unavailable.",
        ));
    }
    let raw = fs::read_to_string(path)
        .map_err(|_| AuthFailure::unavailable("Codex login data is temporarily unavailable."))?;
    let value: Value = serde_json::from_str(&raw)
        .map_err(|_| AuthFailure::unavailable("Codex login format has changed."))?;
    let tokens = value.get("tokens").unwrap_or(&value);
    let access_token = pick_string(tokens, &["access_token", "accessToken"])
        .ok_or_else(|| AuthFailure::signed_out("Codex login expired. Please sign in again."))?
        .to_owned();
    let account_id = pick_string(tokens, &["account_id", "accountId"])
        .map(str::to_owned)
        .or_else(|| account_id_from_jwt(&access_token));
    Ok(Auth {
        access_token,
        account_id,
    })
}

fn headers(auth: &Auth) -> Result<HeaderMap, &'static str> {
    let mut result = HeaderMap::new();
    let mut bearer = HeaderValue::from_str(&format!("Bearer {}", auth.access_token))
        .map_err(|_| "Codex login data is invalid.")?;
    bearer.set_sensitive(true);
    result.insert(AUTHORIZATION, bearer);
    result.insert(ACCEPT, HeaderValue::from_static("application/json"));
    result.insert("originator", HeaderValue::from_static("Codex Desktop"));
    result.insert("OAI-Product-Sku", HeaderValue::from_static("CODEX"));
    if let Some(account_id) = &auth.account_id {
        let mut value =
            HeaderValue::from_str(account_id).map_err(|_| "Account identifier is invalid.")?;
        value.set_sensitive(true);
        result.insert("ChatGPT-Account-Id", value);
    }
    Ok(result)
}

fn parse_official_usage(value: &Value) -> Result<crate::usage_stats::OfficialUsage, ()> {
    use crate::usage_stats::{OfficialUsage, OfficialUsageBucket};
    if value
        .pointer("/metadata/stats_error")
        .and_then(Value::as_str)
        .is_some_and(|error| !error.trim().is_empty())
    {
        return Err(());
    }
    let stats = value.get("stats").filter(|v| v.is_object()).ok_or(())?;
    let counters = stats
        .get("agentic")
        .filter(|v| v.is_object())
        .unwrap_or(stats);
    let read_count = |data: &Value, key: &str| -> Result<Option<u64>, ()> {
        match data.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value.as_u64().map(Some).ok_or(()),
        }
    };
    let buckets = |key: &str| -> Result<Vec<OfficialUsageBucket>, ()> {
        let Some(value) = stats.get(key).filter(|v| !v.is_null()) else {
            return Ok(Vec::new());
        };
        let mut rows = Vec::new();
        let mut dates = std::collections::BTreeSet::new();
        for bucket in value.as_array().ok_or(())? {
            let date = bucket.get("start_date").and_then(Value::as_str).ok_or(())?;
            chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| ())?;
            if !dates.insert(date) {
                return Err(());
            }
            rows.push(OfficialUsageBucket {
                date: date.into(),
                total_tokens: bucket.get("tokens").and_then(Value::as_u64).ok_or(())?,
            });
        }
        rows.sort_by(|a, b| a.date.cmp(&b.date));
        Ok(rows)
    };
    let total_tokens = read_count(counters, "lifetime_tokens")?;
    if total_tokens.is_none() {
        return Err(());
    }
    Ok(OfficialUsage {
        status: "ok".into(),
        generated_at: Utc::now().to_rfc3339(),
        as_of: value
            .pointer("/metadata/stats_as_of")
            .and_then(Value::as_str)
            .filter(|date| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok())
            .map(str::to_owned),
        total_tokens,
        peak_daily_tokens: read_count(counters, "peak_daily_tokens")?,
        session_count: read_count(stats, "total_threads")?,
        longest_running_turn_seconds: read_count(counters, "longest_running_turn_sec")?,
        current_streak: read_count(stats, "current_streak_days")?,
        longest_streak: read_count(stats, "longest_streak_days")?,
        daily: buckets("daily_usage_buckets")?,
        weekly: buckets("weekly_usage_buckets")?,
        cumulative: buckets("cumulative_daily_usage_buckets")?,
    })
}

/// Account history is deliberately independent of the quota refresh scheduler.
pub async fn fetch_official_usage(client: &reqwest::Client) -> crate::usage_stats::OfficialUsage {
    use crate::usage_stats::OfficialUsage;
    let auth = match load_auth() {
        Ok(auth) => auth,
        Err(error) => return OfficialUsage::unavailable(error.status),
    };
    let Ok(headers) = headers(&auth) else {
        return OfficialUsage::unavailable("signed_out");
    };
    let response = match client.get(PROFILE_USAGE_URL).headers(headers).send().await {
        Ok(response) if response.status().is_success() => response,
        _ => return OfficialUsage::unavailable("unavailable"),
    };
    match limited_json(response)
        .await
        .ok()
        .and_then(|value| parse_official_usage(&value).ok())
    {
        Some(usage) => usage,
        None => OfficialUsage::unavailable("unavailable"),
    }
}

fn number_with_key<'a>(value: &'a Value, keys: &[&'a str]) -> Option<(&'a str, f64)> {
    keys.iter()
        .find_map(|key| value.get(*key)?.as_f64().map(|number| (*key, number)))
}

fn integer(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        let value = value.get(*key)?;
        value
            .as_u64()
            .or_else(|| value.as_i64().and_then(|item| u64::try_from(item).ok()))
    })
}

fn timestamp(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        let item = value.get(*key)?;
        if let Some(text) = item.as_str() {
            return Some(text.to_owned());
        }
        item.as_i64()
            .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
            .map(|time| time.to_rfc3339())
    })
}

fn collect_reset_credit_expirations(value: &Value) -> Vec<String> {
    fn visit(value: &Value, output: &mut Vec<String>) {
        match value {
            Value::Array(items) => {
                for item in items {
                    visit(item, output);
                }
            }
            Value::Object(map) => {
                if let Some(time) = timestamp(
                    value,
                    &[
                        "expires_at",
                        "expiresAt",
                        "expiration_time",
                        "expirationTime",
                        "expires",
                    ],
                ) {
                    output.push(time);
                }
                for key in [
                    "credits",
                    "reset_credits",
                    "resetCredits",
                    "available",
                    "items",
                    "grants",
                ] {
                    if let Some(child) = map.get(key) {
                        visit(child, output);
                    }
                }
            }
            _ => {}
        }
    }

    let mut expirations = Vec::new();
    visit(value, &mut expirations);
    expirations.sort();
    expirations.dedup();
    expirations
}

fn scale_ratio_field(key: &str, value: f64) -> bool {
    matches!(
        key,
        "remaining_ratio" | "remainingRatio" | "used_ratio" | "usedRatio" | "utilization"
    ) || (!key.contains("percent") && !key.contains("pct") && value <= 1.0)
}

fn parse_window(value: Option<&Value>) -> Option<UsageWindow> {
    let value = value?;
    let remaining_percent = if let Some((key, remaining)) = number_with_key(
        value,
        &[
            "remaining_percent",
            "remainingPercent",
            "remaining_pct",
            "remainingPct",
            "remaining_ratio",
            "remainingRatio",
            "remaining",
        ],
    ) {
        if scale_ratio_field(key, remaining) {
            remaining * 100.0
        } else {
            remaining
        }
    } else {
        let (key, used) = number_with_key(
            value,
            &[
                "used_percent",
                "usedPercent",
                "used_pct",
                "usedPct",
                "used_ratio",
                "usedRatio",
                "utilization",
                "used",
            ],
        )?;
        let used_percent = if scale_ratio_field(key, used) {
            used * 100.0
        } else {
            used
        };
        100.0 - used_percent
    };
    Some(UsageWindow {
        remaining_percent: remaining_percent.clamp(0.0, 100.0),
        resets_at: timestamp(
            value,
            &[
                "reset_at",
                "resetAt",
                "resets_at",
                "resetsAt",
                "reset_time",
                "resetTime",
            ],
        ),
        window_seconds: integer(
            value,
            &[
                "limit_window_seconds",
                "limitWindowSeconds",
                "window_seconds",
                "windowSeconds",
                "duration_seconds",
                "durationSeconds",
                "period_seconds",
                "periodSeconds",
            ],
        )
        .unwrap_or(0),
    })
}

fn find_window<'a>(
    rate_limit: &'a Value,
    names: &[&str],
    expected_seconds: u64,
) -> Option<&'a Value> {
    const DIRECT_WINDOW_KEYS: [&str; 16] = [
        "primary_window",
        "primaryWindow",
        "secondary_window",
        "secondaryWindow",
        "short_window",
        "shortWindow",
        "five_hour_window",
        "fiveHourWindow",
        "weekly_window",
        "weeklyWindow",
        "week_window",
        "weekWindow",
        "5h",
        "primary",
        "weekly",
        "secondary",
    ];

    let matches_duration = |window: &UsageWindow| {
        expected_seconds > 0
            && window.window_seconds > 0
            && window.window_seconds.abs_diff(expected_seconds) <= 60
    };

    // Window roles can change independently of their field names. Prefer the
    // explicit duration so a temporary weekly-only `primary_window` is not
    // mistaken for the historical 5-hour primary window.
    for key in DIRECT_WINDOW_KEYS {
        let Some(value) = rate_limit.get(key) else {
            continue;
        };
        if parse_window(Some(value)).is_some_and(|window| matches_duration(&window)) {
            return Some(value);
        }
    }

    for key in [
        "windows",
        "limit_windows",
        "limitWindows",
        "limits",
        "buckets",
    ] {
        let Some(items) = rate_limit.get(key).and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            let Some(window) = parse_window(Some(item)) else {
                continue;
            };
            if matches_duration(&window) {
                return Some(item);
            }
        }
    }

    // Older payloads sometimes omit duration. In that case, and only then,
    // fall back to the established semantic field or bucket name.
    for name in names {
        if let Some(value) = rate_limit.get(*name) {
            if parse_window(Some(value)).is_some_and(|window| window.window_seconds == 0) {
                return Some(value);
            }
        }
    }

    for key in [
        "windows",
        "limit_windows",
        "limitWindows",
        "limits",
        "buckets",
    ] {
        let Some(items) = rate_limit.get(key).and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            let Some(window) = parse_window(Some(item)) else {
                continue;
            };
            let matches_name = window.window_seconds == 0
                && pick_string(item, &["name", "type", "id", "window", "label"])
                    .map(|text| {
                        let lower = text.to_ascii_lowercase();
                        names.iter().any(|name| {
                            lower == name.to_ascii_lowercase()
                                || lower.contains(&name.to_ascii_lowercase())
                        })
                    })
                    .unwrap_or(false);
            if matches_name {
                return Some(item);
            }
        }
    }

    None
}

fn parse_usage_windows(rate_limit: &Value) -> (Option<UsageWindow>, Option<UsageWindow>) {
    let short_window = parse_window(find_window(
        rate_limit,
        &[
            "primary_window",
            "primaryWindow",
            "short_window",
            "shortWindow",
            "five_hour_window",
            "fiveHourWindow",
            "5h",
            "primary",
        ],
        18_000,
    ));
    let weekly_window = parse_window(find_window(
        rate_limit,
        &[
            "secondary_window",
            "secondaryWindow",
            "weekly_window",
            "weeklyWindow",
            "week_window",
            "weekWindow",
            "weekly",
            "secondary",
        ],
        604_800,
    ));
    (short_window, weekly_window)
}

fn safe_http_failure(status: reqwest::StatusCode) -> (&'static str, &'static str) {
    match status.as_u16() {
        401 | 403 => ("signed_out", "Codex login expired. Please sign in again."),
        429 => (
            "unavailable",
            "Quota service is rate limited. It will retry automatically.",
        ),
        _ => ("unavailable", "Quota service is temporarily unavailable."),
    }
}

async fn limited_json(mut response: reqwest::Response) -> Result<Value, ()> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES)
    {
        return Err(());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
        if bytes.len().saturating_add(chunk.len()) as u64 > MAX_RESPONSE_BYTES {
            return Err(());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| ())
}

pub async fn fetch_snapshot(client: &reqwest::Client) -> ProviderSnapshot {
    let auth = match load_auth() {
        Ok(value) => value,
        Err(failure) => return ProviderSnapshot::failure(failure.status, failure.message),
    };
    let request_headers = match headers(&auth) {
        Ok(value) => value,
        Err(message) => return ProviderSnapshot::failure("signed_out", message),
    };

    let (usage_result, credits_result) = tokio::join!(
        client
            .get(USAGE_URL)
            .headers(request_headers.clone())
            .send(),
        client.get(CREDITS_URL).headers(request_headers).send(),
    );

    let usage_response = match usage_result {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            let (status, message) = safe_http_failure(response.status());
            return ProviderSnapshot::failure(status, message);
        }
        Err(_) => {
            return ProviderSnapshot::failure(
                "unavailable",
                "Network unavailable. It will retry automatically.",
            )
        }
    };
    let usage: Value = match limited_json(usage_response).await {
        Ok(value) => value,
        Err(_) => {
            return ProviderSnapshot::failure("unavailable", "Quota response format has changed.")
        }
    };
    let rate_limit = usage
        .get("rate_limit")
        .or_else(|| usage.get("rateLimit"))
        .unwrap_or(&usage);
    let (short_window, weekly_window) = parse_usage_windows(rate_limit);
    if short_window.is_none() && weekly_window.is_none() {
        return ProviderSnapshot::failure(
            "unavailable",
            "Quota response does not contain a supported usage window.",
        );
    }

    let usage_credits = usage
        .get("rate_limit_reset_credits")
        .or_else(|| usage.get("rateLimitResetCredits"));
    let usage_reset_credits = usage_credits.and_then(|value| {
        integer(
            value,
            &[
                "available_count",
                "availableCount",
                "remaining",
                "count",
                "quantity",
            ],
        )
    });
    let usage_reset_credit_expires_at = usage_credits
        .map(collect_reset_credit_expirations)
        .unwrap_or_default();

    let (reset_credits, reset_credit_expires_at) = match credits_result {
        Ok(response) if response.status().is_success() => match limited_json(response).await.ok() {
            Some(value) => (
                integer(
                    &value,
                    &[
                        "available_count",
                        "availableCount",
                        "remaining",
                        "count",
                        "quantity",
                    ],
                )
                .or(usage_reset_credits),
                {
                    let expirations = collect_reset_credit_expirations(&value);
                    if expirations.is_empty() {
                        usage_reset_credit_expires_at
                    } else {
                        expirations
                    }
                },
            ),
            None => (usage_reset_credits, usage_reset_credit_expires_at),
        },
        _ => (usage_reset_credits, usage_reset_credit_expires_at),
    };

    ProviderSnapshot {
        provider: "codex".into(),
        display_name: "CODEX".into(),
        plan: pick_string(&usage, &["plan_type", "planType"]).map(|value| value.to_uppercase()),
        short_window,
        weekly_window,
        reset_credits,
        reset_credit_expires_at,
        updated_at: chrono::Utc::now().to_rfc3339(),
        status: "ok".into(),
        message: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_overview_keeps_server_summary_and_bucket_values() {
        let usage = parse_official_usage(&serde_json::json!({"stats": {
            "lifetime_tokens": 1234, "peak_daily_tokens": 900,
            "current_streak_days": 3, "longest_streak_days": 5, "total_threads": 4,
            "daily_usage_buckets": [{"start_date":"2026-10-16","tokens":500}],
            "weekly_usage_buckets": [{"start_date":"2026-10-11","tokens":800}],
            "cumulative_daily_usage_buckets": [{"start_date":"2026-10-16","tokens":1234}]
        },"metadata": {"stats_error": null,"stats_as_of":"2026-07-10"}}))
        .unwrap();
        assert_eq!(usage.total_tokens, Some(1234));
        assert_eq!(usage.peak_daily_tokens, Some(900));
        assert_eq!(usage.daily[0].total_tokens, 500);
        assert_eq!(usage.weekly[0].total_tokens, 800);
        assert_eq!(usage.cumulative[0].total_tokens, 1234);
        assert_eq!(usage.longest_running_turn_seconds, None);
        assert_eq!(usage.as_of.as_deref(), Some("2026-07-10"));
    }

    #[test]
    fn official_overview_rejects_errors_and_malformed_numbers_without_zero_fallbacks() {
        for value in [
            serde_json::json!({"stats": {}}),
            serde_json::json!({"stats": {"lifetime_tokens": -1}}),
            serde_json::json!({"stats": {"lifetime_tokens": 10}, "metadata": {"stats_error": "upstream error"}}),
            serde_json::json!({"stats": {"lifetime_tokens": 10, "daily_usage_buckets": [{"start_date":"bad","tokens":12}]}}),
        ] {
            assert!(parse_official_usage(&value).is_err());
        }
        assert_eq!(
            parse_official_usage(&serde_json::json!({"stats":{"lifetime_tokens":0}}))
                .unwrap()
                .total_tokens,
            Some(0)
        );
    }

    #[test]
    fn official_cutoff_is_never_inferred_from_the_fetch_time() {
        for metadata in [
            serde_json::json!({}),
            serde_json::json!({"stats_as_of":"not-a-date"}),
        ] {
            let usage = parse_official_usage(
                &serde_json::json!({"stats":{"lifetime_tokens":123},"metadata":metadata}),
            )
            .unwrap();
            assert_eq!(usage.total_tokens, Some(123));
            assert_eq!(usage.as_of, None);
        }
    }

    #[tokio::test]
    #[ignore = "uses existing ChatGPT login for read-only statistics, never calls a model"]
    async fn live_official_overview() {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(12))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let stats = fetch_official_usage(&client).await;
        println!(
            "official status={}, total={:?}, as_of={:?}, active buckets={}",
            stats.status,
            stats.total_tokens,
            stats.as_of,
            stats.daily.len()
        );
        assert_eq!(stats.status, "ok");
        assert!(stats.total_tokens.is_some());
    }


    #[test]
    fn distinguishes_missing_login_from_transient_auth_io_failures() {
        let missing = auth_metadata_failure(&std::io::Error::from(std::io::ErrorKind::NotFound));
        assert_eq!(missing.status, "signed_out");

        let unreadable =
            auth_metadata_failure(&std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert_eq!(unreadable.status, "unavailable");
    }

    #[test]
    fn parses_both_window_shapes() {
        let snake = serde_json::json!({
            "used_percent": 26,
            "reset_at": 1738300000,
            "limit_window_seconds": 18000
        });
        let window = parse_window(Some(&snake)).unwrap();
        assert_eq!(window.remaining_percent, 74.0);
        assert_eq!(window.window_seconds, 18000);
        let camel = serde_json::json!({
            "utilization": 0.4,
            "resetsAt": "2026-07-07T00:00:00Z",
            "windowSeconds": 604800
        });
        assert_eq!(parse_window(Some(&camel)).unwrap().remaining_percent, 60.0);
    }

    #[test]
    fn prefers_explicit_remaining_percent() {
        let value = serde_json::json!({
            "remainingPercent": 73.4,
            "usedPercent": 99,
            "resetTime": "2026-07-07T00:00:00Z",
            "durationSeconds": 18000
        });
        let window = parse_window(Some(&value)).unwrap();
        assert_eq!(window.remaining_percent, 73.4);
        assert_eq!(window.window_seconds, 18000);
    }

    #[test]
    fn treats_fractional_percent_fields_as_ratios() {
        let explicit_remaining = serde_json::json!({"remaining": 0.25, "periodSeconds": 18000});
        assert_eq!(
            parse_window(Some(&explicit_remaining))
                .unwrap()
                .remaining_percent,
            25.0
        );

        let used_ratio = serde_json::json!({"used": 0.25, "periodSeconds": 18000});
        assert_eq!(
            parse_window(Some(&used_ratio)).unwrap().remaining_percent,
            75.0
        );
    }

    #[test]
    fn does_not_scale_explicit_percent_fields() {
        let explicit_remaining =
            serde_json::json!({"remaining_percent": 0.4, "windowSeconds": 18000});
        assert_eq!(
            parse_window(Some(&explicit_remaining))
                .unwrap()
                .remaining_percent,
            0.4
        );

        let explicit_used = serde_json::json!({"used_percent": 0.4, "windowSeconds": 18000});
        assert_eq!(
            parse_window(Some(&explicit_used))
                .unwrap()
                .remaining_percent,
            99.6
        );
    }

    #[test]
    fn finds_window_by_duration_or_name_in_arrays() {
        let rate_limit = serde_json::json!({
            "windows": [
                {"name": "weekly", "remainingPercent": 88, "windowSeconds": 604800},
                {"name": "primary", "remainingPercent": 51, "windowSeconds": 18000}
            ]
        });
        let short = parse_window(find_window(
            &rate_limit,
            &["primary_window", "primary"],
            18_000,
        ))
        .unwrap();
        let weekly = parse_window(find_window(
            &rate_limit,
            &["secondary_window", "weekly"],
            604_800,
        ))
        .unwrap();
        assert_eq!(short.remaining_percent, 51.0);
        assert_eq!(weekly.remaining_percent, 88.0);
    }

    #[test]
    fn classifies_weekly_only_primary_window_by_duration() {
        let rate_limit = serde_json::json!({
            "primary_window": {
                "remaining_percent": 63,
                "limit_window_seconds": 604800
            }
        });
        let (short, weekly) = parse_usage_windows(&rate_limit);
        assert!(short.is_none());
        assert_eq!(weekly.unwrap().remaining_percent, 63.0);
    }

    #[test]
    fn keeps_five_hour_only_payloads_compatible() {
        let rate_limit = serde_json::json!({
            "primaryWindow": {
                "remainingPercent": 81,
                "windowSeconds": 18000
            }
        });
        let (short, weekly) = parse_usage_windows(&rate_limit);
        assert_eq!(short.unwrap().remaining_percent, 81.0);
        assert!(weekly.is_none());
    }

    #[test]
    fn collects_and_orders_reset_credit_expirations() {
        let value = serde_json::json!({
            "available_count": 2,
            "credits": [
                {"expires_at": "2026-07-17T12:00:00Z"},
                {"expiresAt": "2026-07-16T12:00:00Z"}
            ]
        });
        assert_eq!(
            collect_reset_credit_expirations(&value),
            vec![
                "2026-07-16T12:00:00Z".to_string(),
                "2026-07-17T12:00:00Z".to_string()
            ]
        );
    }
}
