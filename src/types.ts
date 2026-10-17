export type ProviderId = "codex" | "claude";
export type SnapshotStatus = "ok" | "stale" | "loading" | "unavailable" | "signed_out";
export type Language = "zh-CN" | "en";
export type QuotaTier = "unknown" | "healthy" | "caution" | "critical";
export type DesktopView = "widget" | "panel";

export interface UsageWindow {
  remainingPercent: number;
  resetsAt: string | null;
  windowSeconds: number;
}

export interface ProviderSnapshot {
  provider: ProviderId;
  displayName: string;
  plan: string | null;
  shortWindow: UsageWindow | null;
  weeklyWindow: UsageWindow | null;
  resetCredits: number | null;
  resetCreditExpiresAt?: string[];
  updatedAt: string;
  status: SnapshotStatus;
  message: string | null;
}

export interface WidgetPreferences {
  locked: boolean;
  alwaysOnTop: boolean;
  pinnedProvider: ProviderId | null;
  autoRotateSeconds: number;
  quotaRefreshIntervalSeconds: number | null;
  language: Language;
}

export interface SnapshotState {
  snapshots: ProviderSnapshot[];
  refreshing: boolean;
  revision: number;
  lastAttemptAt: string | null;
  lastSuccessAt: string | null;
  nextRefreshAt: string | null;
}

export interface DesktopState extends SnapshotState {
  preferences: WidgetPreferences;
  widgetVisible: boolean;
  autostartEnabled: boolean;
}

export type SnapshotEventPayload = ProviderSnapshot[] | Partial<SnapshotState> & {
  snapshots: ProviderSnapshot[];
};

export type UsageStatsStatus = "ok" | "empty" | "unavailable";

export interface ModelTokenUsage {
  model: string;
  inputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  reasoningOutputTokens: number;
  totalTokens: number;
}

export interface DailyTokenUsage {
  date: string;
  inputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  reasoningOutputTokens: number;
  totalTokens: number;
  sessionCount: number;
  models: ModelTokenUsage[];
}

export interface UsageStats {
  status: UsageStatsStatus;
  generatedAt: string;
  firstActivityDate: string | null;
  lastActivityDate: string | null;
  inputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  reasoningOutputTokens: number;
  totalTokens: number;
  sessionCount: number;
  activeDays: number;
  currentStreak: number;
  longestStreak: number;
  indexedFiles: number;
  skippedFiles: number;
  models: ModelTokenUsage[];
  daily: DailyTokenUsage[];
  official?: OfficialUsage | null;
}

export interface OfficialUsageBucket { date: string; totalTokens: number }
export interface OfficialUsage {
  status: "ok" | "unavailable" | "signed_out" | "not_applicable";
  generatedAt: string;
  asOf?: string | null;
  totalTokens: number | null;
  peakDailyTokens: number | null;
  sessionCount: number | null;
  longestRunningTurnSeconds: number | null;
  currentStreak: number | null;
  longestStreak: number | null;
  daily: OfficialUsageBucket[];
  weekly: OfficialUsageBucket[];
  cumulative: OfficialUsageBucket[];
}
