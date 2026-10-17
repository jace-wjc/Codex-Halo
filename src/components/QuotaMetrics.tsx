import {
  ArrowClockwise,
  ClockCountdown,
  ClockCounterClockwise,
  CloudSlash,
  Key,
  SignIn,
  SpinnerGap,
  WarningCircle,
  X,
} from "@phosphor-icons/react";
import { memo, useMemo, useState } from "react";
import {
  formatDateTime,
  formatResetTime,
  formatUpdatedTime,
  isStaleExpired,
  normalizePercent,
  overallQuotaTier,
  resetCreditExpiryState,
  type ResetCreditExpiryState,
} from "../lib/format";
import { copy, normalizeLanguage } from "../lib/i18n";
import type { Language, ProviderSnapshot, SnapshotStatus } from "../types";

interface QuotaMetricsProps {
  snapshot: ProviderSnapshot;
  language: Language;
  refreshing?: boolean;
  compact?: boolean;
  onRefresh?: () => void;
}

interface ResetCreditItem {
  value: string | null;
  state: ResetCreditExpiryState;
}

function backendMessage(message: string | null, language: Language): string | null {
  if (!message) return null;
  if (language === "en") return message;
  const normalized = message.toLowerCase();
  if (normalized.includes("login") && normalized.includes("temporarily unavailable")) return "Codex 登录文件暂时无法读取，应用将自动重试。";
  if (normalized.includes("format")) return "额度或登录响应格式发生变化，已停止展示不可靠数据。";
  if (normalized.includes("sign in") || normalized.includes("login")) return "Codex 登录状态不可用，请重新登录。";
  if (normalized.includes("rate limit")) return "请求过于频繁，应用将稍后重试。";
  if (normalized.includes("network")) return "当前网络不可用，应用将自动重试。";
  if (normalized.includes("supported usage window")) return "额度响应未包含可识别的额度窗口，已停止展示不可靠数据。";
  if (normalized.includes("already running")) return "额度正在刷新，请稍候。";
  return message;
}

function StateIcon({ status, expired }: { status: SnapshotStatus; expired: boolean }) {
  if (status === "loading") return <SpinnerGap className="is-spinning" />;
  if (status === "signed_out") return <SignIn />;
  if (status === "stale" || expired) return <ClockCounterClockwise />;
  if (status === "unavailable") return <CloudSlash />;
  return <WarningCircle />;
}

function MetricTile({
  label,
  percent,
  reset,
  unavailableLabel,
}: {
  label: string;
  percent: number | null;
  reset: string;
  unavailableLabel: string;
}) {
  return (
    <article className={`metric-tile${percent === null ? " metric-tile--empty" : ""}`}>
      <p>{label}</p>
      <strong>{percent === null ? "—" : percent}<small>{percent === null ? "" : "%"}</small></strong>
      <div
        className="metric-track"
        role={percent === null ? undefined : "progressbar"}
        aria-label={label}
        aria-valuemin={percent === null ? undefined : 0}
        aria-valuemax={percent === null ? undefined : 100}
        aria-valuenow={percent ?? undefined}
      >
        {percent === null ? null : <span style={{ width: `${percent}%` }} />}
      </div>
      <small className="metric-reset">{percent === null ? unavailableLabel : reset}</small>
    </article>
  );
}

function formatApiCount(value: number | null | undefined, language: Language): string {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) return "—";
  return new Intl.NumberFormat(language === "en" ? "en-US" : "zh-CN", {
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(value);
}

function formatApiCost(value: number | null | undefined): string {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) return "—";
  const maximumFractionDigits = value > 0 && value < 0.01 ? 4 : 2;
  return new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 2,
    maximumFractionDigits,
  }).format(value);
}

function ApiUsageMetrics({
  snapshot,
  language,
  refreshing,
  compact,
  onRefresh,
}: Required<Pick<QuotaMetricsProps, "snapshot" | "language" | "refreshing" | "compact">> & Pick<QuotaMetricsProps, "onRefresh">) {
  const t = copy[language];
  const usage = snapshot.apiUsage ?? null;
  const expired = isStaleExpired(snapshot);
  const periods = usage ? [usage.today, usage.monthToDate] : [];
  const hasValues = periods.some((period) => [
    period.inputTokens,
    period.cachedInputTokens,
    period.outputTokens,
    period.requests,
    period.costUsd,
  ].some((value) => typeof value === "number" && Number.isFinite(value)));

  if (!usage || usage.status === "not_configured") {
    return (
      <section className={`api-mode-card${compact ? " api-mode-card--compact" : ""}`} aria-live="polite">
        <div className="api-mode-card__heading">
          <span aria-hidden="true"><Key /></span>
          <div><strong>{t.apiAccountTitle}</strong><small>{t.apiOrganizationScope}</small></div>
        </div>
        <p>{t.apiQuotaNotApplicable}</p>
        <p className="api-mode-card__admin">{t.apiAdminHint}</p>
        <small className="api-mode-card__local">{t.apiLocalStatsAvailable}</small>
      </section>
    );
  }

  if (expired || (usage.status === "unavailable" && !hasValues)) {
    return (
      <section className="quota-empty quota-empty--unavailable" aria-live="polite" aria-busy={refreshing}>
        <div className="quota-empty__icon" aria-hidden="true"><Key /></div>
        <strong>{t.apiUsageUnavailable}</strong>
        <p>{t.apiUsageUnavailableHint}</p>
        {onRefresh ? (
          <button type="button" className="secondary-button" onClick={onRefresh} disabled={refreshing}>
            {refreshing ? <SpinnerGap className="is-spinning" /> : <ArrowClockwise />}
            <span>{refreshing ? t.refreshing : t.refresh}</span>
          </button>
        ) : null}
      </section>
    );
  }

  const month = usage.monthToDate;
  return (
    <section className={`api-usage-metrics${compact ? " api-usage-metrics--compact" : ""}`} aria-live="polite" aria-busy={refreshing}>
      {snapshot.status === "stale" || usage.status === "stale" || usage.status === "partial" ? (
        <div className={`data-banner${snapshot.status === "stale" || usage.status === "stale" ? " data-banner--stale" : ""}`} role="status">
          <ClockCounterClockwise aria-hidden="true" />
          <span>{snapshot.status === "stale" || usage.status === "stale" ? t.dataStale : t.apiPartialData}</span>
          <small>{t.lastUpdated(formatUpdatedTime(usage.updatedAt, new Date(), language))}</small>
        </div>
      ) : null}
      <div className="api-usage-heading">
        <span aria-hidden="true"><Key /></span>
        <div><strong>{t.apiAccountTitle}</strong><small>{t.apiOrganizationScope}</small></div>
      </div>
      <div className="api-metric-grid">
        <article><span>{t.apiMonthCost}</span><strong>{formatApiCost(month.costUsd)}</strong></article>
        <article><span>{t.apiTodayCost}</span><strong>{formatApiCost(usage.today.costUsd)}</strong></article>
        <article>
          <span>{t.apiMonthInput}</span>
          <strong>{formatApiCount(month.inputTokens, language)}</strong>
          <small>{t.apiCachedTokens(formatApiCount(month.cachedInputTokens, language))}</small>
        </article>
        <article>
          <span>{t.apiMonthOutput}</span>
          <strong>{formatApiCount(month.outputTokens, language)}</strong>
          <small>{t.apiRequests(formatApiCount(month.requests, language))}</small>
        </article>
      </div>
      <p className="api-usage-boundary">{t.apiCostNotBalance}</p>
    </section>
  );
}

export const QuotaMetrics = memo(function QuotaMetrics({
  snapshot,
  language,
  refreshing = false,
  compact = false,
  onRefresh,
}: QuotaMetricsProps) {
  const activeLanguage = normalizeLanguage(language);
  const t = copy[activeLanguage];
  const [showCredits, setShowCredits] = useState(false);
  const short = normalizePercent(snapshot.shortWindow?.remainingPercent);
  const weekly = normalizePercent(snapshot.weeklyWindow?.remainingPercent);
  const expired = isStaleExpired(snapshot);
  const hasValues = short !== null || weekly !== null;
  const canShowValues = snapshot.status !== "signed_out" && !expired && hasValues;
  const tier = overallQuotaTier(snapshot);
  const message = backendMessage(snapshot.message, activeLanguage);
  const creditItems = useMemo<ResetCreditItem[]>(() => {
    const count = snapshot.resetCredits === null ? 0 : Math.min(50, Math.max(0, Math.floor(snapshot.resetCredits)));
    if (count === 0) return [];
    const now = new Date();
    const items: ResetCreditItem[] = (snapshot.resetCreditExpiresAt ?? [])
      .map((value) => ({ value, state: resetCreditExpiryState(value, now) }))
      .sort((left, right) => (left.state.expiresAt ?? Number.POSITIVE_INFINITY) - (right.state.expiresAt ?? Number.POSITIVE_INFINITY))
      .slice(0, count);
    while (items.length < count) {
      items.push({ value: null, state: resetCreditExpiryState(null, now) });
    }
    return items;
  }, [snapshot.resetCreditExpiresAt, snapshot.resetCredits]);

  const expiryLabel = (state: ResetCreditExpiryState): string => {
    if (state.urgency === "expired") return t.creditExpired;
    if (state.daysRemaining === null) return t.creditExpiresUnknown;
    return t.creditCountdown(state.daysRemaining);
  };
  const earliestCredit = creditItems[0] ?? null;
  const creditHint = earliestCredit?.value && earliestCredit.state.expiresAt !== null
    ? `${t.creditEarliestExpiry(formatDateTime(earliestCredit.value, activeLanguage))} · ${expiryLabel(earliestCredit.state)}`
    : snapshot.resetCredits !== null && snapshot.resetCredits > 0
      ? t.noCreditExpiration
      : null;
  const creditUrgency = earliestCredit?.state.urgency ?? "unknown";

  if (snapshot.authMode === "api_key") {
    return (
      <ApiUsageMetrics
        snapshot={snapshot}
        language={activeLanguage}
        refreshing={refreshing}
        compact={compact}
        onRefresh={onRefresh}
      />
    );
  }

  if (!canShowValues) {
    const isLoading = snapshot.status === "loading";
    const title = isLoading
      ? t.loadingQuota
      : snapshot.status === "signed_out"
        ? t.signedInRequired
        : expired
          ? t.staleExpired
          : t.temporarilyUnavailable;
    const hint = isLoading
      ? t.loadingHint
      : snapshot.status === "signed_out"
        ? message ?? t.signedOutHint
        : expired
          ? t.staleHint
          : message ?? t.errorUnavailable;
    return (
      <section className={`quota-empty quota-empty--${snapshot.status}`} aria-live="polite" aria-busy={isLoading || refreshing}>
        <div className="quota-empty__icon" aria-hidden="true"><StateIcon status={snapshot.status} expired={expired} /></div>
        <strong>{title}</strong>
        <p>{hint}</p>
        {!isLoading && onRefresh ? (
          <button type="button" className="secondary-button" onClick={onRefresh} disabled={refreshing}>
            {refreshing ? <SpinnerGap className="is-spinning" /> : <ArrowClockwise />}
            <span>{refreshing ? t.refreshing : t.refresh}</span>
          </button>
        ) : null}
      </section>
    );
  }

  const partial = snapshot.status === "unavailable";
  return (
    <section className={`quota-metrics quota-metrics--${tier}${compact ? " quota-metrics--compact" : ""}`} aria-live="polite" aria-busy={refreshing}>
      {snapshot.status === "stale" || partial ? (
        <div className={`data-banner${snapshot.status === "stale" ? " data-banner--stale" : ""}`} role="status">
          <ClockCounterClockwise aria-hidden="true" />
          <span>{snapshot.status === "stale" ? t.dataStale : t.partialData}</span>
          <small>{t.lastUpdated(formatUpdatedTime(snapshot.updatedAt, new Date(), activeLanguage))}</small>
        </div>
      ) : null}
      <div className="metric-grid">
        {short !== null ? (
          <MetricTile
            label={t.fiveHour}
            percent={short}
            reset={formatResetTime(snapshot.shortWindow?.resetsAt ?? null, new Date(), activeLanguage)}
            unavailableLabel={t.temporarilyUnavailable}
          />
        ) : null}
        {weekly !== null ? (
          <MetricTile
            label={t.weeklyRemaining}
            percent={weekly}
            reset={formatResetTime(snapshot.weeklyWindow?.resetsAt ?? null, new Date(), activeLanguage)}
            unavailableLabel={t.temporarilyUnavailable}
          />
        ) : null}
      </div>
      <div className={`credit-summary credit-summary--${creditUrgency}`}>
        <div className="credit-summary__icon" aria-hidden="true"><ClockCountdown /></div>
        <div className="credit-summary__copy">
          <span>{t.resetCreditsTitle}</span>
          <strong>
            {snapshot.resetCredits === null
              ? t.resetCreditUnknown
              : snapshot.resetCredits === 0
                ? t.noResetCredits
                : t.resetCredits(snapshot.resetCredits)}
          </strong>
          {creditHint ? <small title={creditHint}>{creditHint}</small> : null}
        </div>
        {snapshot.resetCredits !== null && snapshot.resetCredits > 0 ? (
          <button
            type="button"
            className="text-button"
            onClick={() => setShowCredits((value) => !value)}
            aria-expanded={showCredits}
            aria-controls="credit-expiry-details"
          >
            {showCredits ? t.hide : t.view}
          </button>
        ) : null}
      </div>
      {showCredits && snapshot.resetCredits !== null && snapshot.resetCredits > 0 ? (
        <div id="credit-expiry-details" className="credit-popover" role="dialog" aria-label={t.creditExpiryDetails}>
          <header className="credit-popover__header">
            <strong>{t.creditExpiryDetails}</strong>
            <button type="button" onClick={() => setShowCredits(false)} aria-label={t.hide}><X /></button>
          </header>
          <div className="credit-list">
            {creditItems.map((item, index) => (
              <div className={`credit-item credit-item--${item.state.urgency}`} key={`${item.value ?? "unknown"}-${index}`}>
                <div>
                  <strong>{t.creditName(index)}</strong>
                  <small>{item.value ? t.creditUseBefore(formatDateTime(item.value, activeLanguage)) : t.creditExpiresUnknown}</small>
                </div>
                <span>{expiryLabel(item.state)}</span>
              </div>
            ))}
          </div>
        </div>
      ) : null}
    </section>
  );
});
