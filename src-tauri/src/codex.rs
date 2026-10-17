use std::{fs, path::PathBuf};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{Datelike, TimeZone, Utc};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION};
use serde_json::Value;

use crate::models::{
    ApiUsagePeriod, ApiUsageStatus, ApiUsageSummary, AuthMode, ProviderSnapshot, UsageWindow,
};

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CREDITS_URL: &str = "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";
const PROFILE_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/profiles/me";
const ADMIN_USAGE_URL: &str = "https://api.openai.com/v1/organization/usage/completions";
const ADMIN_COSTS_URL: &str = "https://api.openai.com/v1/organization/costs";
const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;
const MAX_AUTH_BYTES: u64 = 256 * 1024;
const MAX_ADMIN_PAGES: usize = 10;

struct Auth {
    access_token: String,
    account_id: Option<String>,
}

enum LocalAuth {
    Chatgpt(Auth),
    ApiKey,
}

#[derive(Clone, Copy, Debug)]
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

fn pick_nonempty_string<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    pick_string(value, keys).filter(|item| !item.trim().is_empty())
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

fn parse_local_auth(value: &Value) -> Result<LocalAuth, AuthFailure> {
    let tokens = value.get("tokens").unwrap_or(value);
    let chatgpt_auth = || {
        let access_token = pick_nonempty_string(tokens, &["access_token", "accessToken"])
            .ok_or_else(|| AuthFailure::signed_out("Codex login expired. Please sign in again."))?;
        let account_id = pick_nonempty_string(tokens, &["account_id", "accountId"])
            .map(str::to_owned)
            .or_else(|| account_id_from_jwt(access_token));
        Ok(LocalAuth::Chatgpt(Auth {
            access_token: access_token.to_owned(),
            account_id,
        }))
    };

    let auth_mode = pick_nonempty_string(value, &["auth_mode", "authMode"])
        .or_else(|| pick_nonempty_string(tokens, &["auth_mode", "authMode"]))
        .map(str::to_ascii_lowercase);
    let has_api_key = pick_nonempty_string(
        value,
        &["OPENAI_API_KEY", "openai_api_key", "api_key", "apiKey"],
    )
    .or_else(|| {
        pick_nonempty_string(
            tokens,
            &["OPENAI_API_KEY", "openai_api_key", "api_key", "apiKey"],
        )
    })
    .is_some();
    // Match Codex AuthDotJson::resolved_mode: an explicit mode wins over
    // legacy credential fields, then an API key is the fallback for old files.
    match auth_mode.as_deref() {
        Some("apikey" | "api_key" | "api-key") => {
            if has_api_key {
                Ok(LocalAuth::ApiKey)
            } else {
                Err(AuthFailure::signed_out(
                    "Codex API-key login is incomplete. Please sign in again.",
                ))
            }
        }
        Some("chatgpt" | "chatgptauthtokens" | "chatgpt_auth_tokens") => chatgpt_auth(),
        Some(_) => Err(AuthFailure::unavailable("Codex login format has changed.")),
        None if has_api_key => Ok(LocalAuth::ApiKey),
        None => chatgpt_auth(),
    }
}

fn load_auth() -> Result<LocalAuth, AuthFailure> {
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
    parse_local_auth(&value)
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
        Ok(LocalAuth::Chatgpt(auth)) => auth,
        Ok(LocalAuth::ApiKey) => return OfficialUsage::unavailable("not_applicable"),
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

fn admin_headers(admin_key: &str) -> Result<HeaderMap, &'static str> {
    let mut result = HeaderMap::new();
    let mut bearer = HeaderValue::from_str(&format!("Bearer {admin_key}"))
        .map_err(|_| "The OpenAI Admin API key is invalid.")?;
    bearer.set_sensitive(true);
    result.insert(AUTHORIZATION, bearer);
    result.insert(ACCEPT, HeaderValue::from_static("application/json"));
    Ok(result)
}

fn admin_http_failure(status: reqwest::StatusCode) -> &'static str {
    match status.as_u16() {
        401 | 403 => "The OpenAI Admin API key was rejected.",
        429 => "The OpenAI organization usage API is rate limited.",
        _ => "The OpenAI organization usage API is temporarily unavailable.",
    }
}

async fn fetch_admin_pages(
    client: &reqwest::Client,
    url: &str,
    admin_key: &str,
    start_time: i64,
    end_time: i64,
) -> Result<Vec<Value>, &'static str> {
    let headers = admin_headers(admin_key)?;
    let mut pages = Vec::new();
    let mut page: Option<String> = None;
    for _ in 0..MAX_ADMIN_PAGES {
        let mut query = vec![
            ("start_time", start_time.to_string()),
            ("end_time", end_time.to_string()),
            ("bucket_width", "1d".to_string()),
            ("limit", "31".to_string()),
        ];
        if let Some(value) = &page {
            query.push(("page", value.clone()));
        }
        let response = client
            .get(url)
            .headers(headers.clone())
            .query(&query)
            .send()
            .await
            .map_err(|_| "Network unavailable while reading OpenAI organization usage.")?;
        if !response.status().is_success() {
            return Err(admin_http_failure(response.status()));
        }
        let value = limited_json(response)
            .await
            .map_err(|_| "The OpenAI organization usage response format has changed.")?;
        let has_more = value
            .get("has_more")
            .or_else(|| value.get("hasMore"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let next_page = pick_nonempty_string(&value, &["next_page", "nextPage"]).map(str::to_owned);
        pages.push(value);
        if !has_more {
            return Ok(pages);
        }
        page = Some(next_page.ok_or("The OpenAI organization usage response format has changed.")?);
    }
    Err("The OpenAI organization usage response exceeded the safe page limit.")
}

#[derive(Clone, Copy, Default)]
struct UsageTotals {
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    requests: u64,
}

impl UsageTotals {
    fn add(&mut self, other: Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(other.cached_input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.requests = self.requests.saturating_add(other.requests);
    }

    fn apply(self, period: &mut ApiUsagePeriod) {
        period.input_tokens = Some(self.input_tokens);
        period.cached_input_tokens = Some(self.cached_input_tokens);
        period.output_tokens = Some(self.output_tokens);
        period.requests = Some(self.requests);
    }
}

fn has_any_key(value: &Value, keys: &[&str]) -> bool {
    keys.iter().any(|key| value.get(*key).is_some())
}

fn parse_admin_usage_pages(
    pages: &[Value],
    today_start: i64,
) -> Result<(UsageTotals, UsageTotals), ()> {
    const INPUT_KEYS: [&str; 2] = ["input_tokens", "inputTokens"];
    const CACHED_KEYS: [&str; 4] = [
        "input_cached_tokens",
        "inputCachedTokens",
        "cached_input_tokens",
        "cachedInputTokens",
    ];
    const OUTPUT_KEYS: [&str; 2] = ["output_tokens", "outputTokens"];
    const REQUEST_KEYS: [&str; 3] = ["num_model_requests", "numModelRequests", "requests"];
    let mut today = UsageTotals::default();
    let mut month = UsageTotals::default();
    for page in pages {
        let buckets = page.get("data").and_then(Value::as_array).ok_or(())?;
        for bucket in buckets {
            let start_time = integer(bucket, &["start_time", "startTime"])
                .and_then(|value| i64::try_from(value).ok())
                .ok_or(())?;
            let results = bucket.get("results").and_then(Value::as_array).ok_or(())?;
            for result in results {
                let recognized = has_any_key(result, &INPUT_KEYS)
                    || has_any_key(result, &CACHED_KEYS)
                    || has_any_key(result, &OUTPUT_KEYS)
                    || has_any_key(result, &REQUEST_KEYS);
                if !recognized {
                    return Err(());
                }
                let totals = UsageTotals {
                    input_tokens: integer(result, &INPUT_KEYS).unwrap_or(0),
                    cached_input_tokens: integer(result, &CACHED_KEYS).unwrap_or(0),
                    output_tokens: integer(result, &OUTPUT_KEYS).unwrap_or(0),
                    requests: integer(result, &REQUEST_KEYS).unwrap_or(0),
                };
                month.add(totals);
                if start_time >= today_start {
                    today.add(totals);
                }
            }
        }
    }
    Ok((today, month))
}

fn finite_number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse::<f64>().ok())
        .filter(|number| number.is_finite() && *number >= 0.0)
}

fn parse_admin_cost_pages(pages: &[Value], today_start: i64) -> Result<(f64, f64), ()> {
    let mut today = 0.0;
    let mut month = 0.0;
    for page in pages {
        let buckets = page.get("data").and_then(Value::as_array).ok_or(())?;
        for bucket in buckets {
            let start_time = integer(bucket, &["start_time", "startTime"])
                .and_then(|value| i64::try_from(value).ok())
                .ok_or(())?;
            let results = bucket.get("results").and_then(Value::as_array).ok_or(())?;
            for result in results {
                let amount = result.get("amount").ok_or(())?;
                let currency = pick_nonempty_string(amount, &["currency"])
                    .unwrap_or("usd")
                    .to_ascii_lowercase();
                if currency != "usd" {
                    return Err(());
                }
                let value = amount.get("value").and_then(finite_number).ok_or(())?;
                month += value;
                if start_time >= today_start {
                    today += value;
                }
            }
        }
    }
    if !today.is_finite() || !month.is_finite() {
        return Err(());
    }
    Ok((today, month))
}

async fn fetch_api_usage(client: &reqwest::Client) -> ApiUsageSummary {
    let Some(admin_key) = std::env::var("OPENAI_ADMIN_KEY")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return ApiUsageSummary::not_configured();
    };
    let now = Utc::now();
    let Some(month_start) = Utc
        .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
        .single()
    else {
        return ApiUsageSummary {
            status: ApiUsageStatus::Unavailable,
            today: ApiUsagePeriod::default(),
            month_to_date: ApiUsagePeriod::default(),
            updated_at: now.to_rfc3339(),
            message: Some("The current billing period could not be calculated.".into()),
        };
    };
    let Some(today_start) = Utc
        .with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
        .single()
    else {
        return ApiUsageSummary {
            status: ApiUsageStatus::Unavailable,
            today: ApiUsagePeriod::default(),
            month_to_date: ApiUsagePeriod::default(),
            updated_at: now.to_rfc3339(),
            message: Some("The current usage day could not be calculated.".into()),
        };
    };
    let (usage_pages, cost_pages) = tokio::join!(
        fetch_admin_pages(
            client,
            ADMIN_USAGE_URL,
            &admin_key,
            month_start.timestamp(),
            now.timestamp(),
        ),
        fetch_admin_pages(
            client,
            ADMIN_COSTS_URL,
            &admin_key,
            month_start.timestamp(),
            now.timestamp(),
        ),
    );
    let usage = usage_pages.and_then(|pages| {
        parse_admin_usage_pages(&pages, today_start.timestamp())
            .map_err(|_| "The OpenAI organization usage response format has changed.")
    });
    let costs = cost_pages.and_then(|pages| {
        parse_admin_cost_pages(&pages, today_start.timestamp())
            .map_err(|_| "The OpenAI organization costs response format has changed.")
    });

    let mut today = ApiUsagePeriod::default();
    let mut month_to_date = ApiUsagePeriod::default();
    if let Ok((today_usage, month_usage)) = usage {
        today_usage.apply(&mut today);
        month_usage.apply(&mut month_to_date);
    }
    if let Ok((today_cost, month_cost)) = costs {
        today.cost_usd = Some(today_cost);
        month_to_date.cost_usd = Some(month_cost);
    }
    let (status, message) = match (usage, costs) {
        (Ok(_), Ok(_)) => (ApiUsageStatus::Ok, None),
        (Err(error), Ok(_)) => (ApiUsageStatus::Partial, Some(error.to_string())),
        (Ok(_), Err(error)) => (ApiUsageStatus::Partial, Some(error.to_string())),
        (Err(left), Err(right)) => {
            let message = if left == right {
                left.to_string()
            } else {
                "OpenAI organization usage and cost data are temporarily unavailable.".into()
            };
            (ApiUsageStatus::Unavailable, Some(message))
        }
    };
    ApiUsageSummary {
        status,
        today,
        month_to_date,
        updated_at: now.to_rfc3339(),
        message,
    }
}

fn api_key_snapshot(api_usage: ApiUsageSummary) -> ProviderSnapshot {
    let unavailable = api_usage.status == ApiUsageStatus::Unavailable;
    ProviderSnapshot {
        provider: "codex".into(),
        display_name: "CODEX".into(),
        auth_mode: AuthMode::ApiKey,
        plan: Some("API".into()),
        short_window: None,
        weekly_window: None,
        reset_credits: None,
        reset_credit_expires_at: Vec::new(),
        updated_at: api_usage.updated_at.clone(),
        status: if unavailable { "unavailable" } else { "ok" }.into(),
        message: api_usage.message.clone(),
        api_usage: Some(api_usage),
    }
}

pub async fn fetch_snapshot(client: &reqwest::Client) -> ProviderSnapshot {
    let auth = match load_auth() {
        Ok(LocalAuth::ApiKey) => return api_key_snapshot(fetch_api_usage(client).await),
        Ok(LocalAuth::Chatgpt(value)) => value,
        Err(failure) => return ProviderSnapshot::failure(failure.status, failure.message),
    };
    let request_headers = match headers(&auth) {
        Ok(value) => value,
        Err(message) => {
            return ProviderSnapshot::failure_for(AuthMode::Chatgpt, "signed_out", message)
        }
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
            return ProviderSnapshot::failure_for(AuthMode::Chatgpt, status, message);
        }
        Err(_) => {
            return ProviderSnapshot::failure_for(
                AuthMode::Chatgpt,
                "unavailable",
                "Network unavailable. It will retry automatically.",
            )
        }
    };
    let usage: Value = match limited_json(usage_response).await {
        Ok(value) => value,
        Err(_) => {
            return ProviderSnapshot::failure_for(
                AuthMode::Chatgpt,
                "unavailable",
                "Quota response format has changed.",
            )
        }
    };
    let rate_limit = usage
        .get("rate_limit")
        .or_else(|| usage.get("rateLimit"))
        .unwrap_or(&usage);
    let (short_window, weekly_window) = parse_usage_windows(rate_limit);
    if short_window.is_none() && weekly_window.is_none() {
        return ProviderSnapshot::failure_for(
            AuthMode::Chatgpt,
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
        auth_mode: AuthMode::Chatgpt,
        plan: pick_string(&usage, &["plan_type", "planType"]).map(|value| value.to_uppercase()),
        short_window,
        weekly_window,
        reset_credits,
        reset_credit_expires_at,
        updated_at: chrono::Utc::now().to_rfc3339(),
        status: "ok".into(),
        message: None,
        api_usage: None,
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
    fn explicit_chatgpt_mode_ignores_a_stale_api_key_field() {
        let value = serde_json::json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": "mock-standard-key",
            "tokens": {
                "access_token": "mock.chatgpt.token",
                "account_id": "mock-account"
            }
        });
        let LocalAuth::Chatgpt(auth) = parse_local_auth(&value).unwrap() else {
            panic!("the explicit ChatGPT mode must match Codex resolved_mode");
        };
        assert_eq!(auth.account_id.as_deref(), Some("mock-account"));
    }

    #[test]
    fn explicit_api_mode_ignores_stale_chatgpt_tokens() {
        let value = serde_json::json!({
            "auth_mode": "apikey",
            "OPENAI_API_KEY": "mock-standard-key",
            "tokens": {
                "access_token": "mock.chatgpt.token",
                "account_id": "mock-account"
            }
        });
        assert!(matches!(parse_local_auth(&value), Ok(LocalAuth::ApiKey)));
    }

    #[test]
    fn detects_api_key_auth_without_exposing_the_key() {
        for value in [
            serde_json::json!({"OPENAI_API_KEY": "mock-standard-key"}),
            serde_json::json!({"auth_mode": "apikey", "OPENAI_API_KEY": "mock-standard-key"}),
            serde_json::json!({"authMode": "api-key", "OPENAI_API_KEY": "mock-standard-key"}),
            serde_json::json!({"tokens": {"apiKey": "mock-standard-key"}}),
        ] {
            assert!(matches!(parse_local_auth(&value), Ok(LocalAuth::ApiKey)));
        }
    }

    #[test]
    fn empty_api_key_does_not_create_a_false_api_session() {
        let value = serde_json::json!({"OPENAI_API_KEY": "  "});
        let failure = parse_local_auth(&value).err().unwrap();
        assert_eq!(failure.status, "signed_out");
    }

    #[test]
    fn explicit_api_mode_requires_a_real_key_marker() {
        let failure = parse_local_auth(&serde_json::json!({"auth_mode": "apikey"}))
            .err()
            .unwrap();
        assert_eq!(failure.status, "signed_out");
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

    #[test]
    fn parses_mock_organization_usage_without_estimating_missing_fields() {
        let pages = vec![serde_json::json!({
            "data": [
                {
                    "start_time": 100,
                    "results": [{
                        "input_tokens": 1200,
                        "input_cached_tokens": 400,
                        "output_tokens": 300,
                        "num_model_requests": 5
                    }]
                },
                {
                    "start_time": 200,
                    "results": [{
                        "input_tokens": 800,
                        "input_cached_tokens": 200,
                        "output_tokens": 100,
                        "num_model_requests": 2
                    }]
                }
            ],
            "has_more": false
        })];
        let (today, month) = parse_admin_usage_pages(&pages, 200).unwrap();
        assert_eq!(today.input_tokens, 800);
        assert_eq!(today.requests, 2);
        assert_eq!(month.input_tokens, 2000);
        assert_eq!(month.cached_input_tokens, 600);
        assert_eq!(month.output_tokens, 400);
        assert_eq!(month.requests, 7);
    }

    #[test]
    fn rejects_mock_usage_results_with_an_unknown_shape() {
        let pages = vec![serde_json::json!({
            "data": [{"start_time": 200, "results": [{"tokens": 999}]}]
        })];
        assert!(parse_admin_usage_pages(&pages, 200).is_err());
    }

    #[test]
    fn parses_mock_usd_costs_and_rejects_other_currencies() {
        let pages = vec![serde_json::json!({
            "data": [
                {"start_time": 100, "results": [{"amount": {"value": 1.25, "currency": "usd"}}]},
                {"start_time": 200, "results": [{"amount": {"value": "0.50", "currency": "usd"}}]}
            ]
        })];
        let (today, month) = parse_admin_cost_pages(&pages, 200).unwrap();
        assert_eq!(today, 0.5);
        assert_eq!(month, 1.75);

        let other_currency = vec![serde_json::json!({
            "data": [{"start_time": 200, "results": [{"amount": {"value": 1, "currency": "eur"}}]}]
        })];
        assert!(parse_admin_cost_pages(&other_currency, 200).is_err());
    }

    #[test]
    fn api_key_snapshot_is_healthy_without_optional_admin_monitoring() {
        let snapshot = api_key_snapshot(ApiUsageSummary::not_configured());
        assert_eq!(snapshot.auth_mode, AuthMode::ApiKey);
        assert_eq!(snapshot.status, "ok");
        assert!(snapshot.short_window.is_none());
        assert!(snapshot.weekly_window.is_none());
        assert_eq!(
            snapshot.api_usage.unwrap().status,
            ApiUsageStatus::NotConfigured
        );
    }
}
