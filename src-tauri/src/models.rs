use serde::{Deserialize, Serialize};

pub const DEFAULT_QUOTA_REFRESH_INTERVAL_SECONDS: u64 = 5 * 60;
pub const MIN_QUOTA_REFRESH_INTERVAL_SECONDS: u64 = 10;
pub const MAX_QUOTA_REFRESH_INTERVAL_SECONDS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    pub remaining_percent: f64,
    pub resets_at: Option<String>,
    pub window_seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    Chatgpt,
    ApiKey,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiUsageStatus {
    NotConfigured,
    Ok,
    Partial,
    Stale,
    Unavailable,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiUsagePeriod {
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub requests: Option<u64>,
    pub cost_usd: Option<f64>,
}

impl ApiUsagePeriod {
    pub fn has_values(&self) -> bool {
        self.input_tokens.is_some()
            || self.cached_input_tokens.is_some()
            || self.output_tokens.is_some()
            || self.requests.is_some()
            || self.cost_usd.is_some()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiUsageSummary {
    pub status: ApiUsageStatus,
    pub today: ApiUsagePeriod,
    pub month_to_date: ApiUsagePeriod,
    pub updated_at: String,
    pub message: Option<String>,
}

impl ApiUsageSummary {
    pub fn not_configured() -> Self {
        Self {
            status: ApiUsageStatus::NotConfigured,
            today: ApiUsagePeriod::default(),
            month_to_date: ApiUsagePeriod::default(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            message: None,
        }
    }

    pub fn has_values(&self) -> bool {
        self.today.has_values() || self.month_to_date.has_values()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSnapshot {
    pub provider: String,
    pub display_name: String,
    pub auth_mode: AuthMode,
    pub plan: Option<String>,
    pub short_window: Option<UsageWindow>,
    pub weekly_window: Option<UsageWindow>,
    pub reset_credits: Option<u64>,
    pub reset_credit_expires_at: Vec<String>,
    pub updated_at: String,
    pub status: String,
    pub message: Option<String>,
    pub api_usage: Option<ApiUsageSummary>,
}

impl ProviderSnapshot {
    pub fn failure(status: &str, message: &str) -> Self {
        Self::failure_for(AuthMode::Unknown, status, message)
    }

    pub fn failure_for(auth_mode: AuthMode, status: &str, message: &str) -> Self {
        Self {
            provider: "codex".into(),
            display_name: "CODEX".into(),
            auth_mode,
            plan: None,
            short_window: None,
            weekly_window: None,
            reset_credits: None,
            reset_credit_expires_at: Vec::new(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            status: status.into(),
            message: Some(message.into()),
            api_usage: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotState {
    pub snapshots: Vec<ProviderSnapshot>,
    pub refreshing: bool,
    pub revision: u64,
    pub last_attempt_at: Option<String>,
    pub last_success_at: Option<String>,
    pub next_refresh_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WidgetPreferences {
    pub locked: bool,
    #[serde(default = "default_always_on_top")]
    pub always_on_top: bool,
    #[serde(default = "default_widget_visible")]
    pub widget_visible: bool,
    pub pinned_provider: Option<String>,
    pub auto_rotate_seconds: u64,
    #[serde(default = "default_quota_refresh_interval_seconds")]
    pub quota_refresh_interval_seconds: Option<u64>,
    #[serde(default = "default_language")]
    pub language: String,
}

fn default_always_on_top() -> bool {
    true
}
fn default_widget_visible() -> bool {
    true
}
fn default_language() -> String {
    "en".into()
}
fn default_quota_refresh_interval_seconds() -> Option<u64> {
    Some(DEFAULT_QUOTA_REFRESH_INTERVAL_SECONDS)
}

impl Default for WidgetPreferences {
    fn default() -> Self {
        Self {
            locked: false,
            always_on_top: true,
            widget_visible: true,
            pinned_provider: None,
            auto_rotate_seconds: 12,
            quota_refresh_interval_seconds: default_quota_refresh_interval_seconds(),
            language: default_language(),
        }
    }
}

impl WidgetPreferences {
    pub fn normalized(mut self) -> Self {
        self.auto_rotate_seconds = self.auto_rotate_seconds.clamp(5, 300);
        if self.quota_refresh_interval_seconds.is_some_and(|seconds| {
            !(MIN_QUOTA_REFRESH_INTERVAL_SECONDS..=MAX_QUOTA_REFRESH_INTERVAL_SECONDS)
                .contains(&seconds)
        }) {
            self.quota_refresh_interval_seconds = default_quota_refresh_interval_seconds();
        }
        if self.pinned_provider.as_deref() != Some("codex") {
            self.pinned_provider = None;
        }
        if self.language != "en" && self.language != "zh-CN" {
            self.language = default_language();
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_preferences_default_to_five_minute_quota_refresh() {
        let preferences: WidgetPreferences = serde_json::from_str(
            r#"{
                "locked": false,
                "alwaysOnTop": true,
                "widgetVisible": true,
                "pinnedProvider": null,
                "autoRotateSeconds": 12
            }"#,
        )
        .unwrap();

        assert_eq!(
            preferences.quota_refresh_interval_seconds,
            Some(DEFAULT_QUOTA_REFRESH_INTERVAL_SECONDS)
        );
        assert_eq!(preferences.language, "en");
    }

    #[test]
    fn invalid_quota_refresh_interval_falls_back_safely() {
        let preferences = WidgetPreferences {
            quota_refresh_interval_seconds: Some(9),
            ..WidgetPreferences::default()
        }
        .normalized();

        assert_eq!(
            preferences.quota_refresh_interval_seconds,
            Some(DEFAULT_QUOTA_REFRESH_INTERVAL_SECONDS)
        );
        assert_eq!(
            WidgetPreferences {
                quota_refresh_interval_seconds: None,
                ..WidgetPreferences::default()
            }
            .normalized()
            .quota_refresh_interval_seconds,
            None
        );
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopState {
    #[serde(flatten)]
    pub snapshot_state: SnapshotState,
    pub preferences: WidgetPreferences,
    pub widget_visible: bool,
    pub autostart_enabled: bool,
}
