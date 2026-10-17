import { describe, expect, it } from "vitest";
import type { DailyTokenUsage } from "../types";
import {
  aggregateModels,
  buildActivityMonthLabels,
  buildHeatmap,
  buildWeeklyActivity,
  buildOfficialWeeklyActivity,
  fillUsageRange,
  formatModelName,
  linePath,
} from "./usage";

const day = (date: string, totalTokens: number, model = "gpt-5.4"): DailyTokenUsage => ({
  date,
  inputTokens: totalTokens - 10,
  cachedInputTokens: 0,
  outputTokens: 10,
  reasoningOutputTokens: 0,
  totalTokens,
  sessionCount: 1,
  models: [{
    model,
    inputTokens: totalTokens - 10,
    cachedInputTokens: 0,
    outputTokens: 10,
    reasoningOutputTokens: 0,
    totalTokens,
  }],
});

describe("usage helpers", () => {
  it("uses server cumulative buckets without summing cumulative days again", () => {
    const weeks = buildOfficialWeeklyActivity([
      { date: "2026-07-05", totalTokens: 100 },
      { date: "2026-07-06", totalTokens: 150 },
    ], true, new Date(2026, 6, 15));
    expect(weeks.at(-2)?.totalTokens).toBe(150);
    expect(weeks.at(-1)?.totalTokens).toBe(150);
  });

  it("preserves the official weekly value and does not carry weekly usage forward", () => {
    const weeks = buildOfficialWeeklyActivity([{ date: "2026-07-05", totalTokens: 123 }], false, new Date(2026, 6, 15));
    expect(weeks.at(-2)?.totalTokens).toBe(123);
    expect(weeks.at(-1)?.totalTokens).toBe(0);
  });
  it("fills fixed ranges without inventing token values", () => {
    const rows = fillUsageRange([day("2026-07-14", 100)], "7d", new Date(2026, 6, 15));
    expect(rows).toHaveLength(7);
    expect(rows.at(-2)?.totalTokens).toBe(100);
    expect(rows.at(-1)?.totalTokens).toBe(0);
  });

  it("aggregates models and keeps descending usage order", () => {
    const models = aggregateModels([
      day("2026-07-14", 100, "gpt-5.4"),
      day("2026-07-15", 250, "gpt-5.6-sol"),
    ]);
    expect(models.map((model) => model.model)).toEqual(["gpt-5.6-sol", "gpt-5.4"]);
    expect(formatModelName(models[0].model)).toBe("GPT 5.6 Sol");
  });

  it("builds a 53-week contribution grid ending today", () => {
    const heatmap = buildHeatmap([day("2026-07-15", 100)], new Date(2026, 6, 15));
    expect(heatmap.length).toBeGreaterThan(360);
    expect(heatmap.at(-1)?.date).toBe("2026-07-15");
    expect(heatmap.at(-1)?.level).toBe(4);
  });

  it("aggregates Sunday-based weeks into bottom-up activity bars", () => {
    const weekly = buildWeeklyActivity([
      day("2026-07-11", 100),
      day("2026-07-12", 200),
      day("2026-07-15", 300),
    ], "weekly", new Date(2026, 6, 15));

    expect(weekly.at(-2)).toMatchObject({ weekStart: "2026-07-05", totalTokens: 100 });
    expect(weekly.at(-1)).toMatchObject({ weekStart: "2026-07-12", totalTokens: 500, filledCells: 7 });
  });

  it("keeps cumulative weekly bars monotonic without inventing usage", () => {
    const cumulative = buildWeeklyActivity([
      day("2026-07-05", 100),
      day("2026-07-12", 300),
    ], "cumulative", new Date(2026, 6, 15));
    const populated = cumulative.filter((week) => week.totalTokens > 0);

    expect(cumulative.find((week) => week.weekStart === "2026-07-05")?.totalTokens).toBe(100);
    expect(cumulative.find((week) => week.weekStart === "2026-07-12")?.totalTokens).toBe(400);
    expect(populated.every((week, index) => index === 0 || week.filledCells >= populated[index - 1].filledCells)).toBe(true);
  });

  it("provides month positions for the complete 53-week grid", () => {
    const labels = buildActivityMonthLabels(new Date(2026, 6, 15));
    expect(labels[0]).toEqual({ week: 3, date: "2025-08-03" });
    expect(labels.at(-1)?.date.startsWith("2026-07")).toBe(true);
  });

  it("returns a bounded svg path", () => {
    expect(linePath([0, 10, 5], 100, 40)).toBe("M0.00,40.00 L50.00,0.00 L100.00,20.00");
  });
});
