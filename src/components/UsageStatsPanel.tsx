import { ChartLineUp, ClockCounterClockwise, Database, LockKey, Stack } from "@phosphor-icons/react";
import { memo, useMemo, useState, type CSSProperties, type KeyboardEvent, type PointerEvent } from "react";
import { copy, normalizeLanguage } from "../lib/i18n";
import {
  MODEL_COLORS,
  aggregateModels,
  buildActivityMonthLabels,
  buildHeatmap,
  buildWeeklyActivity,
  buildOfficialWeeklyActivity,
  fillUsageRange,
  formatModelName,
  formatTokenCount,
  linePath,
  modelDayTokens,
  type ActivityMode,
  type UsageRange,
} from "../lib/usage";
import type { DailyTokenUsage, Language, OfficialUsage, UsageStats } from "../types";

interface UsageStatsPanelProps {
  stats: UsageStats | null;
  loading: boolean;
  failed: boolean;
  language: Language;
}

type StatsTab = "overview" | "models";

const ACTIVITY_WIDTH = 317;
const ACTIVITY_CELL_STEP = 6;
const CHART_WIDTH = 312;
const CHART_HEIGHT = 100;

function localDate(value: string): Date {
  const [year, month, day] = value.split("-").map(Number);
  return new Date(year, month - 1, day);
}

function formatDate(value: string, language: Language): string {
  return new Intl.DateTimeFormat(language, { year: "numeric", month: "short", day: "numeric" }).format(localDate(value));
}

function tooltipStyle(x: number): CSSProperties {
  return { "--tooltip-x": `${x}%` } as CSSProperties;
}

function StatsPlaceholder({
  kind,
  language,
}: {
  kind: "loading" | "empty" | "unavailable";
  language: Language;
}) {
  const t = copy[normalizeLanguage(language)];
  const content = kind === "loading"
    ? [t.statsLoading, t.statsLoadingHint]
    : kind === "empty"
      ? [t.statsEmpty, t.statsEmptyHint]
      : [t.statsUnavailable, t.statsUnavailableHint];
  return (
    <div className={`stats-placeholder stats-placeholder--${kind}`} role={kind === "loading" ? "status" : "alert"}>
      <span aria-hidden="true">{kind === "loading" ? <ClockCounterClockwise className="is-spinning" /> : <Database />}</span>
      <strong>{content[0]}</strong>
      <p>{content[1]}</p>
    </div>
  );
}

function Overview({ stats, official, language }: { stats: UsageStats; official?: OfficialUsage; language: Language }) {
  const t = copy[normalizeLanguage(language)];
  const [activityMode, setActivityMode] = useState<ActivityMode>("daily");
  const [activityTooltip, setActivityTooltip] = useState<{ x: number; label: string; week: number } | null>(null);
  const daily = useMemo(() => official ? official.daily.map((day) => ({
    ...day, inputTokens: 0, cachedInputTokens: 0, outputTokens: 0,
    reasoningOutputTokens: 0, sessionCount: 0, models: [],
  })) : stats.daily, [official, stats.daily]);
  const heatmap = useMemo(() => buildHeatmap(daily), [daily]);
  const weeklyActivity = useMemo(
    () => official && (activityMode === "cumulative" ? official.cumulative : official.weekly).length > 0
      ? buildOfficialWeeklyActivity(activityMode === "cumulative" ? official.cumulative : official.weekly, activityMode === "cumulative")
      : buildWeeklyActivity(daily, activityMode === "cumulative" ? "cumulative" : "weekly"),
    [activityMode, daily, official],
  );
  const monthLabels = useMemo(() => buildActivityMonthLabels(), []);
  const peakDay = useMemo(() => daily.reduce<DailyTokenUsage | null>(
    (peak, day) => !peak || day.totalTokens > peak.totalTokens ? day : peak,
    null,
  ), [daily]);
  const count = (value: number | null | undefined, compact = true) => value == null ? "—" : formatTokenCount(value, language, compact);
  const total = official ? official.totalTokens : stats.totalTokens;
  const peak = official ? official.peakDailyTokens : peakDay?.totalTokens ?? 0;
  const sessions = official ? official.sessionCount : stats.sessionCount;
  const currentStreak = official ? official.currentStreak : stats.currentStreak;
  const longestStreak = official ? official.longestStreak : stats.longestStreak;
  const activeDays = official ? daily.filter((day) => day.totalTokens > 0).length : stats.activeDays;
  const activityLabel = activityMode === "daily"
    ? t.activityHeatmapLabel
    : activityMode === "weekly"
      ? t.weeklyActivityLabel
      : t.cumulativeActivityLabel;

  const showDailyTooltip = (date: string, totalTokens: number, week: number) => {
    const x = ((week * ACTIVITY_CELL_STEP + 2.5) / ACTIVITY_WIDTH) * 100;
    setActivityTooltip({
      x,
      week,
      label: t.dailyActivityTooltip(formatTokenCount(totalTokens, language, false), formatDate(date, language)),
    });
  };

  const showWeeklyTooltip = (week: number, date: string, totalTokens: number) => {
    const x = ((week * ACTIVITY_CELL_STEP + 2.5) / ACTIVITY_WIDTH) * 100;
    setActivityTooltip({
      x,
      week,
      label: activityMode === "cumulative"
        ? t.cumulativeActivityTooltip(formatTokenCount(totalTokens, language, false), formatDate(date, language))
        : t.weeklyActivityTooltip(formatTokenCount(totalTokens, language, false), formatDate(date, language)),
    });
  };

  return (
    <div className="stats-overview">
      <section className="stats-summary-grid" aria-label={t.statsOverview}>
        <article><span>{t.totalTokens}</span><strong>{count(total)}</strong><small>{official ? t.officialAccount : t.allTime}</small></article>
        <article><span>{t.peakTokens}</span><strong>{count(peak)}</strong><small>{peakDay && peakDay.totalTokens === peak ? formatDate(peakDay.date, language) : t.allTime}</small></article>
        <article><span>{t.sessions}</span><strong>{count(sessions, false)}</strong><small>{official ? t.officialAccount : t.recordedSessions}</small></article>
        <article><span>{t.currentStreak}</span><strong>{count(currentStreak, false)}<small>{t.dayUnit}</small></strong><small>{t.keepMomentum}</small></article>
        <article><span>{t.longestStreak}</span><strong>{count(longestStreak, false)}<small>{t.dayUnit}</small></strong><small>{t.activeDays(activeDays)}</small></article>
      </section>

      <section className="activity-card">
        <div className="activity-heading">
          <div><strong>{t.tokenActivity}</strong><span>{t.activeDays(activeDays)}</span></div>
          <div className="activity-modes" aria-label={t.activityAggregation}>
            {(["daily", "weekly", "cumulative"] as const).map((mode) => (
              <button
                key={mode}
                type="button"
                className={activityMode === mode ? "is-active" : ""}
                aria-pressed={activityMode === mode}
                onClick={() => { setActivityMode(mode); setActivityTooltip(null); }}
              >
                {mode === "daily" ? t.dailyMode : mode === "weekly" ? t.weeklyMode : t.cumulativeMode}
              </button>
            ))}
          </div>
        </div>
        <div className="activity-map" role="img" aria-label={activityLabel} onPointerLeave={() => setActivityTooltip(null)}>
          <svg viewBox="0 0 317 41" preserveAspectRatio="none" aria-hidden="true">
            {activityMode === "daily" ? heatmap.map((day) => (
              <rect
                key={day.date}
                className={`activity-cell activity-cell--${day.level}${activityTooltip?.week === day.week ? " activity-cell--hover-column" : ""}`}
                x={day.week * ACTIVITY_CELL_STEP}
                y={day.weekday * ACTIVITY_CELL_STEP}
                width="5"
                height="5"
                rx="1"
                onPointerEnter={() => showDailyTooltip(day.date, day.totalTokens, day.week)}
                onPointerMove={() => showDailyTooltip(day.date, day.totalTokens, day.week)}
              />
            )) : weeklyActivity.flatMap((week) => Array.from({ length: 7 }, (_, row) => {
              const filled = row >= 7 - week.filledCells;
              return (
                <rect
                  key={`${week.week}-${row}`}
                  className={`activity-cell activity-cell--${filled ? 4 : 0}${activityTooltip?.week === week.week ? " activity-cell--hover-column" : ""}`}
                  x={week.week * ACTIVITY_CELL_STEP}
                  y={row * ACTIVITY_CELL_STEP}
                  width="5"
                  height="5"
                  rx="1"
                  onPointerEnter={() => showWeeklyTooltip(week.week, week.weekStart, week.totalTokens)}
                  onPointerMove={() => showWeeklyTooltip(week.week, week.weekStart, week.totalTokens)}
                />
              );
            }))}
          </svg>
          {activityTooltip ? <div className="activity-tooltip" role="tooltip" style={tooltipStyle(activityTooltip.x)}>{activityTooltip.label}</div> : null}
        </div>
        <div className="activity-months" aria-hidden="true">
          {monthLabels.map((label) => <span key={label.date} style={{ gridColumnStart: label.week + 1 }}>{new Intl.DateTimeFormat(language, { month: "short" }).format(localDate(label.date))}</span>)}
        </div>
      </section>

      {official ? <p className="stats-scope-note">{official.asOf ? t.officialStatsAsOf(formatDate(official.asOf, language)) : t.officialStatsScope}</p> : <section className="stats-detail-strip">
        <div><Stack /><span>{t.inputTokens}</span><strong>{formatTokenCount(stats.inputTokens, language)}</strong></div>
        <div><ChartLineUp /><span>{t.outputTokens}</span><strong>{formatTokenCount(stats.outputTokens, language)}</strong></div>
      </section>}
    </div>
  );
}

function Models({ stats, language }: { stats: UsageStats; language: Language }) {
  const t = copy[normalizeLanguage(language)];
  const [range, setRange] = useState<UsageRange>("30d");
  const [hoveredIndex, setHoveredIndex] = useState<number | null>(null);
  const days = useMemo(() => fillUsageRange(stats.daily, range), [range, stats.daily]);
  const models = useMemo(() => aggregateModels(days), [days]);
  const colors = useMemo(() => new Map(stats.models.map((model, index) => [model.model, MODEL_COLORS[index % MODEL_COLORS.length]])), [stats.models]);
  const modelColor = (model: string) => colors.get(model) ?? MODEL_COLORS[MODEL_COLORS.length - 1];
  const chartModels = models.slice(0, 5);
  const rangeTotal = models.reduce((sum, model) => sum + model.totalTokens, 0);
  const maximum = Math.max(1, ...chartModels.flatMap((model) => days.map((day) => modelDayTokens(day, model.model))));
  const firstDate = days[0]?.date ?? "—";
  const lastDate = days.at(-1)?.date ?? "—";
  const hoveredDay = hoveredIndex === null ? null : days[hoveredIndex] ?? null;
  const hoveredX = hoveredIndex === null || days.length <= 1 ? 0 : (hoveredIndex / (days.length - 1)) * CHART_WIDTH;

  const updateHoveredDay = (event: PointerEvent<HTMLDivElement>) => {
    const bounds = event.currentTarget.getBoundingClientRect();
    const ratio = bounds.width > 0 ? Math.min(1, Math.max(0, (event.clientX - bounds.left) / bounds.width)) : 0;
    setHoveredIndex(Math.round(ratio * Math.max(0, days.length - 1)));
  };

  const handleChartKey = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    if (event.key === "Home") setHoveredIndex(0);
    else if (event.key === "End") setHoveredIndex(Math.max(0, days.length - 1));
    else setHoveredIndex((current) => {
      const initial = current ?? Math.max(0, days.length - 1);
      return Math.min(Math.max(0, days.length - 1), Math.max(0, initial + (event.key === "ArrowLeft" ? -1 : 1)));
    });
  };

  return (
    <div className="stats-models">
      <p className="stats-scope-note">{t.localModelsScope}</p>
      <div className="stats-range" aria-label={t.dateRange}>
        {(["7d", "30d", "all"] as const).map((value) => (
          <button key={value} type="button" aria-pressed={range === value} className={range === value ? "is-active" : ""} onClick={() => { setRange(value); setHoveredIndex(null); }}>
            {value === "7d" ? t.last7Days : value === "30d" ? t.last30Days : t.allTime}
          </button>
        ))}
      </div>

      <section className="token-chart-card">
        <div className="stats-section-heading">
          <div><strong>{t.tokensPerDay}</strong><span>{t.dailyModelUsage}</span></div>
          <strong>{formatTokenCount(rangeTotal, language)}</strong>
        </div>
        {rangeTotal > 0 ? (
          <>
            <div
              className="token-chart"
              data-testid="token-chart"
              role="img"
              tabIndex={0}
              aria-label={`${t.tokensPerDay}. ${t.chartKeyboardHint}`}
              onPointerMove={updateHoveredDay}
              onPointerLeave={() => setHoveredIndex(null)}
              onFocus={() => setHoveredIndex(Math.max(0, days.length - 1))}
              onBlur={() => setHoveredIndex(null)}
              onKeyDown={handleChartKey}
            >
              <span className="token-chart__max">{formatTokenCount(maximum, language)}</span>
              <svg viewBox="0 0 312 112" preserveAspectRatio="none" aria-hidden="true">
                {[0, 1, 2].map((line) => <line key={line} className="chart-grid-line" x1="0" x2={CHART_WIDTH} y1={line * 50 + 6} y2={line * 50 + 6} />)}
                {chartModels.map((model) => {
                  const values = days.map((day) => modelDayTokens(day, model.model));
                  return (
                    <path
                      key={model.model}
                      d={linePath(values, CHART_WIDTH, CHART_HEIGHT, maximum)}
                      transform="translate(0 6)"
                      fill="none"
                      stroke={modelColor(model.model)}
                      strokeWidth="2"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                      vectorEffect="non-scaling-stroke"
                    />
                  );
                })}
                {hoveredDay ? (
                  <>
                    <line className="chart-hover-line" x1={hoveredX} x2={hoveredX} y1="6" y2="106" />
                    {chartModels.map((model) => {
                      const value = modelDayTokens(hoveredDay, model.model);
                      const y = 6 + CHART_HEIGHT - (value / maximum) * CHART_HEIGHT;
                      return <circle key={model.model} cx={hoveredX} cy={y} r="3" fill={modelColor(model.model)} className="chart-hover-point" />;
                    })}
                  </>
                ) : null}
              </svg>
              <div className="token-chart__axis"><span>{firstDate.slice(5)}</span><span>{lastDate.slice(5)}</span></div>
              {hoveredDay ? (
                <div className="token-chart__tooltip" role="tooltip" style={tooltipStyle(days.length <= 1 ? 50 : (hoveredIndex! / (days.length - 1)) * 100)}>
                  <strong>{formatDate(hoveredDay.date, language)}</strong>
                  <span className="token-chart__tooltip-total">{t.totalTokens} {formatTokenCount(hoveredDay.totalTokens, language, false)}</span>
                  {chartModels.map((model) => (
                    <span key={model.model}><i style={{ backgroundColor: modelColor(model.model) }} />{formatModelName(model.model)}<em>{formatTokenCount(modelDayTokens(hoveredDay, model.model), language, false)}</em></span>
                  ))}
                </div>
              ) : null}
            </div>
            <div className="model-legend">
              {chartModels.map((model) => <span key={model.model}><i style={{ backgroundColor: modelColor(model.model) }} />{formatModelName(model.model)}</span>)}
            </div>
          </>
        ) : <div className="chart-empty">{t.noUsageInRange}</div>}
      </section>

      <section className="model-breakdown" aria-label={t.modelBreakdown}>
        <div className="stats-section-heading"><div><strong>{t.modelsTab}</strong><span>{t.inputOutputBreakdown}</span></div></div>
        <div className="model-list">
          {models.map((model) => {
            const share = rangeTotal > 0 ? (model.totalTokens / rangeTotal) * 100 : 0;
            return (
              <article key={model.model} className="model-row">
                <div className="model-row__heading"><span><i style={{ backgroundColor: modelColor(model.model) }} />{formatModelName(model.model)}</span><strong>{share.toFixed(1)}%</strong></div>
                <div className="model-share"><i style={{ width: `${share}%`, backgroundColor: modelColor(model.model) }} /></div>
                <div className="model-row__tokens"><span>{t.inputShort} <strong>{formatTokenCount(model.inputTokens, language)}</strong></span><span>{t.outputShort} <strong>{formatTokenCount(model.outputTokens, language)}</strong></span></div>
              </article>
            );
          })}
        </div>
      </section>
    </div>
  );
}

export const UsageStatsPanel = memo(function UsageStatsPanel({ stats, loading, failed, language }: UsageStatsPanelProps) {
  const t = copy[normalizeLanguage(language)];
  const [tab, setTab] = useState<StatsTab>("overview");
  const official = stats?.official;
  const officialOverview = tab === "overview" && official && official.status !== "not_applicable";
  let state: "loading" | "empty" | "unavailable" | null = null;
  if (failed || (!officialOverview && stats?.status === "unavailable") || (!stats && !loading)) state = "unavailable";
  else if (!stats && loading) state = "loading";
  else if (!officialOverview && stats?.status === "empty") state = "empty";

  return (
    <section className="usage-stats-panel">
      <div className="stats-tabs" role="tablist" aria-label={t.usageStats}>
        <button type="button" role="tab" aria-selected={tab === "overview"} className={tab === "overview" ? "is-active" : ""} onClick={() => setTab("overview")}>{t.statsOverview}</button>
        <button type="button" role="tab" aria-selected={tab === "models"} className={tab === "models" ? "is-active" : ""} onClick={() => setTab("models")}>{t.modelsTab}</button>
      </div>
      {state ? <StatsPlaceholder kind={state} language={language} /> : officialOverview && official.status !== "ok" ? (
        <div className="stats-placeholder stats-placeholder--unavailable" role="alert"><span><Database /></span><strong>{t.officialStatsUnavailable}</strong><p>{t.officialStatsUnavailableHint}</p></div>
      ) : stats ? (tab === "overview" ? <Overview stats={stats} official={officialOverview ? official : undefined} language={language} /> : <Models stats={stats} language={language} />) : null}
      <footer className="stats-source"><LockKey /><span>{officialOverview ? t.officialStatsPrivacy : t.localStatsPrivacy}</span>{tab === "models" && stats?.skippedFiles ? <em>{t.partialStats(stats.skippedFiles)}</em> : null}</footer>
      {loading && stats ? <div className="stats-refreshing" role="status"><ClockCounterClockwise className="is-spinning" />{t.statsRefreshing}</div> : null}
    </section>
  );
});
