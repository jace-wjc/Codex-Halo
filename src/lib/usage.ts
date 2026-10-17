import type { DailyTokenUsage, Language, ModelTokenUsage, OfficialUsageBucket } from "../types";

export type UsageRange = "7d" | "30d" | "all";
export type ActivityMode = "daily" | "weekly" | "cumulative";

export const MODEL_COLORS = ["#4e6f99", "#8a6748", "#5f7567", "#756a8d", "#8a5960", "#64717e"] as const;

const emptyDay = (date: string): DailyTokenUsage => ({
  date,
  inputTokens: 0,
  cachedInputTokens: 0,
  outputTokens: 0,
  reasoningOutputTokens: 0,
  totalTokens: 0,
  sessionCount: 0,
  models: [],
});

export function formatTokenCount(value: number, language: Language, compact = true): string {
  if (!Number.isFinite(value)) return "—";
  return new Intl.NumberFormat(language, compact
    ? { notation: "compact", maximumFractionDigits: value >= 1_000_000 ? 1 : 0 }
    : { maximumFractionDigits: 0 }).format(Math.max(0, value));
}

export function formatModelName(model: string): string {
  if (!model || model === "unknown") return "Unknown";
  return model
    .split("-")
    .filter(Boolean)
    .map((part) => {
      const lower = part.toLowerCase();
      if (lower === "gpt") return "GPT";
      if (lower === "codex") return "Codex";
      return /^\d/.test(part) ? part : part.charAt(0).toUpperCase() + part.slice(1);
    })
    .join(" ");
}

function dateKey(value: Date): string {
  const year = value.getFullYear();
  const month = String(value.getMonth() + 1).padStart(2, "0");
  const day = String(value.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function parseDate(value: string): Date {
  const [year, month, day] = value.split("-").map(Number);
  return new Date(year, Math.max(0, month - 1), day);
}

function addDays(value: Date, days: number): Date {
  const next = new Date(value);
  next.setDate(next.getDate() + days);
  return next;
}

function startOfWeek(value: Date): Date {
  return addDays(value, -value.getDay());
}

function activityGridStart(now: Date): Date {
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  return addDays(startOfWeek(today), -52 * 7);
}

function calendarDayIndex(value: Date): number {
  return Date.UTC(value.getFullYear(), value.getMonth(), value.getDate()) / 86_400_000;
}

export function fillUsageRange(
  daily: DailyTokenUsage[],
  range: UsageRange,
  now = new Date(),
): DailyTokenUsage[] {
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const byDate = new Map(daily.map((day) => [day.date, day]));
  const firstRecorded = daily.length > 0 ? parseDate(daily[0].date) : today;
  const start = range === "7d"
    ? addDays(today, -6)
    : range === "30d"
      ? addDays(today, -29)
      : firstRecorded < today ? firstRecorded : today;
  const rows: DailyTokenUsage[] = [];
  for (let cursor = start; cursor <= today; cursor = addDays(cursor, 1)) {
    const key = dateKey(cursor);
    rows.push(byDate.get(key) ?? emptyDay(key));
  }
  return rows;
}

export function aggregateModels(daily: DailyTokenUsage[]): ModelTokenUsage[] {
  const models = new Map<string, ModelTokenUsage>();
  for (const day of daily) {
    for (const usage of day.models) {
      const current = models.get(usage.model) ?? {
        model: usage.model,
        inputTokens: 0,
        cachedInputTokens: 0,
        outputTokens: 0,
        reasoningOutputTokens: 0,
        totalTokens: 0,
      };
      current.inputTokens += usage.inputTokens;
      current.cachedInputTokens += usage.cachedInputTokens;
      current.outputTokens += usage.outputTokens;
      current.reasoningOutputTokens += usage.reasoningOutputTokens;
      current.totalTokens += usage.totalTokens;
      models.set(usage.model, current);
    }
  }
  return [...models.values()].sort((left, right) => right.totalTokens - left.totalTokens);
}

export interface HeatmapDay {
  date: string;
  totalTokens: number;
  level: 0 | 1 | 2 | 3 | 4;
  week: number;
  weekday: number;
}

export function buildHeatmap(daily: DailyTokenUsage[], now = new Date()): HeatmapDay[] {
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const start = activityGridStart(now);
  const totals = new Map(daily.map((day) => [day.date, day.totalTokens]));
  const values: Array<Omit<HeatmapDay, "level">> = [];
  for (let index = 0; index < 53 * 7; index += 1) {
    const date = addDays(start, index);
    if (date > today) break;
    values.push({
      date: dateKey(date),
      totalTokens: totals.get(dateKey(date)) ?? 0,
      week: Math.floor(index / 7),
      weekday: index % 7,
    });
  }
  const max = Math.max(0, ...values.map((day) => day.totalTokens));
  return values.map((day) => {
    if (day.totalTokens <= 0 || max <= 0) return { ...day, level: 0 };
    const ratio = Math.log1p(day.totalTokens) / Math.log1p(max);
    return { ...day, level: Math.min(4, Math.max(1, Math.ceil(ratio * 4))) as 1 | 2 | 3 | 4 };
  });
}

export interface WeeklyActivity {
  week: number;
  weekStart: string;
  totalTokens: number;
  filledCells: number;
}

export function buildWeeklyActivity(
  daily: DailyTokenUsage[],
  mode: Exclude<ActivityMode, "daily">,
  now = new Date(),
): WeeklyActivity[] {
  const start = activityGridStart(now);
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const weeklyTotals = Array.from({ length: 53 }, () => 0);
  let tokensBeforeGrid = 0;

  for (const day of daily) {
    const date = parseDate(day.date);
    if (date > today) continue;
    const offset = calendarDayIndex(date) - calendarDayIndex(start);
    if (offset < 0) {
      tokensBeforeGrid += day.totalTokens;
      continue;
    }
    const week = Math.floor(offset / 7);
    if (week < weeklyTotals.length) weeklyTotals[week] += day.totalTokens;
  }

  let cumulative = tokensBeforeGrid;
  const totals = weeklyTotals.map((value) => {
    cumulative += value;
    return mode === "weekly" ? value : cumulative;
  });
  const maximum = Math.max(0, ...totals);

  return totals.map((totalTokens, week) => ({
    week,
    weekStart: dateKey(addDays(start, week * 7)),
    totalTokens,
    filledCells: totalTokens > 0 && maximum > 0
      ? Math.min(7, Math.max(1, Math.ceil((totalTokens / maximum) * 7)))
      : 0,
  }));
}

/** Keep the server's weekly/cumulative totals rather than adding cumulative days. */
export function buildOfficialWeeklyActivity(
  buckets: OfficialUsageBucket[], cumulative: boolean, now = new Date(),
): WeeklyActivity[] {
  const start = activityGridStart(now);
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const weekly = new Map<number, number>();
  let before = 0;
  for (const bucket of [...buckets].sort((a, b) => a.date.localeCompare(b.date))) {
    const date = parseDate(bucket.date);
    if (date > today) continue;
    const week = Math.floor((calendarDayIndex(date) - calendarDayIndex(start)) / 7);
    if (week < 0) before = bucket.totalTokens;
    else if (week < 53) weekly.set(week, bucket.totalTokens);
  }
  let previous = cumulative ? before : 0;
  const totals = Array.from({ length: 53 }, (_, week) => {
    const value = weekly.get(week) ?? (cumulative ? previous : 0);
    previous = value;
    return value;
  });
  const maximum = Math.max(0, ...totals);
  return totals.map((totalTokens, week) => ({
    week, weekStart: dateKey(addDays(start, week * 7)), totalTokens,
    filledCells: totalTokens > 0 && maximum > 0 ? Math.min(7, Math.max(1, Math.ceil(totalTokens / maximum * 7))) : 0,
  }));
}

export interface ActivityMonthLabel {
  week: number;
  date: string;
}

export function buildActivityMonthLabels(now = new Date()): ActivityMonthLabel[] {
  const start = activityGridStart(now);
  const labels: ActivityMonthLabel[] = [];
  let previousMonth = -1;
  for (let week = 0; week < 53; week += 1) {
    const date = addDays(start, week * 7);
    if (date.getMonth() !== previousMonth) {
      labels.push({ week, date: dateKey(date) });
      previousMonth = date.getMonth();
    }
  }
  if (labels.length > 1 && labels[1].week - labels[0].week < 4) labels.shift();
  return labels;
}

export function modelDayTokens(day: DailyTokenUsage, model: string): number {
  return day.models.find((item) => item.model === model)?.totalTokens ?? 0;
}

export function linePath(values: number[], width: number, height: number, scaleMaximum?: number): string {
  if (values.length === 0) return "";
  const max = Math.max(1, scaleMaximum ?? 0, ...values);
  return values.map((value, index) => {
    const x = values.length === 1 ? width / 2 : (index / (values.length - 1)) * width;
    const y = height - (value / max) * height;
    return `${index === 0 ? "M" : "L"}${x.toFixed(2)},${y.toFixed(2)}`;
  }).join(" ");
}
