import {
  ArrowClockwise,
  CaretRight,
  CursorClick,
  Eye,
  EyeSlash,
  Power,
  PushPin,
  RocketLaunch,
  SpinnerGap,
  Timer,
  Translate,
  WarningCircle,
} from "@phosphor-icons/react";
import { memo, useEffect, useState, type ReactNode } from "react";
import { overallQuotaTier } from "../lib/format";
import { copy, normalizeLanguage } from "../lib/i18n";
import type { Language, ProviderSnapshot, UsageStats, WidgetPreferences } from "../types";
import { ProviderMark } from "./ProviderMark";
import { QuotaMetrics } from "./QuotaMetrics";
import { UsageStatsPanel } from "./UsageStatsPanel";

interface MenuPanelProps {
  snapshot: ProviderSnapshot;
  preferences: WidgetPreferences;
  widgetVisible: boolean;
  autostartEnabled: boolean;
  refreshing: boolean;
  usageStats?: UsageStats | null;
  usageLoading?: boolean;
  usageFailed?: boolean;
  initialSection?: "quota" | "stats";
  pendingAction?: string | null;
  nextRefreshAt?: string | null;
  notice?: ReactNode;
  onRefresh: () => void;
  onLoadUsageStats?: () => void;
  onToggleWidget: () => void;
  onToggleAlwaysOnTop: () => void;
  onToggleAutostart: () => void;
  onToggleLanguage: () => void;
  onSetRefreshInterval?: (seconds: number | null) => void;
  onToggleClickThrough: () => void;
  onQuit: () => void;
}

type RefreshUnit = "seconds" | "minutes" | "hours";

const REFRESH_PRESETS = [10, 30, 60, 300, 600, 1800] as const;
const REFRESH_UNIT_SECONDS: Record<RefreshUnit, number> = {
  seconds: 1,
  minutes: 60,
  hours: 3600,
};

function formatRefreshInterval(seconds: number | null, language: Language): string {
  if (seconds === null) return copy[language].manualRefreshOnly;
  const [value, unit] = seconds % 3600 === 0
    ? [seconds / 3600, language === "en" ? (seconds === 3600 ? "hour" : "hours") : "小时"]
    : seconds % 60 === 0
      ? [seconds / 60, language === "en" ? (seconds === 60 ? "minute" : "minutes") : "分钟"]
      : [seconds, language === "en" ? (seconds === 1 ? "second" : "seconds") : "秒"];
  return language === "en" ? `Every ${value} ${unit}` : `每 ${value} ${unit}`;
}

function customIntervalParts(seconds: number | null): { value: string; unit: RefreshUnit } {
  if (seconds && seconds % 3600 === 0) return { value: String(seconds / 3600), unit: "hours" };
  if (seconds && seconds % 60 === 0) return { value: String(seconds / 60), unit: "minutes" };
  if (seconds) return { value: String(seconds), unit: "seconds" };
  return { value: "5", unit: "minutes" };
}

function RefreshIntervalSetting({
  seconds,
  nextRefreshAt,
  language,
  apiMode,
  pending,
  onChange,
}: {
  seconds: number | null;
  nextRefreshAt: string | null;
  language: Language;
  apiMode: boolean;
  pending: boolean;
  onChange: (seconds: number | null) => void;
}) {
  const t = copy[language];
  const [open, setOpen] = useState(false);
  const [choice, setChoice] = useState<string>(seconds === null ? "manual" : String(seconds));
  const initialCustom = customIntervalParts(seconds);
  const [customValue, setCustomValue] = useState(initialCustom.value);
  const [customUnit, setCustomUnit] = useState<RefreshUnit>(initialCustom.unit);

  useEffect(() => {
    if (!open) return undefined;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [open]);

  const openDialog = () => {
    const isPreset = seconds !== null && REFRESH_PRESETS.includes(seconds as typeof REFRESH_PRESETS[number]);
    const custom = customIntervalParts(seconds);
    setChoice(seconds === null ? "manual" : isPreset ? String(seconds) : "custom");
    setCustomValue(custom.value);
    setCustomUnit(custom.unit);
    setOpen(true);
  };

  const numericCustomValue = Number(customValue);
  const customSeconds = Number.isSafeInteger(numericCustomValue) && numericCustomValue > 0
    ? numericCustomValue * REFRESH_UNIT_SECONDS[customUnit]
    : Number.NaN;
  const draftSeconds = choice === "manual"
    ? null
    : choice === "custom"
      ? customSeconds
      : Number(choice);
  const valid = draftSeconds === null
    || (Number.isSafeInteger(draftSeconds) && draftSeconds >= 10 && draftSeconds <= 86_400);
  const nextDate = nextRefreshAt ? new Date(nextRefreshAt) : null;
  const nextLabel = nextDate && !Number.isNaN(nextDate.getTime())
    ? t.nextRefreshAt(new Intl.DateTimeFormat(language, {
      hour: "2-digit",
      minute: "2-digit",
      hour12: false,
    }).format(nextDate))
    : null;
  const intervalLabel = formatRefreshInterval(seconds, language);
  const settingLabel = apiMode ? t.apiAutoRefresh : t.quotaAutoRefresh;
  const settingHint = apiMode ? t.apiRefreshIntervalHint : t.refreshIntervalHint;

  return (
    <>
      <button type="button" className="setting-row" onClick={openDialog} disabled={pending}>
        <span className="setting-row__icon" aria-hidden="true"><Timer /></span>
        <span className="setting-row__copy">
          <strong>{settingLabel}</strong>
          <small>{seconds === null || !nextLabel ? intervalLabel : `${intervalLabel} · ${nextLabel}`}</small>
        </span>
        {pending ? <SpinnerGap className="is-spinning setting-row__pending" /> : <CaretRight className="setting-row__chevron" aria-hidden="true" />}
      </button>

      {open ? (
        <div
          className="refresh-interval-backdrop"
          onMouseDown={(event) => { if (event.target === event.currentTarget) setOpen(false); }}
        >
          <form
            className="refresh-interval-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="refresh-interval-title"
            onSubmit={(event) => {
              event.preventDefault();
              if (!valid) return;
              if (draftSeconds !== seconds) onChange(draftSeconds);
              setOpen(false);
            }}
          >
            <div className="refresh-interval-dialog__heading">
              <span className="setting-row__icon" aria-hidden="true"><Timer /></span>
              <div>
                <strong id="refresh-interval-title">{t.refreshIntervalTitle}</strong>
                <p>{settingHint}</p>
              </div>
            </div>

            <div className="refresh-preset-grid">
              {REFRESH_PRESETS.map((preset) => (
                <button
                  key={preset}
                  type="button"
                  className={choice === String(preset) ? "is-active" : ""}
                  aria-pressed={choice === String(preset)}
                  onClick={() => setChoice(String(preset))}
                >
                  <span>{formatRefreshInterval(preset, language)}</span>
                  {preset === 300 ? <small>{t.recommended}</small> : null}
                </button>
              ))}
              <button
                type="button"
                className={choice === "manual" ? "is-active" : ""}
                aria-pressed={choice === "manual"}
                onClick={() => setChoice("manual")}
              >
                <span>{t.manualRefreshOnly}</span>
              </button>
              <button
                type="button"
                className={choice === "custom" ? "is-active" : ""}
                aria-pressed={choice === "custom"}
                onClick={() => setChoice("custom")}
              >
                <span>{t.refreshCustom}</span>
              </button>
            </div>

            {choice === "custom" ? (
              <div className="refresh-custom-fields">
                <input
                  type="number"
                  min="1"
                  step="1"
                  value={customValue}
                  aria-label={t.refreshCustom}
                  autoFocus
                  onChange={(event) => setCustomValue(event.target.value)}
                />
                <select
                  value={customUnit}
                  aria-label={t.refreshIntervalTitle}
                  onChange={(event) => setCustomUnit(event.target.value as RefreshUnit)}
                >
                  <option value="seconds">{t.secondsUnit}</option>
                  <option value="minutes">{t.minutesUnit}</option>
                  <option value="hours">{t.hoursUnit}</option>
                </select>
              </div>
            ) : null}

            {!valid ? <p className="refresh-interval-message refresh-interval-message--error">{t.refreshIntervalInvalid}</p> : null}
            {valid && draftSeconds === 10 ? (
              <p className="refresh-interval-message refresh-interval-message--warning"><WarningCircle />{t.refreshHighFrequencyWarning}</p>
            ) : null}
            {valid && draftSeconds === null ? <p className="refresh-interval-message">{apiMode ? t.apiRefreshIntervalManualHint : t.refreshIntervalManualHint}</p> : null}
            {valid && draftSeconds !== null && draftSeconds !== 10 ? <p className="refresh-interval-message">{t.refreshIntervalScheduleHint}</p> : null}

            <div className="refresh-interval-actions">
              <button type="button" onClick={() => setOpen(false)}>{t.cancel}</button>
              <button type="submit" className="is-primary" disabled={!valid}>{t.save}</button>
            </div>
          </form>
        </div>
      ) : null}
    </>
  );
}

function SettingSwitch({
  icon,
  label,
  hint,
  checked,
  pending,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  hint?: string;
  checked: boolean;
  pending: boolean;
  onClick: () => void;
}) {
  return (
    <button type="button" className="setting-row" role="switch" aria-checked={checked} onClick={onClick} disabled={pending}>
      <span className="setting-row__icon" aria-hidden="true">{icon}</span>
      <span className="setting-row__copy"><strong>{label}</strong>{hint ? <small>{hint}</small> : null}</span>
      {pending ? <SpinnerGap className="is-spinning setting-row__pending" /> : <span className="switch-control" aria-hidden="true"><i /></span>}
    </button>
  );
}

export const MenuPanel = memo(function MenuPanel({
  snapshot,
  preferences,
  widgetVisible,
  autostartEnabled,
  refreshing,
  usageStats = null,
  usageLoading = false,
  usageFailed = false,
  initialSection = "quota",
  pendingAction = null,
  nextRefreshAt = null,
  notice = null,
  onRefresh,
  onLoadUsageStats = () => undefined,
  onToggleWidget,
  onToggleAlwaysOnTop,
  onToggleAutostart,
  onToggleLanguage,
  onSetRefreshInterval = () => undefined,
  onToggleClickThrough,
  onQuit,
}: MenuPanelProps) {
  const [section, setSection] = useState<"quota" | "stats">(initialSection);
  const language = normalizeLanguage(preferences.language);
  const t = copy[language];
  const tier = overallQuotaTier(snapshot);
  const apiMode = snapshot.authMode === "api_key";
  const statusLabel = apiMode
    ? snapshot.status === "unavailable"
      ? t.apiUsageUnavailable
      : snapshot.status === "stale"
        ? t.dataStale
        : t.apiUsageBased
    : snapshot.status === "loading"
    ? t.loadingQuota
    : snapshot.status === "signed_out"
      ? t.notSignedIn
      : snapshot.status === "unavailable"
        ? t.unavailableStatus
        : snapshot.status === "stale"
          ? t.dataStale
          : tier === "healthy"
            ? t.statusHealthy
            : tier === "caution"
              ? t.statusCaution
              : tier === "critical"
                ? t.statusCritical
                : t.statusUnknown;
  const actionLoading = refreshing || usageLoading;
  const handleSection = (next: "quota" | "stats") => {
    setSection(next);
    if (next === "stats") onLoadUsageStats();
  };

  return (
    <main className="menu-panel">
      <header className="panel-header">
        <div className="panel-brand">
          <ProviderMark />
          <div><strong>{t.appName}</strong><span>{section === "stats" ? t.usageStats : apiMode ? t.apiUsageBased : snapshot.plan ?? t.accountFallback}</span></div>
        </div>
        <div className="panel-header__actions">
          {section === "stats" ? (
            <span className="status-pill status-pill--local" title={apiMode ? t.localData : t.statsSources}><i aria-hidden="true" />{apiMode ? t.localData : t.statsSources}</span>
          ) : (
            <span className={`status-pill status-pill--${snapshot.status} status-pill--${tier}${apiMode ? " status-pill--api" : ""}`} title={statusLabel}>
              <i aria-hidden="true" />{statusLabel}
            </span>
          )}
          <button
            type="button"
            className="refresh-button"
            onClick={onRefresh}
            disabled={actionLoading}
            aria-label={apiMode ? t.apiRefreshAll : t.refreshAll}
          >
            {actionLoading ? <SpinnerGap className="is-spinning" /> : <ArrowClockwise />}
            <span>{actionLoading ? t.refreshing : t.refresh}</span>
          </button>
        </div>
      </header>

      <nav className="panel-section-tabs" aria-label={t.panelSections}>
        <button type="button" className={section === "quota" ? "is-active" : ""} aria-current={section === "quota" ? "page" : undefined} onClick={() => handleSection("quota")}>{apiMode ? t.apiPanelTitle : t.panelTitle}</button>
        <button type="button" className={section === "stats" ? "is-active" : ""} aria-current={section === "stats" ? "page" : undefined} onClick={() => handleSection("stats")}>{t.usageStats}</button>
      </nav>

      {section === "quota" ? (
        <>
          <QuotaMetrics snapshot={snapshot} language={language} refreshing={refreshing} onRefresh={onRefresh} />
          <section className="panel-settings" aria-label={t.settings}>
            <h2>{t.settings}</h2>
            <div className="settings-group">
              <SettingSwitch
                icon={widgetVisible ? <Eye /> : <EyeSlash />}
                label={widgetVisible ? t.hideWidget : t.openWidget}
                checked={widgetVisible}
                pending={pendingAction === "widget"}
                onClick={onToggleWidget}
              />
              <SettingSwitch
                icon={<PushPin />}
                label={t.alwaysOnTop}
                checked={preferences.alwaysOnTop}
                pending={pendingAction === "alwaysOnTop"}
                onClick={onToggleAlwaysOnTop}
              />
              <SettingSwitch
                icon={<RocketLaunch />}
                label={t.launchAtLogin}
                checked={autostartEnabled}
                pending={pendingAction === "autostart"}
                onClick={onToggleAutostart}
              />
              <RefreshIntervalSetting
                seconds={preferences.quotaRefreshIntervalSeconds}
                nextRefreshAt={nextRefreshAt}
                language={language}
                apiMode={apiMode}
                pending={pendingAction === "refreshInterval"}
                onChange={onSetRefreshInterval}
              />
              <SettingSwitch
                icon={<CursorClick />}
                label={t.clickThrough}
                hint={preferences.locked ? t.clickThroughHint : undefined}
                checked={preferences.locked}
                pending={pendingAction === "clickThrough"}
                onClick={onToggleClickThrough}
              />
              <button type="button" className="setting-row" onClick={onToggleLanguage} disabled={pendingAction === "language"}>
                <span className="setting-row__icon" aria-hidden="true"><Translate /></span>
                <span className="setting-row__copy"><strong>{t.language}</strong><small>{language === "en" ? "English" : "简体中文"}</small></span>
                {pendingAction === "language" ? <SpinnerGap className="is-spinning setting-row__pending" /> : <span className="setting-value">{language === "en" ? "中" : "EN"}</span>}
              </button>
            </div>
          </section>

          {notice ? <div className="panel-notice" role="status">{notice}</div> : null}
          <button type="button" className="quit-button" onClick={onQuit}><Power /><span>{t.quit}</span></button>
        </>
      ) : (
        <UsageStatsPanel stats={usageStats} loading={usageLoading} failed={usageFailed} language={language} />
      )}

      {section === "stats" && notice ? <div className="panel-notice" role="status">{notice}</div> : null}
    </main>
  );
});
