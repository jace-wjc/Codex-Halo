import type {
  DesktopState,
  DesktopView,
  Language,
  ProviderSnapshot,
  SnapshotEventPayload,
  SnapshotState,
  UsageStats,
  WidgetPreferences,
} from "../types";

export const defaultPreferences: WidgetPreferences = {
  locked: false,
  alwaysOnTop: true,
  pinnedProvider: null,
  autoRotateSeconds: 12,
  quotaRefreshIntervalSeconds: 300,
  language: "en",
};

const emptySnapshotState = (): SnapshotState => ({
  snapshots: [],
  refreshing: false,
  revision: 0,
  lastAttemptAt: null,
  lastSuccessAt: null,
  nextRefreshAt: null,
});

export const isTauri = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

function requireDesktop(): void {
  if (!isTauri()) throw new Error("Codex Halo desktop bridge is unavailable.");
}

async function invokeDesktop<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  requireDesktop();
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(command, args);
}

function normalizeSnapshotState(payload: SnapshotEventPayload): SnapshotState {
  if (Array.isArray(payload)) return { ...emptySnapshotState(), snapshots: payload };
  return {
    snapshots: payload.snapshots,
    refreshing: payload.refreshing ?? false,
    revision: payload.revision ?? 0,
    lastAttemptAt: payload.lastAttemptAt ?? null,
    lastSuccessAt: payload.lastSuccessAt ?? null,
    nextRefreshAt: payload.nextRefreshAt ?? null,
  };
}

function normalizeDesktopState(value: Partial<DesktopState> & Pick<DesktopState, "snapshots" | "preferences">): DesktopState {
  return {
    snapshots: value.snapshots,
    preferences: { ...defaultPreferences, ...value.preferences },
    widgetVisible: value.widgetVisible ?? true,
    autostartEnabled: value.autostartEnabled ?? false,
    refreshing: value.refreshing ?? false,
    revision: value.revision ?? 0,
    lastAttemptAt: value.lastAttemptAt ?? null,
    lastSuccessAt: value.lastSuccessAt ?? null,
    nextRefreshAt: value.nextRefreshAt ?? null,
  };
}

export async function getDesktopView(): Promise<DesktopView> {
  const queryView = new URLSearchParams(window.location.search).get("view");
  if (queryView === "panel") return "panel";
  if (!isTauri()) return "widget";
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow().label === "panel" ? "panel" : "widget";
}

export async function getAppState(): Promise<DesktopState> {
  requireDesktop();
  try {
    const value = await invokeDesktop<DesktopState>("get_app_state");
    return normalizeDesktopState(value);
  } catch {
    // Compatibility with v0.1.x while the native state coordinator is unavailable.
    const [snapshots, preferences] = await Promise.all([
      invokeDesktop<ProviderSnapshot[]>("get_snapshots"),
      invokeDesktop<WidgetPreferences>("get_preferences"),
    ]);
    let autostartEnabled = false;
    try {
      autostartEnabled = await invokeDesktop<boolean>("get_autostart");
    } catch {
      // Older builds expose autostart only through their native tray menu.
    }
    return normalizeDesktopState({ snapshots, preferences, autostartEnabled });
  }
}

export async function fetchSnapshots(force = false): Promise<ProviderSnapshot[]> {
  return invokeDesktop<ProviderSnapshot[]>(force ? "refresh_snapshots" : "get_snapshots");
}

export async function refreshSnapshots(): Promise<ProviderSnapshot[]> {
  return fetchSnapshots(true);
}

export async function getUsageStats(force = false): Promise<UsageStats> {
  return invokeDesktop<UsageStats>("get_usage_stats", { force });
}

export async function getPreferences(): Promise<WidgetPreferences> {
  if (!isTauri()) return defaultPreferences;
  return invokeDesktop<WidgetPreferences>("get_preferences");
}

export async function updatePreferences(value: WidgetPreferences): Promise<void> {
  await invokeDesktop("set_preferences", { preferences: value });
}

export async function setLanguage(language: Language): Promise<WidgetPreferences> {
  try {
    return await invokeDesktop<WidgetPreferences>("set_language", { language });
  } catch {
    const preferences = { ...(await getPreferences()), language };
    await updatePreferences(preferences);
    return preferences;
  }
}

export async function setQuotaRefreshInterval(intervalSeconds: number | null): Promise<WidgetPreferences> {
  return invokeDesktop<WidgetPreferences>("set_quota_refresh_interval", { intervalSeconds });
}

export async function setClickThrough(locked: boolean): Promise<WidgetPreferences> {
  return invokeDesktop<WidgetPreferences>("set_widget_locked", { locked });
}

export async function setAlwaysOnTop(alwaysOnTop: boolean): Promise<WidgetPreferences> {
  return invokeDesktop<WidgetPreferences>("set_widget_always_on_top", { alwaysOnTop });
}

export async function setWidgetVisible(visible: boolean): Promise<boolean> {
  return invokeDesktop<boolean>("set_widget_visible", { visible });
}

export async function getAutostart(): Promise<boolean> {
  return invokeDesktop<boolean>("get_autostart");
}

export async function setAutostart(enabled: boolean): Promise<boolean> {
  return invokeDesktop<boolean>("set_autostart", { enabled });
}

export async function quitApp(): Promise<void> {
  await invokeDesktop("quit_app");
}

export async function startDragging(): Promise<void> {
  if (!isTauri()) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const { invoke } = await import("@tauri-apps/api/core");
  const currentWindow = getCurrentWindow();
  await invoke("begin_widget_drag");
  await currentWindow.startDragging();
  let previous = await currentWindow.outerPosition();
  let stableTicks = 0;
  let attempts = 0;
  const finishWhenStable = window.setInterval(() => {
    void currentWindow.outerPosition()
      .then((next) => {
        attempts += 1;
        const stable = Math.abs(next.x - previous.x) <= 1 && Math.abs(next.y - previous.y) <= 1;
        stableTicks = stable ? stableTicks + 1 : 0;
        previous = next;
        if (stableTicks >= 3 || attempts >= 25) {
          window.clearInterval(finishWhenStable);
          void invoke("finish_widget_drag").catch(() => undefined);
        }
      })
      .catch(() => {
        window.clearInterval(finishWhenStable);
        void invoke("finish_widget_drag").catch(() => undefined);
      });
  }, 80);
}

let widgetTransition: Promise<void> = Promise.resolve();

export function setWidgetExpanded(expanded: boolean): Promise<void> {
  if (!isTauri()) return Promise.resolve();
  const operation = async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    try {
      if (!expanded) {
        await invoke("collapse_widget");
        return;
      }
      const { currentMonitor } = await import("@tauri-apps/api/window");
      const monitor = await currentMonitor().catch(() => null);
      const workArea = monitor ? {
        position: { x: monitor.workArea.position.x, y: monitor.workArea.position.y },
        size: { width: monitor.workArea.size.width, height: monitor.workArea.size.height },
      } : null;
      await invoke("expand_widget", { workArea });
    } catch {
      const { getCurrentWindow, LogicalSize } = await import("@tauri-apps/api/window");
      await getCurrentWindow().setSize(new LogicalSize(expanded ? 344 : 100, expanded ? 364 : 100));
    }
  };
  const next = widgetTransition.then(operation, operation);
  widgetTransition = next.catch(() => undefined);
  return next;
}

export interface DesktopEventHandlers {
  onSnapshots: (value: SnapshotState) => void;
  onPreferences: (value: WidgetPreferences) => void;
  onWidgetVisibility: (visible: boolean) => void;
  onAutostart: (enabled: boolean) => void;
  onRefreshRequested?: () => void;
}

export async function listenDesktopEvents(handlers: DesktopEventHandlers): Promise<() => void> {
  if (!isTauri()) return () => undefined;
  const { listen } = await import("@tauri-apps/api/event");
  const unlisteners: Array<() => void> = [];
  try {
    unlisteners.push(await listen<SnapshotEventPayload>("snapshots-changed", (event) => {
      handlers.onSnapshots(normalizeSnapshotState(event.payload));
    }));
    unlisteners.push(await listen<WidgetPreferences>("preferences-changed", (event) => {
      handlers.onPreferences({ ...defaultPreferences, ...event.payload });
    }));
    unlisteners.push(await listen<boolean>("widget-visibility-changed", (event) => {
      handlers.onWidgetVisibility(event.payload);
    }));
    unlisteners.push(await listen<boolean>("autostart-changed", (event) => {
      handlers.onAutostart(event.payload);
    }));
    if (handlers.onRefreshRequested) {
      unlisteners.push(await listen("refresh-requested", handlers.onRefreshRequested));
    }
  } catch (error) {
    for (const unlisten of unlisteners) unlisten();
    throw error;
  }
  return () => { for (const unlisten of unlisteners) unlisten(); };
}
