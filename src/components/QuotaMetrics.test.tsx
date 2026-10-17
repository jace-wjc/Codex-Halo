// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ProviderSnapshot } from "../types";
import { QuotaMetrics } from "./QuotaMetrics";

const success: ProviderSnapshot = {
  provider: "codex",
  displayName: "CODEX",
  plan: "PRO",
  shortWindow: { remainingPercent: 74, resetsAt: "2026-07-15T00:00:00Z", windowSeconds: 18_000 },
  weeklyWindow: { remainingPercent: 8, resetsAt: "2026-07-20T00:00:00Z", windowSeconds: 604_800 },
  resetCredits: 0,
  resetCreditExpiresAt: [],
  updatedAt: new Date().toISOString(),
  status: "ok",
  message: null,
};

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("QuotaMetrics", () => {
  it("shows both real windows and distinguishes zero reset credits", () => {
    render(<QuotaMetrics snapshot={success} language="zh-CN" />);
    expect(screen.getByText("5 小时额度")).toBeInTheDocument();
    expect(screen.getByText("周额度")).toBeInTheDocument();
    expect(screen.getByText("74")).toBeInTheDocument();
    expect(screen.getByText("8")).toBeInTheDocument();
    expect(screen.getByText("暂无重置额度")).toBeInTheDocument();
  });

  it("does not leak old values into a signed-out state", () => {
    render(<QuotaMetrics snapshot={{ ...success, status: "signed_out", message: "Please sign in" }} language="en" />);
    expect(screen.getByText("Sign in to Codex")).toBeInTheDocument();
    expect(screen.queryByText("74")).not.toBeInTheDocument();
  });

  it("hides expired stale values and offers a real retry action", () => {
    const refresh = vi.fn();
    render(
      <QuotaMetrics
        snapshot={{ ...success, status: "stale", updatedAt: "2020-01-01T00:00:00Z", message: "Network unavailable" }}
        language="en"
        onRefresh={refresh}
      />,
    );
    expect(screen.getByText("Quota data expired")).toBeInTheDocument();
    expect(screen.queryByText("74")).not.toBeInTheDocument();
    screen.getByRole("button", { name: "Refresh" }).click();
    expect(refresh).toHaveBeenCalledOnce();
  });

  it("shows a weekly-only payload without inventing a 5-hour window", () => {
    render(<QuotaMetrics snapshot={{ ...success, shortWindow: null }} language="en" />);
    expect(screen.getByText("Weekly quota")).toBeInTheDocument();
    expect(screen.getByText("8")).toBeInTheDocument();
    expect(screen.queryByText("5-hour quota")).not.toBeInTheDocument();
    expect(screen.queryByText("Some quota data is unavailable")).not.toBeInTheDocument();
    expect(screen.queryByText("—")).not.toBeInTheDocument();
  });

  it("keeps the historical 5-hour-only payload compatible", () => {
    render(<QuotaMetrics snapshot={{ ...success, weeklyWindow: null }} language="en" />);
    expect(screen.getByText("5-hour quota")).toBeInTheDocument();
    expect(screen.getByText("74")).toBeInTheDocument();
    expect(screen.queryByText("Weekly quota")).not.toBeInTheDocument();
  });

  it("shows stepped expiration warnings for every reset credit", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-07-14T12:00:00Z"));
    render(
      <QuotaMetrics
        snapshot={{
          ...success,
          resetCredits: 3,
          resetCreditExpiresAt: [
            "2026-07-17T12:00:00Z",
            "2026-07-15T12:00:00Z",
            "2026-07-16T12:00:00Z",
          ],
        }}
        language="zh-CN"
      />,
    );
    expect(screen.getByTitle(/最早.*1 天内到期，请及时使用/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "查看" }));
    expect(screen.getByText("重置卡到期详情")).toBeInTheDocument();
    expect(screen.getByText("剩 3 天，记得使用").closest(".credit-item")).toHaveClass("credit-item--soon");
    expect(screen.getByText("仅剩 2 天，建议尽快使用").closest(".credit-item")).toHaveClass("credit-item--warning");
    expect(screen.getByText("1 天内到期，请及时使用").closest(".credit-item")).toHaveClass("credit-item--critical");
  });

  it("does not misreport a transient auth read failure as signed out", () => {
    render(
      <QuotaMetrics
        snapshot={{
          ...success,
          status: "unavailable",
          shortWindow: null,
          weeklyWindow: null,
          message: "Codex login data is temporarily unavailable.",
        }}
        language="zh-CN"
      />,
    );
    expect(screen.getByText("Codex 登录文件暂时无法读取，应用将自动重试。")).toBeInTheDocument();
    expect(screen.queryByText("请先登录 Codex")).not.toBeInTheDocument();
  });

  it("treats API-key auth as usage based instead of signed out", () => {
    render(
      <QuotaMetrics
        snapshot={{
          ...success,
          authMode: "api_key",
          plan: "API",
          shortWindow: null,
          weeklyWindow: null,
          resetCredits: null,
          apiUsage: {
            status: "not_configured",
            today: { inputTokens: null, cachedInputTokens: null, outputTokens: null, requests: null, costUsd: null },
            monthToDate: { inputTokens: null, cachedInputTokens: null, outputTokens: null, requests: null, costUsd: null },
            updatedAt: "2026-07-15T00:00:00Z",
            message: null,
          },
        }}
        language="en"
      />,
    );
    expect(screen.getByText("Usage-based API account")).toBeInTheDocument();
    expect(screen.getByText(/5-hour and weekly quotas do not apply/)).toBeInTheDocument();
    expect(screen.getByText(/OPENAI_ADMIN_KEY/)).toBeInTheDocument();
    expect(screen.queryByText("Sign in to Codex")).not.toBeInTheDocument();
    expect(screen.queryByText("5-hour quota")).not.toBeInTheDocument();
  });

  it("shows only real mock organization totals and labels cost as non-balance data", () => {
    render(
      <QuotaMetrics
        snapshot={{
          ...success,
          authMode: "api_key",
          plan: "API",
          shortWindow: null,
          weeklyWindow: null,
          resetCredits: null,
          apiUsage: {
            status: "ok",
            today: { inputTokens: 400, cachedInputTokens: 100, outputTokens: 80, requests: 2, costUsd: 0.25 },
            monthToDate: { inputTokens: 12000, cachedInputTokens: 3000, outputTokens: 2400, requests: 35, costUsd: 7.5 },
            updatedAt: new Date().toISOString(),
            message: null,
          },
        }}
        language="en"
      />,
    );
    expect(screen.getByText("$7.50")).toBeInTheDocument();
    expect(screen.getByText("$0.25")).toBeInTheDocument();
    expect(screen.getByText("12K")).toBeInTheDocument();
    expect(screen.getByText("2.4K")).toBeInTheDocument();
    expect(screen.getByText(/not prepaid balance/)).toBeInTheDocument();
  });
});
