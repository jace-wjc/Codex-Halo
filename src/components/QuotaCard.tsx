import {
  ArrowClockwise,
  ArrowDown,
  ArrowUp,
  ClockCounterClockwise,
  CloudSlash,
  Key,
  PushPin,
  PushPinSlash,
  SignIn,
  SpinnerGap,
  WarningCircle,
} from "@phosphor-icons/react";
import { memo, type ReactNode } from "react";
import { isStaleExpired, normalizePercent, overallQuotaTier } from "../lib/format";
import { copy, normalizeLanguage } from "../lib/i18n";
import type { Language, ProviderSnapshot, WidgetPreferences } from "../types";
import { ProviderMark } from "./ProviderMark";
import { QuotaMetrics } from "./QuotaMetrics";

interface Props {
  snapshot: ProviderSnapshot;
  preferences: WidgetPreferences;
  providerCount: number;
  onPrevious?: () => void;
  onNext?: () => void;
  onTogglePin?: () => void;
  onLock?: () => void;
  onLanguage?: () => void;
  onDrag: () => void;
  onHover: (hovered: boolean) => void;
  onRefresh?: () => void;
  isRefreshing?: boolean;
  isConsuming?: boolean;
  notice?: ReactNode;
  initialShowCreditTip?: boolean;
}

function tierLabel(snapshot: ProviderSnapshot, language: Language): string {
  const t = copy[normalizeLanguage(language)];
  if (snapshot.authMode === "api_key") {
    if (snapshot.status === "unavailable") return t.apiUsageUnavailable;
    if (snapshot.status === "stale") return t.dataStale;
    return t.apiUsageBased;
  }
  if (snapshot.status === "loading") return t.loadingQuota;
  if (snapshot.status === "signed_out") return t.notSignedIn;
  if (snapshot.status === "unavailable") return t.unavailableStatus;
  if (snapshot.status === "stale") return t.dataStale;
  switch (overallQuotaTier(snapshot)) {
    case "healthy": return t.statusHealthy;
    case "caution": return t.statusCaution;
    case "critical": return t.statusCritical;
    default: return t.statusUnknown;
  }
}

function OrbStateIcon({ snapshot }: { snapshot: ProviderSnapshot }) {
  if (snapshot.authMode === "api_key") return <Key />;
  if (snapshot.status === "loading") return <SpinnerGap className="is-spinning" />;
  if (snapshot.status === "signed_out") return <SignIn />;
  if (snapshot.status === "stale") return <ClockCounterClockwise />;
  if (snapshot.status === "unavailable") return <CloudSlash />;
  return <WarningCircle />;
}

export const QuotaCard = memo(function QuotaCard({
  snapshot,
  preferences,
  providerCount,
  onPrevious,
  onNext,
  onLock,
  onLanguage,
  onDrag,
  onHover,
  onRefresh,
  isRefreshing = false,
  isConsuming = false,
  notice = null,
}: Props) {
  const language = normalizeLanguage(preferences.language);
  const t = copy[language];
  const tier = overallQuotaTier(snapshot);
  const statusLabel = isConsuming ? t.active : tierLabel(snapshot, language);

  return (
    <main
      className={`quota-card quota-card--${snapshot.status} quota-card--${tier}`}
      onMouseEnter={() => onHover(true)}
      onMouseLeave={() => onHover(false)}
    >
      <header className="card-header" onMouseDown={(event) => { if (event.button === 0) void onDrag(); }}>
        <div className="provider-heading">
          <ProviderMark />
          <div>
            <p>{snapshot.displayName}</p>
            <span>{snapshot.plan ?? t.accountFallback}</span>
          </div>
        </div>
        <nav className="card-actions" aria-label={t.controls} onMouseDown={(event) => event.stopPropagation()}>
          {providerCount > 1 && onPrevious ? <button type="button" onClick={onPrevious} aria-label={t.servicePrevious}><ArrowUp /></button> : null}
          {providerCount > 1 && onNext ? <button type="button" onClick={onNext} aria-label={t.serviceNext}><ArrowDown /></button> : null}
          <span className={`status-pill status-pill--${snapshot.status} status-pill--${tier}${snapshot.authMode === "api_key" ? " status-pill--api" : ""}${isConsuming ? " status-pill--active" : ""}`} title={statusLabel}>
            <i aria-hidden="true" />{statusLabel}
          </span>
          {onLanguage ? <button type="button" className="language-button" onClick={onLanguage} aria-label={t.switchLanguage}>{language === "en" ? "中" : "EN"}</button> : null}
          {onRefresh ? (
            <button type="button" onClick={onRefresh} disabled={isRefreshing} aria-label={t.refreshQuota} title={t.refreshQuota}>
              {isRefreshing ? <SpinnerGap className="is-spinning" /> : <ArrowClockwise />}
            </button>
          ) : null}
          {onLock ? (
            <button type="button" className={preferences.alwaysOnTop ? "is-active" : ""} onClick={onLock} aria-pressed={preferences.alwaysOnTop} aria-label={preferences.alwaysOnTop ? t.pinOff : t.pinOn}>
              {preferences.alwaysOnTop ? <PushPin weight="fill" /> : <PushPinSlash />}
            </button>
          ) : null}
        </nav>
      </header>

      <QuotaMetrics snapshot={snapshot} language={language} refreshing={isRefreshing} compact onRefresh={onRefresh} />
      {notice ? <div className="operation-notice" role="status">{notice}</div> : null}
    </main>
  );
});

export const QuotaOrb = memo(function QuotaOrb({
  snapshot,
  onDrag,
  onHover,
  language = "zh-CN",
}: Pick<Props, "snapshot" | "onDrag" | "onHover"> & { language?: Language }) {
  const activeLanguage = normalizeLanguage(language);
  const t = copy[activeLanguage];
  const short = normalizePercent(snapshot.shortWindow?.remainingPercent);
  const weekly = normalizePercent(snapshot.weeklyWindow?.remainingPercent);
  const percent = short ?? weekly;
  const weeklyFallback = short === null && weekly !== null;
  const expired = isStaleExpired(snapshot);
  const apiMode = snapshot.authMode === "api_key";
  const available = snapshot.status !== "signed_out" && snapshot.status !== "loading" && !expired && percent !== null;
  const tier = overallQuotaTier(snapshot);
  const circumference = 2 * Math.PI * 28;
  const dashOffset = available ? circumference * (1 - percent / 100) : circumference;
  const label = apiMode
    ? tierLabel(snapshot, activeLanguage)
    : available
    ? weeklyFallback ? `${t.weeklyRemaining} ${percent}%` : t.availableLabel(percent)
    : tierLabel(snapshot, activeLanguage);

  return (
    <main
      className={`quota-orb quota-card--${snapshot.status} quota-card--${tier}`}
      onMouseEnter={() => onHover(true)}
      onMouseLeave={() => onHover(false)}
      onMouseDown={(event) => { if (event.button === 0) void onDrag(); }}
      aria-label={label}
      title={label}
    >
      <svg className="orb-ring" viewBox="0 0 68 68" aria-hidden="true">
        <circle className="orb-ring__track" cx="34" cy="34" r="28" />
        {available ? <circle className="orb-ring__value" cx="34" cy="34" r="28" strokeDasharray={circumference} strokeDashoffset={dashOffset} /> : null}
      </svg>
      {apiMode ? (
        <section className="orb-api" aria-hidden="true"><Key /><span>API</span></section>
      ) : available ? (
        <section className="orb-metric" aria-hidden="true">
          {weeklyFallback ? <small className="orb-window-label">W</small> : null}
          <span>{percent}</span><small>%</small>
        </section>
      ) : (
        <section className="orb-unavailable" aria-hidden="true"><OrbStateIcon snapshot={snapshot} /></section>
      )}
      <span className={`orb-status orb-status--${snapshot.status} orb-status--${tier}${apiMode ? " orb-status--api" : ""}`} aria-hidden="true" />
    </main>
  );
});
