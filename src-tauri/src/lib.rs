mod codex;
mod geometry;
mod models;
mod usage_stats;

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

use geometry::{GeometryController, WorkAreaPayload};
use models::{
    ApiUsageStatus, AuthMode, DesktopState, ProviderSnapshot, SnapshotState, WidgetPreferences,
    MAX_QUOTA_REFRESH_INTERVAL_SECONDS, MIN_QUOTA_REFRESH_INTERVAL_SECONDS,
};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalRect, PhysicalSize, State, WindowEvent,
};
#[cfg(target_os = "macos")]
use tauri::{Position, Rect, Size};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_window_state::{Builder as WindowStateBuilder, StateFlags};

const TRAY_ID: &str = "main";
const MANUAL_REFRESH_DEBOUNCE: Duration = Duration::from_secs(5);
const FAST_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const MAX_FAILURE_BACKOFF: Duration = Duration::from_secs(30 * 60);
const STALE_VALUE_MAX_AGE: Duration = Duration::from_secs(30 * 60);
const PANEL_MARGIN: i32 = 8;
const PANEL_LOGICAL_WIDTH: f64 = 384.0;
const PANEL_LOGICAL_HEIGHT: f64 = 668.0;
const PANEL_BLUR_DISMISS_DELAY: Duration = Duration::from_millis(160);
const LEGACY_CONFIG_IDENTIFIER: &str = "app.quotafloat.desktop";

#[derive(Default)]
struct QuotaRuntime {
    snapshots: Vec<ProviderSnapshot>,
    refreshing: bool,
    revision: u64,
    last_attempt_at: Option<String>,
    last_success_at: Option<String>,
    next_refresh_at: Option<String>,
    last_attempt_instant: Option<Instant>,
    next_refresh_instant: Option<Instant>,
    failure_count: u32,
}

impl QuotaRuntime {
    fn public_state(&self) -> SnapshotState {
        SnapshotState {
            snapshots: self.snapshots.clone(),
            refreshing: self.refreshing,
            revision: self.revision,
            last_attempt_at: self.last_attempt_at.clone(),
            last_success_at: self.last_success_at.clone(),
            next_refresh_at: self.next_refresh_at.clone(),
        }
    }
}

#[derive(Clone)]
struct TrayMenuItems {
    status: MenuItem<tauri::Wry>,
    details: MenuItem<tauri::Wry>,
    refresh: MenuItem<tauri::Wry>,
    widget_visible: CheckMenuItem<tauri::Wry>,
    always_on_top: CheckMenuItem<tauri::Wry>,
    locked: CheckMenuItem<tauri::Wry>,
    autostart: CheckMenuItem<tauri::Wry>,
    language: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}

struct AppState {
    client: reqwest::Client,
    preferences: Mutex<WidgetPreferences>,
    preferences_transaction: Mutex<()>,
    preferences_path: PathBuf,
    quota: Mutex<QuotaRuntime>,
    refresh_lock: tokio::sync::Mutex<()>,
    usage_index_path: PathBuf,
    usage_refresh_lock: tokio::sync::Mutex<()>,
    usage_cache: Mutex<Option<(Instant, Option<String>, usage_stats::UsageStats)>>,
    scheduler_notify: tokio::sync::Notify,
    geometry: GeometryController,
    tray_menu: Mutex<Option<TrayMenuItems>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RefreshTrigger {
    Scheduled,
    Activation,
    Manual,
}

fn lock_unpoison<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn migrate_legacy_app_data(data_dir: &Path) {
    let Some(parent) = data_dir.parent() else {
        return;
    };
    let legacy_dir = parent.join(LEGACY_CONFIG_IDENTIFIER);
    if legacy_dir == data_dir || !legacy_dir.is_dir() || fs::create_dir_all(data_dir).is_err() {
        return;
    }
    for file_name in [
        "preferences.json",
        "preferences.json.bak",
        "usage-index-v1.json",
        ".window-state.json",
    ] {
        let source = legacy_dir.join(file_name);
        let target = data_dir.join(file_name);
        if source.is_file() && !target.exists() {
            let _ = fs::copy(source, target);
        }
    }
}

fn load_preferences(path: &PathBuf) -> WidgetPreferences {
    let parse = |candidate: &PathBuf| {
        fs::read_to_string(candidate)
            .ok()
            .and_then(|raw| serde_json::from_str::<WidgetPreferences>(&raw).ok())
    };
    if let Some(value) = parse(path) {
        return value.normalized();
    }
    let backup = path.with_extension("json.bak");
    if let Some(value) = parse(&backup) {
        eprintln!("preferences recovered from backup");
        return value.normalized();
    }
    WidgetPreferences::default()
}

fn persist_preferences(path: &PathBuf, value: &WidgetPreferences) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|_| "failed to create settings directory".to_string())?;
    }
    let serialized =
        serde_json::to_vec_pretty(value).map_err(|_| "failed to serialize settings".to_string())?;
    let temporary = path.with_extension("json.tmp");
    let backup = path.with_extension("json.bak");
    let mut file = fs::File::create(&temporary)
        .map_err(|_| "failed to create temporary settings file".to_string())?;
    file.write_all(&serialized)
        .and_then(|_| file.sync_all())
        .map_err(|_| "failed to write settings".to_string())?;
    if path.exists() {
        let _ = fs::remove_file(&backup);
        fs::rename(path, &backup).map_err(|_| "failed to back up settings".to_string())?;
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::rename(&backup, path);
        return Err(format!("failed to commit settings: {error}"));
    }
    Ok(())
}

fn merge_snapshots(
    current: &[ProviderSnapshot],
    incoming: Vec<ProviderSnapshot>,
) -> Vec<ProviderSnapshot> {
    incoming
        .into_iter()
        .map(|next| {
            if next.status == "ok" || next.status == "signed_out" {
                return next;
            }
            let previous = current.iter().find(|item| {
                item.provider == next.provider
                    && (next.auth_mode == AuthMode::Unknown || item.auth_mode == next.auth_mode)
                    && (item.short_window.is_some()
                        || item.weekly_window.is_some()
                        || item
                            .api_usage
                            .as_ref()
                            .is_some_and(|usage| usage.has_values()))
            });
            if let Some(previous) = previous {
                let api_failure_message = next
                    .api_usage
                    .as_ref()
                    .and_then(|usage| usage.message.clone())
                    .or_else(|| next.message.clone());
                let mut stale = previous.clone();
                stale.status = "stale".into();
                stale.message = next.message;
                if let Some(api_usage) = stale.api_usage.as_mut() {
                    api_usage.status = ApiUsageStatus::Stale;
                    api_usage.message = api_failure_message;
                }
                stale
            } else {
                next
            }
        })
        .collect()
}

fn snapshot_refresh_succeeded(snapshot: &ProviderSnapshot) -> bool {
    snapshot.status == "ok"
        && !snapshot.api_usage.as_ref().is_some_and(|usage| {
            matches!(
                usage.status,
                ApiUsageStatus::Partial | ApiUsageStatus::Unavailable
            )
        })
}

fn is_near_reset(snapshots: &[ProviderSnapshot], now: chrono::DateTime<chrono::Utc>) -> bool {
    snapshots.iter().any(|snapshot| {
        [&snapshot.short_window, &snapshot.weekly_window]
            .into_iter()
            .flatten()
            .filter_map(|window| window.resets_at.as_deref())
            .filter_map(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&chrono::Utc) - now)
            .any(|remaining| {
                remaining >= chrono::Duration::minutes(-5)
                    && remaining <= chrono::Duration::minutes(15)
            })
    })
}

fn failure_backoff(failure_count: u32) -> Duration {
    let exponent = failure_count.saturating_sub(1).min(6);
    Duration::from_secs(
        (30_u64.saturating_mul(1_u64 << exponent)).min(MAX_FAILURE_BACKOFF.as_secs()),
    )
}

fn stale_expiration_delay(
    snapshots: &[ProviderSnapshot],
    now: chrono::DateTime<chrono::Utc>,
) -> Option<Duration> {
    let max_age = chrono::Duration::from_std(STALE_VALUE_MAX_AGE).ok()?;
    snapshots
        .iter()
        .filter(|snapshot| snapshot.status == "stale")
        .filter_map(|snapshot| chrono::DateTime::parse_from_rfc3339(&snapshot.updated_at).ok())
        .filter_map(|updated_at| {
            let remaining = updated_at.with_timezone(&chrono::Utc) + max_age - now;
            let remaining = remaining.to_std().ok()?;
            if remaining.is_zero() {
                None
            } else {
                Some(remaining.saturating_add(Duration::from_millis(1)))
            }
        })
        .min()
}

fn next_refresh_delay(
    success: bool,
    failure_count: u32,
    snapshots: &[ProviderSnapshot],
    now: chrono::DateTime<chrono::Utc>,
    configured_interval: Duration,
) -> Duration {
    if !success {
        let backoff = failure_backoff(failure_count);
        return stale_expiration_delay(snapshots, now)
            .map(|until_expired| backoff.min(until_expired))
            .unwrap_or(backoff);
    }
    if is_near_reset(snapshots, now) {
        configured_interval.min(FAST_REFRESH_INTERVAL)
    } else {
        configured_interval
    }
}

fn configured_refresh_interval(state: &AppState) -> Option<Duration> {
    lock_unpoison(&state.preferences)
        .quota_refresh_interval_seconds
        .map(Duration::from_secs)
}

fn set_runtime_schedule(
    runtime: &mut QuotaRuntime,
    now_instant: Instant,
    now_wall: chrono::DateTime<chrono::Utc>,
    success: bool,
    configured_interval: Option<Duration>,
) {
    if let Some(configured_interval) = configured_interval {
        let delay = next_refresh_delay(
            success,
            runtime.failure_count,
            &runtime.snapshots,
            now_wall,
            configured_interval,
        );
        runtime.next_refresh_instant = Some(now_instant + delay);
        runtime.next_refresh_at = chrono::Duration::from_std(delay)
            .ok()
            .map(|delay| (now_wall + delay).to_rfc3339());
    } else {
        runtime.next_refresh_instant =
            stale_expiration_delay(&runtime.snapshots, now_wall).map(|delay| now_instant + delay);
        runtime.next_refresh_at = None;
    }
}

fn rounded_percent(value: f64) -> i64 {
    value.clamp(0.0, 100.0).round() as i64
}

fn compact_count(value: u64) -> String {
    if value >= 1_000_000_000 {
        format!("{:.1}B", value as f64 / 1_000_000_000.0)
    } else if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn compact_cost(value: f64) -> String {
    if value >= 1_000_000.0 {
        format!("${:.1}M", value / 1_000_000.0)
    } else if value >= 1_000.0 {
        format!("${:.1}K", value / 1_000.0)
    } else {
        format!("${value:.2}")
    }
}

fn stale_values_expired(snapshot: &ProviderSnapshot, now: chrono::DateTime<chrono::Utc>) -> bool {
    if snapshot.status != "stale" {
        return false;
    }
    let Ok(updated_at) = chrono::DateTime::parse_from_rfc3339(&snapshot.updated_at) else {
        return true;
    };
    let age = now.signed_duration_since(updated_at.with_timezone(&chrono::Utc));
    age > chrono::Duration::from_std(STALE_VALUE_MAX_AGE)
        .unwrap_or_else(|_| chrono::Duration::minutes(30))
}

fn tray_summary(snapshot_state: &SnapshotState) -> String {
    let snapshot = snapshot_state
        .snapshots
        .iter()
        .find(|item| item.provider == "codex")
        .or_else(|| snapshot_state.snapshots.first());
    let Some(snapshot) = snapshot else {
        return if snapshot_state.refreshing {
            "Codex …".into()
        } else {
            "Codex —".into()
        };
    };
    let hide_values = stale_values_expired(snapshot, chrono::Utc::now());
    if snapshot.auth_mode == AuthMode::ApiKey {
        let value = if hide_values {
            "API —".into()
        } else if let Some(cost) = snapshot
            .api_usage
            .as_ref()
            .and_then(|usage| usage.month_to_date.cost_usd)
        {
            format!("API {}", compact_cost(cost))
        } else if let Some(tokens) = snapshot.api_usage.as_ref().and_then(|usage| {
            let period = &usage.month_to_date;
            match (period.input_tokens, period.output_tokens) {
                (Some(input), Some(output)) => Some(input.saturating_add(output)),
                (Some(input), None) => Some(input),
                (None, Some(output)) => Some(output),
                (None, None) => None,
            }
        }) {
            format!("API {} tok", compact_count(tokens))
        } else if snapshot.status == "unavailable" {
            "API —".into()
        } else {
            "API".into()
        };
        let suffix = if snapshot.status == "stale" && !hide_values {
            " ⚠︎"
        } else {
            ""
        };
        return format!("{value}{suffix}");
    }
    let mut windows = Vec::with_capacity(2);
    if let Some(window) = &snapshot.short_window {
        let value = if hide_values {
            "—".into()
        } else {
            format!("{}%", rounded_percent(window.remaining_percent))
        };
        windows.push(format!("5h {value}"));
    }
    if let Some(window) = &snapshot.weekly_window {
        let value = if hide_values {
            "—".into()
        } else {
            format!("{}%", rounded_percent(window.remaining_percent))
        };
        windows.push(format!("W {value}"));
    }
    if windows.is_empty() {
        return if snapshot_state.refreshing {
            "Codex …".into()
        } else {
            "Codex —".into()
        };
    }
    let suffix = if snapshot.status == "stale" && !hide_values {
        " ⚠︎"
    } else {
        ""
    };
    format!("{}{suffix}", windows.join(" · "))
}

fn tray_title(snapshot_state: &SnapshotState) -> String {
    let action = if snapshot_state.refreshing {
        "…"
    } else {
        "↻"
    };
    format!("{}  {action}", tray_summary(snapshot_state))
}

fn refresh_hit_test(position_x: f64, rect_left: f64, rect_width: f64, rect_height: f64) -> bool {
    if rect_width <= 0.0 || rect_height <= 0.0 {
        return false;
    }
    let right = rect_left + rect_width;
    let action_width = rect_height * 0.9;
    position_x >= right - action_width && position_x <= right
}

#[cfg(target_os = "macos")]
fn tray_refresh_hit(position: PhysicalPosition<f64>, rect: Rect) -> bool {
    let (Position::Physical(rect_position), Size::Physical(rect_size)) = (rect.position, rect.size)
    else {
        return false;
    };
    refresh_hit_test(
        position.x,
        rect_position.x as f64,
        rect_size.width as f64,
        rect_size.height as f64,
    )
}

fn tray_tooltip(snapshot_state: &SnapshotState) -> String {
    let summary = tray_summary(snapshot_state);
    let stale_expired = snapshot_state
        .snapshots
        .iter()
        .any(|snapshot| stale_values_expired(snapshot, chrono::Utc::now()));
    let api_snapshot = snapshot_state
        .snapshots
        .iter()
        .find(|snapshot| snapshot.auth_mode == AuthMode::ApiKey);
    let state = if snapshot_state.refreshing {
        "refreshing"
    } else if api_snapshot.is_some_and(|snapshot| {
        snapshot
            .api_usage
            .as_ref()
            .is_some_and(|usage| usage.status == ApiUsageStatus::NotConfigured)
    }) {
        "API usage based; organization totals not configured"
    } else if api_snapshot.is_some_and(|snapshot| {
        snapshot
            .api_usage
            .as_ref()
            .is_some_and(|usage| usage.status == ApiUsageStatus::Partial)
    }) {
        "partial organization usage"
    } else if stale_expired {
        "last known data expired"
    } else if snapshot_state
        .snapshots
        .iter()
        .any(|snapshot| snapshot.status == "signed_out")
    {
        "sign in required"
    } else if snapshot_state
        .snapshots
        .iter()
        .any(|snapshot| snapshot.status == "stale")
    {
        "last known data"
    } else if snapshot_state
        .snapshots
        .iter()
        .any(|snapshot| snapshot.status != "ok")
    {
        "temporarily unavailable"
    } else {
        "up to date"
    };
    format!("Codex Halo · {summary} · {state}")
}

fn publish_snapshot_state(app: &AppHandle, state: &AppState) {
    let snapshot_state = lock_unpoison(&state.quota).public_state();
    let summary = tray_summary(&snapshot_state);
    let tooltip = tray_tooltip(&snapshot_state);
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        #[cfg(target_os = "macos")]
        let _ = tray.set_title(Some(tray_title(&snapshot_state).as_str()));
        let _ = tray.set_tooltip(Some(tooltip.as_str()));
    }
    let status_item = {
        let tray_menu = lock_unpoison(&state.tray_menu);
        tray_menu.as_ref().map(|items| items.status.clone())
    };
    if let Some(status_item) = status_item {
        let _ = status_item.set_text(summary);
    }
    let _ = app.emit("snapshots-changed", snapshot_state);
}

fn should_skip_refresh(
    runtime: &QuotaRuntime,
    trigger: RefreshTrigger,
    now: Instant,
    automatic_refresh_enabled: bool,
) -> bool {
    if runtime.snapshots.is_empty() {
        return false;
    }
    if trigger == RefreshTrigger::Manual {
        return runtime
            .last_attempt_instant
            .is_some_and(|last| now.saturating_duration_since(last) < MANUAL_REFRESH_DEBOUNCE);
    }
    if !automatic_refresh_enabled {
        return true;
    }
    runtime.next_refresh_instant.is_some_and(|next| now < next)
}

async fn refresh_quota(
    app: AppHandle,
    state: &AppState,
    trigger: RefreshTrigger,
) -> Vec<ProviderSnapshot> {
    let _refresh_guard = state.refresh_lock.lock().await;
    let now = Instant::now();
    let automatic_refresh_enabled = configured_refresh_interval(state).is_some();
    {
        let mut runtime = lock_unpoison(&state.quota);
        if should_skip_refresh(&runtime, trigger, now, automatic_refresh_enabled) {
            return runtime.snapshots.clone();
        }
        runtime.refreshing = true;
        runtime.revision = runtime.revision.wrapping_add(1);
    }
    publish_snapshot_state(&app, state);

    let incoming = vec![codex::fetch_snapshot(&state.client).await];
    let success = incoming.iter().all(snapshot_refresh_succeeded);
    let now_wall = chrono::Utc::now();
    let now_instant = Instant::now();
    let configured_interval = configured_refresh_interval(state);
    let snapshots = {
        let mut runtime = lock_unpoison(&state.quota);
        runtime.failure_count = if success {
            0
        } else {
            runtime.failure_count.saturating_add(1)
        };
        runtime.snapshots = merge_snapshots(&runtime.snapshots, incoming);
        runtime.refreshing = false;
        runtime.last_attempt_at = Some(now_wall.to_rfc3339());
        runtime.last_attempt_instant = Some(now_instant);
        if success {
            runtime.last_success_at = Some(now_wall.to_rfc3339());
        }
        set_runtime_schedule(
            &mut runtime,
            now_instant,
            now_wall,
            success,
            configured_interval,
        );
        runtime.revision = runtime.revision.wrapping_add(1);
        runtime.snapshots.clone()
    };
    if trigger != RefreshTrigger::Scheduled {
        state.scheduler_notify.notify_one();
    }
    publish_snapshot_state(&app, state);
    snapshots
}

fn spawn_refresh(app: &AppHandle, trigger: RefreshTrigger) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(state) = app.try_state::<AppState>() {
            let _ = refresh_quota(app.clone(), state.inner(), trigger).await;
        }
    });
}

fn spawn_refresh_scheduler(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let Some(state) = app.try_state::<AppState>() else {
                break;
            };
            let (has_snapshots, next_refresh) = {
                let runtime = lock_unpoison(&state.quota);
                (!runtime.snapshots.is_empty(), runtime.next_refresh_instant)
            };

            if !has_snapshots {
                let _ = refresh_quota(app.clone(), state.inner(), RefreshTrigger::Scheduled).await;
                tokio::task::yield_now().await;
                continue;
            }

            let Some(next_refresh) = next_refresh else {
                state.scheduler_notify.notified().await;
                continue;
            };

            let delay = next_refresh.saturating_duration_since(Instant::now());
            if !delay.is_zero() {
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = state.scheduler_notify.notified() => continue,
                }
            }

            if configured_refresh_interval(state.inner()).is_some() {
                let _ = refresh_quota(app.clone(), state.inner(), RefreshTrigger::Scheduled).await;
            } else {
                let now_wall = chrono::Utc::now();
                let now_instant = Instant::now();
                {
                    let mut runtime = lock_unpoison(&state.quota);
                    runtime.next_refresh_instant =
                        stale_expiration_delay(&runtime.snapshots, now_wall)
                            .map(|next| now_instant + next);
                    runtime.next_refresh_at = None;
                    runtime.revision = runtime.revision.wrapping_add(1);
                }
                publish_snapshot_state(&app, state.inner());
            }
            tokio::task::yield_now().await;
        }
    });
}

fn reschedule_quota(app: &AppHandle, state: &AppState) {
    let configured_interval = configured_refresh_interval(state);
    let now_wall = chrono::Utc::now();
    let now_instant = Instant::now();
    {
        let mut runtime = lock_unpoison(&state.quota);
        if runtime.snapshots.is_empty() {
            runtime.next_refresh_instant = None;
            runtime.next_refresh_at = None;
        } else {
            let success = runtime.failure_count == 0;
            set_runtime_schedule(
                &mut runtime,
                now_instant,
                now_wall,
                success,
                configured_interval,
            );
        }
        runtime.revision = runtime.revision.wrapping_add(1);
    }
    state.scheduler_notify.notify_one();
    publish_snapshot_state(app, state);
}

fn apply_window_preferences(
    app: &AppHandle,
    preferences: &WidgetPreferences,
) -> Result<(), String> {
    let window = app
        .get_webview_window("widget")
        .ok_or_else(|| "widget window missing".to_string())?;
    window
        .set_always_on_top(preferences.always_on_top)
        .map_err(|error| format!("failed to toggle always-on-top: {error}"))?;
    window
        .set_ignore_cursor_events(preferences.locked)
        .map_err(|_| "failed to toggle click-through".to_string())
}

fn commit_preferences<F>(
    app: &AppHandle,
    state: &AppState,
    update: F,
) -> Result<WidgetPreferences, String>
where
    F: FnOnce(&mut WidgetPreferences),
{
    let _transaction = lock_unpoison(&state.preferences_transaction);
    let previous = lock_unpoison(&state.preferences).clone();
    let mut next = previous.clone();
    update(&mut next);
    let next = next.normalized();
    persist_preferences(&state.preferences_path, &next)?;
    if let Err(error) = apply_window_preferences(app, &next) {
        let _ = persist_preferences(&state.preferences_path, &previous);
        let _ = apply_window_preferences(app, &previous);
        return Err(error);
    }
    *lock_unpoison(&state.preferences) = next.clone();
    sync_tray_controls(app, state);
    let _ = app.emit("preferences-changed", next.clone());
    Ok(next)
}

fn change_widget_visibility(
    app: &AppHandle,
    state: &AppState,
    requested: Option<bool>,
) -> Result<bool, String> {
    let _transaction = lock_unpoison(&state.preferences_transaction);
    let previous = lock_unpoison(&state.preferences).clone();
    let visible = requested.unwrap_or(!previous.widget_visible);
    let window = app
        .get_webview_window("widget")
        .ok_or_else(|| "widget window missing".to_string())?;
    if previous.widget_visible == visible {
        if visible {
            let _ = geometry::ensure_widget_visible(app, &state.geometry);
            window
                .show()
                .map_err(|error| format!("failed to show widget: {error}"))?;
        } else {
            window
                .hide()
                .map_err(|error| format!("failed to hide widget: {error}"))?;
        }
        sync_tray_controls(app, state);
        return Ok(visible);
    }
    let mut next = previous.clone();
    next.widget_visible = visible;
    persist_preferences(&state.preferences_path, &next)?;
    let result = if visible {
        let _ = geometry::ensure_widget_visible(app, &state.geometry);
        window.show()
    } else {
        window.hide()
    };
    if let Err(error) = result {
        let _ = persist_preferences(&state.preferences_path, &previous);
        return Err(format!("failed to change widget visibility: {error}"));
    }
    *lock_unpoison(&state.preferences) = next.clone();
    sync_tray_controls(app, state);
    let _ = app.emit("preferences-changed", next);
    let _ = app.emit("widget-visibility-changed", visible);
    Ok(visible)
}

fn set_autostart_impl(app: &AppHandle, enabled: bool) -> Result<bool, String> {
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    result.map_err(|_| "failed to update start-at-login".to_string())?;
    let actual = manager.is_enabled().unwrap_or(enabled);
    if let Some(state) = app.try_state::<AppState>() {
        sync_tray_controls(app, state.inner());
    }
    let _ = app.emit("autostart-changed", actual);
    Ok(actual)
}

fn sync_tray_controls(app: &AppHandle, state: &AppState) {
    let preferences = lock_unpoison(&state.preferences).clone();
    let autostart = app.autolaunch().is_enabled().unwrap_or(false);
    let items = {
        let tray_menu = lock_unpoison(&state.tray_menu);
        tray_menu.as_ref().cloned()
    };
    let Some(items) = items else {
        return;
    };
    let english = preferences.language == "en";
    let _ = items.widget_visible.set_checked(preferences.widget_visible);
    let _ = items.always_on_top.set_checked(preferences.always_on_top);
    let _ = items.locked.set_checked(preferences.locked);
    let _ = items.autostart.set_checked(autostart);
    let _ = items.details.set_text(if english {
        "Open quota details"
    } else {
        "打开额度详情"
    });
    let _ = items.refresh.set_text(if english {
        "Refresh now"
    } else {
        "立即刷新"
    });
    let _ = items.widget_visible.set_text(if english {
        "Show floating widget"
    } else {
        "显示悬浮窗"
    });
    let _ = items.always_on_top.set_text(if english {
        "Keep widget on top"
    } else {
        "悬浮窗保持置顶"
    });
    let _ = items.locked.set_text(if english {
        "Click-through widget"
    } else {
        "悬浮窗鼠标穿透"
    });
    let _ = items.autostart.set_text(if english {
        "Start at login"
    } else {
        "开机启动"
    });
    let _ = items.language.set_text(if english {
        "切换到中文"
    } else {
        "Switch to English"
    });
    let _ = items.quit.set_text(if english { "Quit" } else { "退出" });
}

fn panel_position(
    anchor: PhysicalPosition<f64>,
    panel_size: PhysicalSize<u32>,
    work_area: PhysicalRect<i32, u32>,
) -> PhysicalPosition<i32> {
    let left = work_area.position.x + PANEL_MARGIN;
    let top = work_area.position.y + PANEL_MARGIN;
    let right = work_area.position.x + work_area.size.width as i32 - PANEL_MARGIN;
    let bottom = work_area.position.y + work_area.size.height as i32 - PANEL_MARGIN;
    let max_x = (right - panel_size.width as i32).max(left);
    let max_y = (bottom - panel_size.height as i32).max(top);
    let x = (anchor.x.round() as i32 - panel_size.width as i32 / 2).clamp(left, max_x);
    let anchor_y = anchor.y.round() as i32;
    let y = if anchor_y <= work_area.position.y {
        top
    } else if anchor_y >= work_area.position.y + work_area.size.height as i32 {
        max_y
    } else if anchor_y < work_area.position.y + work_area.size.height as i32 / 2 {
        (anchor_y + 12).clamp(top, max_y)
    } else {
        (anchor_y - panel_size.height as i32 - 12).clamp(top, max_y)
    };
    PhysicalPosition::new(x, y)
}

fn panel_physical_size(scale_factor: f64) -> PhysicalSize<u32> {
    PhysicalSize::new(
        (PANEL_LOGICAL_WIDTH * scale_factor).round().max(1.0) as u32,
        (PANEL_LOGICAL_HEIGHT * scale_factor).round().max(1.0) as u32,
    )
}

fn show_panel_at(
    app: &AppHandle,
    anchor: PhysicalPosition<f64>,
    toggle: bool,
) -> Result<(), String> {
    let panel = app
        .get_webview_window("panel")
        .ok_or_else(|| "panel window missing".to_string())?;
    if toggle && panel.is_visible().unwrap_or(false) {
        panel
            .hide()
            .map_err(|_| "failed to hide panel".to_string())?;
        return Ok(());
    }
    let monitor = app
        .monitor_from_point(anchor.x, anchor.y)
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten())
        .ok_or_else(|| "no monitor available".to_string())?;
    let panel_size = panel_physical_size(monitor.scale_factor());
    panel
        .set_position(panel_position(anchor, panel_size, *monitor.work_area()))
        .map_err(|_| "failed to position panel".to_string())?;
    #[cfg(target_os = "macos")]
    let _ = panel.set_visible_on_all_workspaces(true);
    panel
        .show()
        .map_err(|_| "failed to show panel".to_string())?;
    if let Err(error) = panel.set_focus() {
        let _ = panel.hide();
        return Err(format!("failed to focus panel: {error}"));
    }
    spawn_refresh(app, RefreshTrigger::Activation);
    Ok(())
}

fn show_panel_near_cursor(app: &AppHandle, toggle: bool) {
    let anchor = app
        .cursor_position()
        .unwrap_or_else(|_| PhysicalPosition::new(0.0, 0.0));
    let _ = show_panel_at(app, anchor, toggle);
}

fn setup_tray(app: &tauri::App, preferences: &WidgetPreferences) -> tauri::Result<TrayMenuItems> {
    let english = preferences.language == "en";
    let status = MenuItem::with_id(app, "status", "Codex …", false, None::<&str>)?;
    let details = MenuItem::with_id(
        app,
        "details",
        if english {
            "Open quota details"
        } else {
            "打开额度详情"
        },
        true,
        None::<&str>,
    )?;
    let refresh = MenuItem::with_id(
        app,
        "refresh",
        if english {
            "Refresh now"
        } else {
            "立即刷新"
        },
        true,
        None::<&str>,
    )?;
    let widget_visible = CheckMenuItem::with_id(
        app,
        "widget-visible",
        if english {
            "Show floating widget"
        } else {
            "显示悬浮窗"
        },
        true,
        preferences.widget_visible,
        None::<&str>,
    )?;
    let always_on_top = CheckMenuItem::with_id(
        app,
        "always-on-top",
        if english {
            "Keep widget on top"
        } else {
            "悬浮窗保持置顶"
        },
        true,
        preferences.always_on_top,
        None::<&str>,
    )?;
    let locked = CheckMenuItem::with_id(
        app,
        "locked",
        if english {
            "Click-through widget"
        } else {
            "悬浮窗鼠标穿透"
        },
        true,
        preferences.locked,
        None::<&str>,
    )?;
    let autostart_enabled = app.autolaunch().is_enabled().unwrap_or(false);
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        if english {
            "Start at login"
        } else {
            "开机启动"
        },
        true,
        autostart_enabled,
        None::<&str>,
    )?;
    let language = MenuItem::with_id(
        app,
        "language",
        if english {
            "切换到中文"
        } else {
            "Switch to English"
        },
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(
        app,
        "quit",
        if english { "Quit" } else { "退出" },
        true,
        None::<&str>,
    )?;
    let separator_one = PredefinedMenuItem::separator(app)?;
    let separator_two = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &details,
            &refresh,
            &separator_one,
            &widget_visible,
            &always_on_top,
            &locked,
            &autostart,
            &language,
            &separator_two,
            &quit,
        ],
    )?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("Codex Halo · loading quota")
        .icon(tauri::include_image!("icons/tray-icon.png"));
    #[cfg(target_os = "macos")]
    {
        builder = builder.icon_as_template(true).title("Codex …  ↻");
    }
    builder
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "details" => show_panel_near_cursor(app, false),
            "refresh" => spawn_refresh(app, RefreshTrigger::Manual),
            "widget-visible" => {
                if let Some(state) = app.try_state::<AppState>() {
                    let _ = change_widget_visibility(app, state.inner(), None);
                }
            }
            "always-on-top" => {
                if let Some(state) = app.try_state::<AppState>() {
                    let _ = commit_preferences(app, state.inner(), |next| {
                        next.always_on_top = !next.always_on_top;
                    });
                }
            }
            "locked" => {
                if let Some(state) = app.try_state::<AppState>() {
                    let _ = commit_preferences(app, state.inner(), |next| {
                        next.locked = !next.locked;
                    });
                }
            }
            "autostart" => {
                let enabled = app.autolaunch().is_enabled().unwrap_or(false);
                let _ = set_autostart_impl(app, !enabled);
            }
            "language" => {
                if let Some(state) = app.try_state::<AppState>() {
                    let _ = commit_preferences(app, state.inner(), |next| {
                        next.language = if next.language == "en" {
                            "zh-CN".into()
                        } else {
                            "en".into()
                        };
                    });
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(TrayMenuItems {
        status,
        details,
        refresh,
        widget_visible,
        always_on_top,
        locked,
        autostart,
        language,
        quit,
    })
}

#[tauri::command]
fn get_app_state(app: AppHandle, state: State<'_, AppState>) -> Result<DesktopState, String> {
    let preferences = lock_unpoison(&state.preferences).clone();
    let widget_visible = app
        .get_webview_window("widget")
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(preferences.widget_visible);
    let autostart_enabled = app.autolaunch().is_enabled().unwrap_or(false);
    Ok(DesktopState {
        snapshot_state: lock_unpoison(&state.quota).public_state(),
        preferences,
        widget_visible,
        autostart_enabled,
    })
}

#[tauri::command]
async fn get_snapshots(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<ProviderSnapshot>, String> {
    let snapshots = lock_unpoison(&state.quota).snapshots.clone();
    if !snapshots.is_empty() {
        return Ok(snapshots);
    }
    Ok(refresh_quota(app, state.inner(), RefreshTrigger::Activation).await)
}

#[tauri::command]
async fn refresh_snapshots(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<ProviderSnapshot>, String> {
    Ok(refresh_quota(app, state.inner(), RefreshTrigger::Manual).await)
}

#[tauri::command]
async fn get_usage_stats(
    force: Option<bool>,
    state: State<'_, AppState>,
) -> Result<usage_stats::UsageStats, String> {
    let _refresh = state.usage_refresh_lock.lock().await;
    let auth_revision = codex::usage_auth_revision();
    if !force.unwrap_or(false) {
        if let Some((fetched, revision, stats)) = lock_unpoison(&state.usage_cache).as_ref() {
            if *revision == auth_revision && fetched.elapsed() < Duration::from_secs(60) {
                return Ok(stats.clone());
            }
        }
    }
    let index_path = state.usage_index_path.clone();
    let (local, official) = tokio::join!(
        tokio::task::spawn_blocking(move || usage_stats::load_usage_stats(&index_path)),
        codex::fetch_official_usage(&state.client),
    );
    let mut stats = local
        .ok()
        .and_then(Result::ok)
        .unwrap_or_else(usage_stats::UsageStats::unavailable);
    stats.official = Some(official);
    *lock_unpoison(&state.usage_cache) = Some((Instant::now(), auth_revision, stats.clone()));
    Ok(stats)
}

#[tauri::command]
fn expand_widget(
    work_area: Option<WorkAreaPayload>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    geometry::expand_widget(&app, &state.geometry, work_area)
}

#[tauri::command]
fn collapse_widget(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    geometry::collapse_widget(&app, &state.geometry)
}

#[tauri::command]
fn begin_widget_drag(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    geometry::begin_widget_drag(&app, &state.geometry)
}

#[tauri::command]
fn finish_widget_drag(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    geometry::finish_widget_drag(&app, &state.geometry)
}

#[tauri::command]
fn get_preferences(state: State<'_, AppState>) -> Result<WidgetPreferences, String> {
    Ok(lock_unpoison(&state.preferences).clone())
}

#[tauri::command]
fn set_preferences(
    preferences: WidgetPreferences,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let previous_interval = lock_unpoison(&state.preferences).quota_refresh_interval_seconds;
    let next = commit_preferences(&app, state.inner(), |current| {
        let widget_visible = current.widget_visible;
        *current = preferences;
        current.widget_visible = widget_visible;
    })?;
    if previous_interval != next.quota_refresh_interval_seconds {
        reschedule_quota(&app, state.inner());
    }
    Ok(())
}

#[tauri::command]
fn set_widget_locked(
    locked: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<WidgetPreferences, String> {
    commit_preferences(&app, state.inner(), |next| next.locked = locked)
}

#[tauri::command]
fn set_widget_always_on_top(
    always_on_top: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<WidgetPreferences, String> {
    commit_preferences(&app, state.inner(), |next| {
        next.always_on_top = always_on_top;
    })
}

#[tauri::command]
fn set_widget_visible(
    visible: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    change_widget_visibility(&app, state.inner(), Some(visible))
}

#[tauri::command]
fn get_autostart(app: AppHandle) -> Result<bool, String> {
    app.autolaunch()
        .is_enabled()
        .map_err(|_| "failed to read start-at-login state".to_string())
}

#[tauri::command]
fn set_autostart(enabled: bool, app: AppHandle) -> Result<bool, String> {
    set_autostart_impl(&app, enabled)
}

#[tauri::command]
fn set_language(
    language: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<WidgetPreferences, String> {
    commit_preferences(&app, state.inner(), |next| next.language = language)
}

#[tauri::command]
fn set_quota_refresh_interval(
    interval_seconds: Option<u64>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<WidgetPreferences, String> {
    if interval_seconds.is_some_and(|seconds| {
        !(MIN_QUOTA_REFRESH_INTERVAL_SECONDS..=MAX_QUOTA_REFRESH_INTERVAL_SECONDS)
            .contains(&seconds)
    }) {
        return Err(format!(
            "quota refresh interval must be between {MIN_QUOTA_REFRESH_INTERVAL_SECONDS} and {MAX_QUOTA_REFRESH_INTERVAL_SECONDS} seconds"
        ));
    }

    let previous_interval = lock_unpoison(&state.preferences).quota_refresh_interval_seconds;
    let next = commit_preferences(&app, state.inner(), |preferences| {
        preferences.quota_refresh_interval_seconds = interval_seconds;
    })?;
    if previous_interval != next.quota_refresh_interval_seconds {
        reschedule_quota(&app, state.inner());
    }
    Ok(next)
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

fn enable_window_fallback(app: &tauri::App, state: &AppState) {
    eprintln!("tray setup failed; enabling window fallback");
    let _transaction = lock_unpoison(&state.preferences_transaction);
    let mut preferences = lock_unpoison(&state.preferences).clone();
    preferences.widget_visible = true;
    let _ = persist_preferences(&state.preferences_path, &preferences);
    *lock_unpoison(&state.preferences) = preferences;
    if let Some(window) = app.get_webview_window("widget") {
        let _ = window.set_skip_taskbar(false);
        let _ = geometry::ensure_widget_visible(app.handle(), &state.geometry);
        let _ = window.show();
        let _ = window.set_focus();
    }
    #[cfg(target_os = "macos")]
    let _ = app
        .handle()
        .set_activation_policy(tauri::ActivationPolicy::Regular);
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_panel_near_cursor(app, false);
            spawn_refresh(app, RefreshTrigger::Activation);
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(
            WindowStateBuilder::default()
                .with_filter(|label| label == "widget")
                .with_state_flags(StateFlags::POSITION)
                .build(),
        )
        .setup(|app| {
            let data_dir = app.path().app_config_dir()?;
            migrate_legacy_app_data(&data_dir);
            let preferences_path = data_dir.join("preferences.json");
            let usage_index_path = data_dir.join("usage-index-v4.json");
            let preferences = load_preferences(&preferences_path);
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(12))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(format!("CodexHalo/{}", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("static HTTP client configuration must be valid");
            app.manage(AppState {
                client,
                preferences: Mutex::new(preferences.clone()),
                preferences_transaction: Mutex::new(()),
                preferences_path,
                quota: Mutex::new(QuotaRuntime::default()),
                refresh_lock: tokio::sync::Mutex::new(()),
                usage_index_path,
                usage_refresh_lock: tokio::sync::Mutex::new(()),
                usage_cache: Mutex::new(None),
                scheduler_notify: tokio::sync::Notify::new(),
                geometry: GeometryController::default(),
                tray_menu: Mutex::new(None),
            });
            let state = app.state::<AppState>();
            match setup_tray(app, &preferences) {
                Ok(items) => {
                    *lock_unpoison(&state.tray_menu) = Some(items);
                    #[cfg(target_os = "macos")]
                    if let Err(error) = app
                        .handle()
                        .set_activation_policy(tauri::ActivationPolicy::Accessory)
                    {
                        eprintln!("failed to switch to menu-bar-only activation policy: {error}");
                    }
                }
                Err(_) => enable_window_fallback(app, state.inner()),
            }
            let preferences = lock_unpoison(&state.preferences).clone();
            if let Some(window) = app.get_webview_window("widget") {
                let _ = apply_window_preferences(app.handle(), &preferences);
                if preferences.widget_visible {
                    let _ = geometry::ensure_widget_visible(app.handle(), &state.geometry);
                    let _ = window.show();
                } else {
                    let _ = window.hide();
                }
            }
            if let Some(panel) = app.get_webview_window("panel") {
                let _ = panel.hide();
            }
            sync_tray_controls(app.handle(), state.inner());
            publish_snapshot_state(app.handle(), state.inner());
            spawn_refresh_scheduler(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_app_state,
            get_snapshots,
            refresh_snapshots,
            get_usage_stats,
            expand_widget,
            collapse_widget,
            begin_widget_drag,
            finish_widget_drag,
            get_preferences,
            set_preferences,
            set_widget_locked,
            set_widget_always_on_top,
            set_widget_visible,
            get_autostart,
            set_autostart,
            set_language,
            set_quota_refresh_interval,
            quit_app
        ])
        .on_tray_icon_event(|app, event| {
            if let TrayIconEvent::Click {
                id,
                position,
                rect,
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if id.as_ref() == TRAY_ID {
                    #[cfg(target_os = "macos")]
                    if tray_refresh_hit(position, rect) {
                        spawn_refresh(app, RefreshTrigger::Manual);
                        return;
                    }
                    let _ = show_panel_at(app, position, true);
                }
            }
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                if window.label() == "widget" {
                    if let Some(state) = window.app_handle().try_state::<AppState>() {
                        let _ = change_widget_visibility(
                            window.app_handle(),
                            state.inner(),
                            Some(false),
                        );
                    } else {
                        let _ = window.hide();
                    }
                } else {
                    let _ = window.hide();
                }
            }
            WindowEvent::Focused(false) if window.label() == "panel" => {
                let panel = window.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(PANEL_BLUR_DISMISS_DELAY).await;
                    if !panel.is_focused().unwrap_or(false) {
                        let _ = panel.hide();
                    }
                });
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("failed to build Codex Halo");
    app.run(|app_handle, event| {
        if matches!(event, tauri::RunEvent::Resumed) {
            if let Some(state) = app_handle.try_state::<AppState>() {
                let preferences = lock_unpoison(&state.preferences).clone();
                if preferences.widget_visible {
                    let _ = geometry::ensure_widget_visible(app_handle, &state.geometry);
                }
            }
            spawn_refresh(app_handle, RefreshTrigger::Activation);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(status: &str, short: Option<f64>, weekly: Option<f64>) -> ProviderSnapshot {
        ProviderSnapshot {
            provider: "codex".into(),
            display_name: "CODEX".into(),
            auth_mode: AuthMode::Chatgpt,
            plan: Some("PRO".into()),
            short_window: short.map(|remaining_percent| models::UsageWindow {
                remaining_percent,
                resets_at: None,
                window_seconds: 18_000,
            }),
            weekly_window: weekly.map(|remaining_percent| models::UsageWindow {
                remaining_percent,
                resets_at: None,
                window_seconds: 604_800,
            }),
            reset_credits: None,
            reset_credit_expires_at: Vec::new(),
            updated_at: "2026-07-14T00:00:00Z".into(),
            status: status.into(),
            message: None,
            api_usage: None,
        }
    }

    fn api_snapshot(status: ApiUsageStatus, cost_usd: Option<f64>) -> ProviderSnapshot {
        ProviderSnapshot {
            provider: "codex".into(),
            display_name: "CODEX".into(),
            auth_mode: AuthMode::ApiKey,
            plan: Some("API".into()),
            short_window: None,
            weekly_window: None,
            reset_credits: None,
            reset_credit_expires_at: Vec::new(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            status: if status == ApiUsageStatus::Unavailable {
                "unavailable"
            } else {
                "ok"
            }
            .into(),
            message: None,
            api_usage: Some(models::ApiUsageSummary {
                status,
                today: models::ApiUsagePeriod::default(),
                month_to_date: models::ApiUsagePeriod {
                    cost_usd,
                    ..models::ApiUsagePeriod::default()
                },
                updated_at: chrono::Utc::now().to_rfc3339(),
                message: None,
            }),
        }
    }

    #[test]
    fn transient_failure_keeps_last_values_as_stale() {
        let previous = snapshot("ok", Some(74.0), Some(42.0));
        let mut failure = snapshot("unavailable", None, None);
        failure.message = Some("Network unavailable".into());
        let result = merge_snapshots(&[previous], vec![failure]);
        assert_eq!(result[0].status, "stale");
        assert_eq!(
            result[0].short_window.as_ref().unwrap().remaining_percent,
            74.0
        );
        assert_eq!(result[0].message.as_deref(), Some("Network unavailable"));
    }

    #[test]
    fn transient_failure_keeps_weekly_only_values_as_stale() {
        let previous = snapshot("ok", None, Some(42.0));
        let mut failure = snapshot("unavailable", None, None);
        failure.message = Some("Network unavailable".into());
        let result = merge_snapshots(&[previous], vec![failure]);
        assert_eq!(result[0].status, "stale");
        assert_eq!(
            result[0].weekly_window.as_ref().unwrap().remaining_percent,
            42.0
        );
    }

    #[test]
    fn signed_out_replaces_last_values() {
        let previous = snapshot("ok", Some(74.0), Some(42.0));
        let result = merge_snapshots(&[previous], vec![snapshot("signed_out", None, None)]);
        assert_eq!(result[0].status, "signed_out");
        assert!(result[0].short_window.is_none());
    }

    #[test]
    fn transient_admin_failure_keeps_known_api_totals_as_stale() {
        let previous = api_snapshot(ApiUsageStatus::Ok, Some(12.34));
        let mut failure = api_snapshot(ApiUsageStatus::Unavailable, None);
        failure.message = Some("Organization usage unavailable".into());
        failure.api_usage.as_mut().unwrap().message = failure.message.clone();

        let result = merge_snapshots(&[previous], vec![failure]);

        assert_eq!(result[0].status, "stale");
        assert_eq!(
            result[0].api_usage.as_ref().unwrap().status,
            ApiUsageStatus::Stale
        );
        assert_eq!(
            result[0].api_usage.as_ref().unwrap().month_to_date.cost_usd,
            Some(12.34)
        );
    }

    #[test]
    fn partial_admin_results_use_failure_backoff_without_hiding_real_values() {
        let partial = api_snapshot(ApiUsageStatus::Partial, Some(4.2));
        assert_eq!(partial.status, "ok");
        assert!(!snapshot_refresh_succeeded(&partial));
        assert!(snapshot_refresh_succeeded(&api_snapshot(
            ApiUsageStatus::NotConfigured,
            None,
        )));
        assert!(snapshot_refresh_succeeded(&snapshot(
            "ok",
            Some(74.0),
            Some(42.0),
        )));
    }

    #[test]
    fn failure_backoff_is_bounded() {
        assert_eq!(failure_backoff(1), Duration::from_secs(30));
        assert_eq!(failure_backoff(2), Duration::from_secs(60));
        assert_eq!(failure_backoff(99), MAX_FAILURE_BACKOFF);
    }

    #[test]
    fn failure_retry_does_not_cross_stale_value_expiration() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-07-14T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let mut stale = snapshot("stale", Some(74.0), Some(42.0));
        stale.updated_at = "2026-07-14T11:31:00Z".into();
        assert_eq!(
            next_refresh_delay(false, 99, &[stale.clone()], now, Duration::from_secs(10),),
            Duration::from_secs(60) + Duration::from_millis(1)
        );
        assert_eq!(
            next_refresh_delay(false, 1, &[stale], now, Duration::from_secs(10)),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn configured_interval_controls_successful_refreshes() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-07-14T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let regular = snapshot("ok", Some(74.0), Some(42.0));

        assert_eq!(
            next_refresh_delay(true, 0, &[regular], now, Duration::from_secs(10),),
            Duration::from_secs(10)
        );
    }

    #[test]
    fn near_reset_only_accelerates_slower_intervals() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-07-14T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let mut near_reset = snapshot("ok", Some(74.0), Some(42.0));
        near_reset.short_window.as_mut().unwrap().resets_at = Some("2026-07-14T12:10:00Z".into());

        assert_eq!(
            next_refresh_delay(
                true,
                0,
                &[near_reset.clone()],
                now,
                Duration::from_secs(300),
            ),
            Duration::from_secs(60)
        );
        assert_eq!(
            next_refresh_delay(true, 0, &[near_reset], now, Duration::from_secs(10),),
            Duration::from_secs(10)
        );
    }

    #[test]
    fn manual_only_mode_skips_activation_refreshes_but_allows_manual_refresh() {
        let now = Instant::now();
        let runtime = QuotaRuntime {
            snapshots: vec![snapshot("ok", Some(74.0), Some(42.0))],
            ..QuotaRuntime::default()
        };

        assert!(should_skip_refresh(
            &runtime,
            RefreshTrigger::Activation,
            now,
            false,
        ));
        assert!(!should_skip_refresh(
            &runtime,
            RefreshTrigger::Manual,
            now,
            false,
        ));
    }

    #[test]
    fn manual_only_mode_has_no_public_next_refresh() {
        let now_wall = chrono::Utc::now();
        let now_instant = Instant::now();
        let mut runtime = QuotaRuntime {
            snapshots: vec![snapshot("ok", Some(74.0), Some(42.0))],
            ..QuotaRuntime::default()
        };

        set_runtime_schedule(&mut runtime, now_instant, now_wall, true, None);

        assert!(runtime.next_refresh_instant.is_none());
        assert!(runtime.next_refresh_at.is_none());
    }

    #[test]
    fn menu_bar_summary_is_compact_and_marks_stale_data() {
        let mut recent = snapshot("stale", Some(74.2), Some(41.7));
        recent.updated_at = chrono::Utc::now().to_rfc3339();
        let state = SnapshotState {
            snapshots: vec![recent],
            refreshing: false,
            revision: 1,
            last_attempt_at: None,
            last_success_at: None,
            next_refresh_at: None,
        };
        assert_eq!(tray_summary(&state), "5h 74% · W 42% ⚠︎");
    }

    #[test]
    fn menu_bar_adapts_to_the_windows_returned_by_the_service() {
        let weekly_only = SnapshotState {
            snapshots: vec![snapshot("ok", None, Some(41.7))],
            refreshing: false,
            revision: 1,
            last_attempt_at: None,
            last_success_at: None,
            next_refresh_at: None,
        };
        assert_eq!(tray_summary(&weekly_only), "W 42%");

        let short_only = SnapshotState {
            snapshots: vec![snapshot("ok", Some(74.2), None)],
            ..weekly_only
        };
        assert_eq!(tray_summary(&short_only), "5h 74%");
    }

    #[test]
    fn menu_bar_reports_api_cost_without_calling_it_balance() {
        let state = SnapshotState {
            snapshots: vec![api_snapshot(ApiUsageStatus::Ok, Some(12.345))],
            refreshing: false,
            revision: 1,
            last_attempt_at: None,
            last_success_at: None,
            next_refresh_at: None,
        };
        assert_eq!(tray_summary(&state), "API $12.35");
        assert!(!tray_tooltip(&state)
            .to_ascii_lowercase()
            .contains("balance"));

        let basic = SnapshotState {
            snapshots: vec![api_snapshot(ApiUsageStatus::NotConfigured, None)],
            ..state
        };
        assert_eq!(tray_summary(&basic), "API");
        assert!(tray_tooltip(&basic).contains("organization totals not configured"));
    }

    #[test]
    fn menu_bar_title_exposes_a_refresh_action_and_loading_feedback() {
        let ready = SnapshotState {
            snapshots: vec![snapshot("ok", None, Some(41.7))],
            refreshing: false,
            revision: 1,
            last_attempt_at: None,
            last_success_at: None,
            next_refresh_at: None,
        };
        assert_eq!(tray_title(&ready), "W 42%  ↻");
        assert_eq!(
            tray_title(&SnapshotState {
                refreshing: true,
                ..ready
            }),
            "W 42%  …"
        );
    }

    #[test]
    fn menu_bar_refresh_hit_area_only_uses_the_trailing_control() {
        assert!(!refresh_hit_test(130.0, 100.0, 100.0, 24.0));
        assert!(!refresh_hit_test(177.0, 100.0, 100.0, 24.0));
        assert!(refresh_hit_test(179.0, 100.0, 100.0, 24.0));
        assert!(refresh_hit_test(199.0, 100.0, 100.0, 24.0));
        assert!(!refresh_hit_test(201.0, 100.0, 100.0, 24.0));
    }

    #[test]
    fn menu_bar_hides_expired_stale_values() {
        let mut expired = snapshot("stale", Some(74.2), Some(41.7));
        expired.updated_at = (chrono::Utc::now() - chrono::Duration::minutes(31)).to_rfc3339();
        let state = SnapshotState {
            snapshots: vec![expired],
            refreshing: false,
            revision: 1,
            last_attempt_at: None,
            last_success_at: None,
            next_refresh_at: None,
        };
        assert_eq!(tray_summary(&state), "5h — · W —");
        assert!(tray_tooltip(&state).contains("expired"));
    }

    #[test]
    fn panel_stays_inside_negative_origin_work_area() {
        let position = panel_position(
            PhysicalPosition::new(-30.0, 0.0),
            PhysicalSize::new(384, 668),
            PhysicalRect {
                position: PhysicalPosition::new(-1280, 24),
                size: PhysicalSize::new(1280, 960),
            },
        );
        assert_eq!(position, PhysicalPosition::new(-392, 32));
    }

    #[test]
    fn panel_uses_target_monitor_scale_for_mixed_dpi_positioning() {
        assert_eq!(panel_physical_size(1.0), PhysicalSize::new(384, 668));
        assert_eq!(panel_physical_size(2.0), PhysicalSize::new(768, 1336));
        let position = panel_position(
            PhysicalPosition::new(3_000.0, 24.0),
            panel_physical_size(2.0),
            PhysicalRect {
                position: PhysicalPosition::new(1920, 48),
                size: PhysicalSize::new(3840, 2112),
            },
        );
        assert_eq!(position, PhysicalPosition::new(2616, 56));
    }

    #[test]
    fn legacy_config_migration_preserves_new_files_and_copies_missing_indexes() {
        let base = std::env::temp_dir().join(format!(
            "codex-halo-config-migration-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let legacy = base.join(LEGACY_CONFIG_IDENTIFIER);
        let current = base.join("app.codexhalo.desktop");
        fs::create_dir_all(&legacy).unwrap();
        fs::create_dir_all(&current).unwrap();
        fs::write(legacy.join("preferences.json"), b"legacy preferences").unwrap();
        fs::write(legacy.join("usage-index-v1.json"), b"legacy index").unwrap();
        fs::write(legacy.join(".window-state.json"), b"legacy window state").unwrap();
        fs::write(current.join("preferences.json"), b"current preferences").unwrap();

        migrate_legacy_app_data(&current);

        assert_eq!(
            fs::read(current.join("preferences.json")).unwrap(),
            b"current preferences"
        );
        assert_eq!(
            fs::read(current.join("usage-index-v1.json")).unwrap(),
            b"legacy index"
        );
        assert_eq!(
            fs::read(current.join(".window-state.json")).unwrap(),
            b"legacy window state"
        );
        let _ = fs::remove_dir_all(base);
    }
}
