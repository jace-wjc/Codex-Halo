import type { ProviderId, ProviderSnapshot, SnapshotStatus } from "../types";

function hasApiUsageValues(snapshot: ProviderSnapshot): boolean {
  const usage = snapshot.apiUsage;
  if (!usage) return false;
  return [usage.today, usage.monthToDate].some((period) => [
    period.inputTokens,
    period.cachedInputTokens,
    period.outputTokens,
    period.requests,
    period.costUsd,
  ].some((value) => typeof value === "number" && Number.isFinite(value)));
}

export function mergeSnapshots(current: ProviderSnapshot[], incoming: ProviderSnapshot[]): ProviderSnapshot[] {
  return incoming.map((next) => {
    if (next.status === "ok") return next;
    if (next.status === "signed_out") return next;
    if (next.shortWindow || next.weeklyWindow) return next;
    const previous = current.find((item) => {
      if (item.provider !== next.provider) return false;
      if (next.authMode && next.authMode !== "unknown" && item.authMode !== next.authMode) return false;
      return Boolean(item.shortWindow || item.weeklyWindow || hasApiUsageValues(item));
    });
    return previous
      ? {
          ...previous,
          status: "stale",
          message: next.message,
          updatedAt: previous.updatedAt,
          apiUsage: previous.apiUsage
            ? {
                ...previous.apiUsage,
                status: "stale",
                message: next.apiUsage?.message ?? next.message,
              }
            : previous.apiUsage,
        }
      : next;
  });
}

export function emptySnapshot(
  status: SnapshotStatus,
  message: string | null,
  provider: ProviderId = "codex",
): ProviderSnapshot {
  return {
    provider,
    displayName: provider === "codex" ? "CODEX" : provider.toUpperCase(),
    authMode: "unknown",
    plan: null,
    shortWindow: null,
    weeklyWindow: null,
    resetCredits: null,
    resetCreditExpiresAt: [],
    updatedAt: new Date().toISOString(),
    status,
    message,
    apiUsage: null,
  };
}
