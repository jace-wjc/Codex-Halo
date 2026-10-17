import { describe, expect, it } from "vitest";
import type { ProviderSnapshot } from "../types";
import { emptySnapshot, mergeSnapshots } from "./snapshots";

const success: ProviderSnapshot = {
  provider: "codex",
  displayName: "CODEX",
  plan: "PRO",
  shortWindow: { remainingPercent: 74, resetsAt: "2026-07-07T02:00:00Z", windowSeconds: 18_000 },
  weeklyWindow: { remainingPercent: 42, resetsAt: "2026-07-10T00:00:00Z", windowSeconds: 604_800 },
  resetCredits: 1,
  updatedAt: "2026-07-07T00:00:00Z",
  status: "ok",
  message: null,
};

describe("snapshot failure handling", () => {
  it("retains the last successful values during a transient failure", () => {
    const failure: ProviderSnapshot = { ...success, shortWindow: null, weeklyWindow: null, status: "unavailable", message: "Network unavailable", updatedAt: "2026-07-07T01:00:00Z" };
    expect(mergeSnapshots([success], [failure])[0]).toEqual({ ...success, status: "stale", message: "Network unavailable" });
  });

  it("shows a failure when no successful snapshot exists", () => {
    const signedOut: ProviderSnapshot = { ...success, shortWindow: null, weeklyWindow: null, status: "signed_out", message: "Please sign in" };
    expect(mergeSnapshots([], [signedOut])[0].status).toBe("signed_out");
  });

  it("does not hide an expired login behind stale quota data", () => {
    const signedOut: ProviderSnapshot = { ...success, shortWindow: null, weeklyWindow: null, status: "signed_out", message: "Please sign in" };
    expect(mergeSnapshots([success], [signedOut])[0].status).toBe("signed_out");
  });

  it("replaces stale data after recovery", () => {
    expect(mergeSnapshots([{ ...success, status: "stale" }], [{ ...success, shortWindow: { ...success.shortWindow!, remainingPercent: 88 } }])[0].shortWindow?.remainingPercent).toBe(88);
  });

  it("keeps explicit partial data instead of replacing it with older values", () => {
    const partial: ProviderSnapshot = {
      ...success,
      status: "unavailable",
      shortWindow: null,
      weeklyWindow: { ...success.weeklyWindow!, remainingPercent: 31 },
      message: "The 5h window is unavailable",
    };
    expect(mergeSnapshots([success], [partial])[0]).toEqual(partial);
  });

  it("creates an unavailable shell without quota values", () => {
    const value = emptySnapshot("unavailable", null);
    expect(value.shortWindow).toBeNull();
    expect(value.weeklyWindow).toBeNull();
    expect(value.resetCredits).toBeNull();
  });

  it("keeps real API totals as explicitly stale after a transient Admin API failure", () => {
    const apiSuccess: ProviderSnapshot = {
      ...success,
      authMode: "api_key",
      plan: "API",
      shortWindow: null,
      weeklyWindow: null,
      resetCredits: null,
      apiUsage: {
        status: "ok",
        today: { inputTokens: 100, cachedInputTokens: 20, outputTokens: 10, requests: 1, costUsd: 0.1 },
        monthToDate: { inputTokens: 1000, cachedInputTokens: 200, outputTokens: 100, requests: 10, costUsd: 1.5 },
        updatedAt: success.updatedAt,
        message: null,
      },
    };
    const failure: ProviderSnapshot = {
      ...apiSuccess,
      status: "unavailable",
      message: "Organization API unavailable",
      apiUsage: {
        status: "unavailable",
        today: { inputTokens: null, cachedInputTokens: null, outputTokens: null, requests: null, costUsd: null },
        monthToDate: { inputTokens: null, cachedInputTokens: null, outputTokens: null, requests: null, costUsd: null },
        updatedAt: "2026-07-07T01:00:00Z",
        message: "Organization API unavailable",
      },
    };

    const merged = mergeSnapshots([apiSuccess], [failure])[0];
    expect(merged.status).toBe("stale");
    expect(merged.apiUsage?.status).toBe("stale");
    expect(merged.apiUsage?.monthToDate.costUsd).toBe(1.5);
  });
});
