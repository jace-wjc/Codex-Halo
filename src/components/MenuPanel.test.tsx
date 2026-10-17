// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ProviderSnapshot, UsageStats, WidgetPreferences } from "../types";
import { MenuPanel } from "./MenuPanel";

const snapshot: ProviderSnapshot = {
  provider: "codex",
  displayName: "CODEX",
  plan: "PRO",
  shortWindow: { remainingPercent: 74, resetsAt: null, windowSeconds: 18_000 },
  weeklyWindow: { remainingPercent: 42, resetsAt: null, windowSeconds: 604_800 },
  resetCredits: null,
  updatedAt: new Date().toISOString(),
  status: "ok",
  message: null,
};
const preferences: WidgetPreferences = {
  locked: false,
  alwaysOnTop: true,
  pinnedProvider: null,
  autoRotateSeconds: 12,
  quotaRefreshIntervalSeconds: 300,
  language: "zh-CN",
};
const usageStats: UsageStats = {
  status: "ok",
  generatedAt: "2026-07-15T00:00:00Z",
  firstActivityDate: "2026-07-14",
  lastActivityDate: "2026-07-15",
  inputTokens: 3300,
  cachedInputTokens: 1200,
  outputTokens: 300,
  reasoningOutputTokens: 100,
  totalTokens: 3600,
  sessionCount: 2,
  activeDays: 2,
  currentStreak: 2,
  longestStreak: 2,
  indexedFiles: 2,
  skippedFiles: 0,
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

afterEach(cleanup);

describe("MenuPanel", () => {
  it("exposes every requested desktop action", () => {
    const actions = {
      onRefresh: vi.fn(),
      onToggleWidget: vi.fn(),
      onToggleAlwaysOnTop: vi.fn(),
      onToggleAutostart: vi.fn(),
      onToggleLanguage: vi.fn(),
      onToggleClickThrough: vi.fn(),
      onQuit: vi.fn(),
    };
    render(
      <MenuPanel
        snapshot={snapshot}
        preferences={preferences}
        widgetVisible
        autostartEnabled={false}
        refreshing={false}
        {...actions}
      />,
    );

    expect(screen.getByText("额度偏低")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "刷新额度与本地使用统计" }));
    fireEvent.click(screen.getByRole("switch", { name: /隐藏悬浮窗/ }));
    fireEvent.click(screen.getByRole("switch", { name: /保持置顶/ }));
    fireEvent.click(screen.getByRole("switch", { name: /开机启动/ }));
    fireEvent.click(screen.getByRole("switch", { name: /鼠标穿透/ }));
    fireEvent.click(screen.getByRole("button", { name: /语言/ }));
    fireEvent.click(screen.getByRole("button", { name: /退出 Codex Halo/ }));

    expect(actions.onRefresh).toHaveBeenCalledOnce();
    expect(actions.onToggleWidget).toHaveBeenCalledOnce();
    expect(actions.onToggleAlwaysOnTop).toHaveBeenCalledOnce();
    expect(actions.onToggleAutostart).toHaveBeenCalledOnce();
    expect(actions.onToggleClickThrough).toHaveBeenCalledOnce();
    expect(actions.onToggleLanguage).toHaveBeenCalledOnce();
    expect(actions.onQuit).toHaveBeenCalledOnce();
  });

  it("disables refresh and an in-flight setting", () => {
    render(
      <MenuPanel
        snapshot={snapshot}
        preferences={preferences}
        widgetVisible
        autostartEnabled={false}
        refreshing
        pendingAction="autostart"
        onRefresh={() => {}}
        onToggleWidget={() => {}}
        onToggleAlwaysOnTop={() => {}}
        onToggleAutostart={() => {}}
        onToggleLanguage={() => {}}
        onToggleClickThrough={() => {}}
        onQuit={() => {}}
      />,
    );
    expect(screen.getByRole("button", { name: "刷新额度与本地使用统计" })).toBeDisabled();
    expect(screen.getByRole("switch", { name: /开机启动/ })).toBeDisabled();
  });

  it("loads local stats on demand and switches between Overview and Models", () => {
    const onLoadUsageStats = vi.fn();
    const onRefresh = vi.fn();
    render(
      <MenuPanel
        snapshot={snapshot}
        preferences={preferences}
        widgetVisible
        autostartEnabled={false}
        refreshing={false}
        usageStats={usageStats}
        onRefresh={onRefresh}
        onLoadUsageStats={onLoadUsageStats}
        onToggleWidget={() => {}}
        onToggleAlwaysOnTop={() => {}}
        onToggleAutostart={() => {}}
        onToggleLanguage={() => {}}
        onToggleClickThrough={() => {}}
        onQuit={() => {}}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "使用统计" }));
    expect(onLoadUsageStats).toHaveBeenCalledOnce();
    expect(screen.getByRole("tab", { name: "Overview" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByText("总 Token")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("tab", { name: "Models" }));
    expect(screen.getByText("Tokens per Day")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "全部时间" }));
    expect(screen.getAllByText("GPT 5.4")).toHaveLength(2);
    fireEvent.click(screen.getByRole("button", { name: "刷新额度与本地使用统计" }));
    expect(onRefresh).toHaveBeenCalledOnce();
  });

  it("warns before saving the 10-second quota refresh interval", () => {
    const onSetRefreshInterval = vi.fn();
    render(
      <MenuPanel
        snapshot={snapshot}
        preferences={preferences}
        widgetVisible
        autostartEnabled={false}
        refreshing={false}
        onRefresh={() => {}}
        onToggleWidget={() => {}}
        onToggleAlwaysOnTop={() => {}}
        onToggleAutostart={() => {}}
        onToggleLanguage={() => {}}
        onSetRefreshInterval={onSetRefreshInterval}
        onToggleClickThrough={() => {}}
        onQuit={() => {}}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /额度自动刷新/ }));
    expect(screen.getByRole("dialog", { name: "自动刷新频率" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "每 10 秒" }));
    expect(screen.getByText(/增加接口请求/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    expect(onSetRefreshInterval).toHaveBeenCalledWith(10);
  });

  it("accepts a validated custom quota refresh interval", () => {
    const onSetRefreshInterval = vi.fn();
    render(
      <MenuPanel
        snapshot={snapshot}
        preferences={preferences}
        widgetVisible
        autostartEnabled={false}
        refreshing={false}
        onRefresh={() => {}}
        onToggleWidget={() => {}}
        onToggleAlwaysOnTop={() => {}}
        onToggleAutostart={() => {}}
        onToggleLanguage={() => {}}
        onSetRefreshInterval={onSetRefreshInterval}
        onToggleClickThrough={() => {}}
        onQuit={() => {}}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /额度自动刷新/ }));
    fireEvent.click(screen.getByRole("button", { name: "自定义" }));
    fireEvent.change(screen.getByRole("spinbutton", { name: "自定义" }), { target: { value: "2" } });
    fireEvent.change(screen.getByRole("combobox", { name: "自动刷新频率" }), { target: { value: "minutes" } });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    expect(onSetRefreshInterval).toHaveBeenCalledWith(120);
  });

  it("rejects a custom quota refresh interval shorter than 10 seconds", () => {
    render(
      <MenuPanel
        snapshot={snapshot}
        preferences={preferences}
        widgetVisible
        autostartEnabled={false}
        refreshing={false}
        onRefresh={() => {}}
        onToggleWidget={() => {}}
        onToggleAlwaysOnTop={() => {}}
        onToggleAutostart={() => {}}
        onToggleLanguage={() => {}}
        onToggleClickThrough={() => {}}
        onQuit={() => {}}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /额度自动刷新/ }));
    fireEvent.click(screen.getByRole("button", { name: "自定义" }));
    fireEvent.change(screen.getByRole("spinbutton", { name: "自定义" }), { target: { value: "9" } });
    fireEvent.change(screen.getByRole("combobox", { name: "自动刷新频率" }), { target: { value: "seconds" } });

    expect(screen.getByText("请输入 10 秒到 24 小时之间的时间")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();
  });
});
