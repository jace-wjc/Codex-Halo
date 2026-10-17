// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { OfficialUsage, UsageStats } from "../types";
import { UsageStatsPanel } from "./UsageStatsPanel";

const emptyStats: UsageStats = {
  status: "empty",
  generatedAt: "2026-07-15T00:00:00Z",
  firstActivityDate: null,
  lastActivityDate: null,
  inputTokens: 0,
  cachedInputTokens: 0,
  outputTokens: 0,
  reasoningOutputTokens: 0,
  totalTokens: 0,
  sessionCount: 0,
  activeDays: 0,
  currentStreak: 0,
  longestStreak: 0,
  indexedFiles: 0,
  skippedFiles: 0,
  models: [],
  daily: [],
};

const usageStats: UsageStats = {
  ...emptyStats,
  status: "ok",
  firstActivityDate: "2026-07-14",
  lastActivityDate: "2026-07-15",
  inputTokens: 3300,
  outputTokens: 300,
  totalTokens: 3600,
  sessionCount: 2,
  activeDays: 2,
  currentStreak: 2,
  longestStreak: 2,
  indexedFiles: 2,
  models: [{ model: "gpt-5.4", inputTokens: 3300, cachedInputTokens: 1200, outputTokens: 300, reasoningOutputTokens: 100, totalTokens: 3600 }],
  daily: [
    {
      date: "2026-07-14",
      inputTokens: 1400,
      cachedInputTokens: 500,
      outputTokens: 100,
      reasoningOutputTokens: 40,
      totalTokens: 1500,
      sessionCount: 1,
      models: [{ model: "gpt-5.4", inputTokens: 1400, cachedInputTokens: 500, outputTokens: 100, reasoningOutputTokens: 40, totalTokens: 1500 }],
    },
    {
      date: "2026-07-15",
      inputTokens: 1900,
      cachedInputTokens: 700,
      outputTokens: 200,
      reasoningOutputTokens: 60,
      totalTokens: 2100,
      sessionCount: 1,
      models: [{ model: "gpt-5.4", inputTokens: 1900, cachedInputTokens: 700, outputTokens: 200, reasoningOutputTokens: 60, totalTokens: 2100 }],
    },
  ],
};

beforeEach(() => { vi.useFakeTimers({ toFake: ["Date"] }); vi.setSystemTime(new Date(2026, 6, 15, 12)); });
afterEach(() => { cleanup(); vi.useRealTimers(); });

const official: OfficialUsage = {
  status: "ok", generatedAt: "2026-07-15T00:00:00Z", totalTokens: 1_234_000,
  asOf: "2026-07-14",
  peakDailyTokens: 700_000, sessionCount: 88, longestRunningTurnSeconds: 300,
  currentStreak: 20, longestStreak: 22,
  daily: [{ date: "2026-07-15", totalTokens: 500_000 }],
  weekly: [{ date: "2026-07-12", totalTokens: 600_000 }],
  cumulative: [{ date: "2026-07-15", totalTokens: 1_234_000 }],
};

describe("UsageStatsPanel states", () => {
  it("shows the official cutoff rather than the request time", () => {
    render(<UsageStatsPanel stats={{ ...usageStats, official }} loading={false} failed={false} language="en" />);
    expect(screen.getByText(/Official data through/)).toHaveTextContent("Jul 14, 2026");
    expect(screen.getByText(/Official data through/)).not.toHaveTextContent("Jul 15");
  });

  it("does not fabricate an official cutoff when the server omits it", () => {
    render(<UsageStatsPanel stats={{ ...usageStats, official: { ...official, asOf: null } }} loading={false} failed={false} language="zh-CN" />);
    expect(screen.getByText(/数据可能延迟更新/)).toBeInTheDocument();
    expect(screen.queryByText(/官方统计截至/)).not.toBeInTheDocument();
  });
  it("uses official totals and streaks while keeping Models local", () => {
    const { container } = render(<UsageStatsPanel stats={{ ...usageStats, official }} loading={false} failed={false} language="en" />);
    expect(container.querySelector(".stats-summary-grid")).toHaveTextContent("1.2M");
    expect(container.querySelector(".stats-summary-grid")).toHaveTextContent("700K");
    expect(container.querySelector(".stats-summary-grid")).toHaveTextContent("88");
    expect(container.querySelector(".stats-summary-grid")).not.toHaveTextContent("4K");
    expect(screen.queryByText("Input tokens")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "Models" }));
    expect(container.querySelector(".token-chart-card")).toHaveTextContent("4K");
    expect(screen.getByText(/Local model details/)).toBeInTheDocument();
  });

  it("never replaces failed official totals with local totals", () => {
    render(<UsageStatsPanel stats={{ ...usageStats, official: { ...official, status: "unavailable", totalTokens: null } }} loading={false} failed={false} language="en" />);
    expect(screen.getByRole("alert")).toHaveTextContent("Official usage statistics unavailable");
    expect(screen.queryByText("3.6K")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "Models" }));
    expect(screen.getByTestId("token-chart")).toBeInTheDocument();
  });

  it("shows official statistics even when local history is empty", () => {
    render(<UsageStatsPanel stats={{ ...emptyStats, official }} loading={false} failed={false} language="en" />);
    expect(screen.getByText("1.2M")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "Models" }));
    expect(screen.getByRole("alert")).toHaveTextContent("No usage to show yet");
  });
  it("explains that the initial index may take longer", () => {
    render(<UsageStatsPanel stats={null} loading failed={false} language="zh-CN" />);
    expect(screen.getByRole("status")).toHaveTextContent("正在建立本地统计");
    expect(screen.queryByText(/总 Token/)).not.toBeInTheDocument();
  });

  it("does not fabricate values when there are no local counters", () => {
    render(<UsageStatsPanel stats={emptyStats} loading={false} failed={false} language="zh-CN" />);
    expect(screen.getByRole("alert")).toHaveTextContent("还没有可统计的记录");
    expect(screen.queryByText("0 Token")).not.toBeInTheDocument();
  });

  it("shows an explicit unavailable state after a native read failure", () => {
    render(<UsageStatsPanel stats={null} loading={false} failed language="en" />);
    expect(screen.getByRole("alert")).toHaveTextContent("Local statistics unavailable");
  });

  it("switches between daily, weekly, and cumulative activity without changing source data", () => {
    const { container } = render(<UsageStatsPanel stats={usageStats} loading={false} failed={false} language="zh-CN" />);
    expect(screen.getByText("单日峰值")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Weekly" }));
    expect(screen.getByRole("button", { name: "Weekly" })).toHaveAttribute("aria-pressed", "true");
    const activeCell = container.querySelector(".activity-cell--4");
    expect(activeCell).not.toBeNull();
    fireEvent.pointerEnter(activeCell!);
    expect(screen.getByRole("tooltip")).toHaveTextContent("当周");

    fireEvent.click(screen.getByRole("button", { name: "Cumulative" }));
    expect(screen.getByRole("button", { name: "Cumulative" })).toHaveAttribute("aria-pressed", "true");
  });

  it("shows exact per-model values when the token chart is inspected", () => {
    render(<UsageStatsPanel stats={usageStats} loading={false} failed={false} language="en" />);
    fireEvent.click(screen.getByRole("tab", { name: "Models" }));
    fireEvent.focus(screen.getByTestId("token-chart"));

    const tooltip = screen.getByRole("tooltip");
    expect(tooltip).toHaveTextContent("Jul 15, 2026");
    expect(tooltip).toHaveTextContent("GPT 5.4");
    expect(tooltip).toHaveTextContent("2,100");
  });
});
