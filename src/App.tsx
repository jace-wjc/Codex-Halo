import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { FloatingWidget } from "./components/FloatingWidget";
import { MenuPanel } from "./components/MenuPanel";
import {
  defaultPreferences,
  getAppState,
  getDesktopView,
  getUsageStats,
  listenDesktopEvents,
  quitApp,
  refreshSnapshots,
  setAlwaysOnTop,
  setAutostart,
  setClickThrough,
  setLanguage,
  setQuotaRefreshInterval,
  setWidgetVisible,
} from "./lib/bridge";
import { copy, nextLanguage, normalizeLanguage } from "./lib/i18n";
import { emptySnapshot, mergeSnapshots } from "./lib/snapshots";
import type { DesktopState, DesktopView, SnapshotState, UsageStats, WidgetPreferences } from "./types";

const INITIAL_STATE: DesktopState = {
  snapshots: [],
  preferences: defaultPreferences,
  widgetVisible: true,
  autostartEnabled: false,
  refreshing: false,
  revision: 0,
  lastAttemptAt: null,
  lastSuccessAt: null,
  nextRefreshAt: null,
};

const USAGE_STATS_REVALIDATE_MS = 60_000;

export default function App() {
  const [desktopState, setDesktopState] = useState<DesktopState>(INITIAL_STATE);
  const [view, setView] = useState<DesktopView>(() => new URLSearchParams(window.location.search).get("view") === "panel" ? "panel" : "widget");
  const [initialized, setInitialized] = useState(false);
  const [manualRefreshing, setManualRefreshing] = useState(false);
  const [pendingAction, setPendingAction] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [usageStats, setUsageStats] = useState<UsageStats | null>(null);
  const [usageLoading, setUsageLoading] = useState(false);
  const [usageFailed, setUsageFailed] = useState(false);
  const usageLoadingRef = useRef(false);
  const usageLoadedAtRef = useRef(0);
  const language = normalizeLanguage(desktopState.preferences.language);
  const t = copy[language];

  const applySnapshotState = useCallback((incoming: SnapshotState) => {
    setDesktopState((current) => {
      if (incoming.revision > 0 && incoming.revision < current.revision) return current;
      return {
        ...current,
        snapshots: mergeSnapshots(current.snapshots, incoming.snapshots),
        refreshing: incoming.refreshing,
        revision: Math.max(current.revision, incoming.revision),
        lastAttemptAt: incoming.lastAttemptAt,
        lastSuccessAt: incoming.lastSuccessAt,
        nextRefreshAt: incoming.nextRefreshAt,
      };
    });
  }, []);

  useEffect(() => {
    void getDesktopView().then(setView).catch(() => undefined);
  }, []);

  useEffect(() => {
    document.documentElement.dataset.view = view;
    return () => { delete document.documentElement.dataset.view; };
  }, [view]);

  useEffect(() => {
    let cancelled = false;
    let cleanup: () => void = () => undefined;
    const observed = {
      snapshots: false,
      preferences: false,
      widgetVisibility: false,
      autostart: false,
    };

    void (async () => {
      try {
        const unlisten = await listenDesktopEvents({
          onSnapshots: (value) => {
            observed.snapshots = true;
            applySnapshotState(value);
          },
          onPreferences: (preferences) => {
            observed.preferences = true;
            setDesktopState((current) => ({ ...current, preferences }));
          },
          onWidgetVisibility: (widgetVisible) => {
            observed.widgetVisibility = true;
            setDesktopState((current) => ({ ...current, widgetVisible }));
          },
          onAutostart: (autostartEnabled) => {
            observed.autostart = true;
            setDesktopState((current) => ({ ...current, autostartEnabled }));
          },
        });
        if (cancelled) unlisten(); else cleanup = unlisten;
      } catch {
        if (!cancelled) setNotice(t.settingsActionFailed);
      }

      try {
        const value = await getAppState();
        if (cancelled) return;
        setDesktopState((current) => {
          const keepObservedSnapshotState = observed.snapshots
            || (value.revision > 0 && value.revision < current.revision);
          return {
            ...value,
            snapshots: keepObservedSnapshotState
              ? current.snapshots
              : mergeSnapshots(current.snapshots, value.snapshots),
            preferences: observed.preferences ? current.preferences : value.preferences,
            widgetVisible: observed.widgetVisibility ? current.widgetVisible : value.widgetVisible,
            autostartEnabled: observed.autostart ? current.autostartEnabled : value.autostartEnabled,
            refreshing: keepObservedSnapshotState ? current.refreshing : value.refreshing,
            revision: Math.max(current.revision, value.revision),
            lastAttemptAt: keepObservedSnapshotState ? current.lastAttemptAt : value.lastAttemptAt,
            lastSuccessAt: keepObservedSnapshotState ? current.lastSuccessAt : value.lastSuccessAt,
            nextRefreshAt: keepObservedSnapshotState ? current.nextRefreshAt : value.nextRefreshAt,
          };
        });
      } catch {
        if (!cancelled) {
          setDesktopState((current) => ({ ...current, snapshots: [emptySnapshot("unavailable", null)] }));
        }
      } finally {
        if (!cancelled) setInitialized(true);
      }
    })();

    return () => { cancelled = true; cleanup(); };
  }, [applySnapshotState]);

  const handleRefresh = useCallback(async () => {
    if (desktopState.refreshing || manualRefreshing) return;
    setManualRefreshing(true);
    setNotice(null);
    try {
      const snapshots = await refreshSnapshots();
      setDesktopState((current) => ({ ...current, snapshots: mergeSnapshots(current.snapshots, snapshots) }));
    } catch {
      setNotice(t.refreshFailed);
    } finally {
      setManualRefreshing(false);
    }
  }, [desktopState.refreshing, manualRefreshing, t.refreshFailed]);

  const handleUsageStats = useCallback(async (force = false) => {
    if (usageLoadingRef.current || (!force && Date.now() - usageLoadedAtRef.current < USAGE_STATS_REVALIDATE_MS)) return;
    usageLoadingRef.current = true;
    setUsageLoading(true);
    setUsageFailed(false);
    try {
      setUsageStats(await getUsageStats(force));
      usageLoadedAtRef.current = Date.now();
    } catch {
      setUsageFailed(true);
    } finally {
      usageLoadingRef.current = false;
      setUsageLoading(false);
    }
  }, []);

  const handleRefreshAll = useCallback(async () => {
    await Promise.allSettled([handleRefresh(), handleUsageStats(true)]);
  }, [handleRefresh, handleUsageStats]);

  const runPreferenceAction = useCallback(async (
    key: string,
    operation: () => Promise<WidgetPreferences>,
  ) => {
    if (pendingAction) return;
    setPendingAction(key);
    setNotice(null);
    try {
      const preferences = await operation();
      setDesktopState((current) => ({ ...current, preferences: { ...defaultPreferences, ...preferences } }));
    } catch {
      setNotice(t.settingsActionFailed);
    } finally {
      setPendingAction(null);
    }
  }, [pendingAction, t.settingsActionFailed]);

  const handleWidgetVisibility = useCallback(async () => {
    if (pendingAction) return;
    setPendingAction("widget");
    setNotice(null);
    try {
      const widgetVisible = await setWidgetVisible(!desktopState.widgetVisible);
      setDesktopState((current) => ({ ...current, widgetVisible }));
    } catch {
      setNotice(t.windowActionFailed);
    } finally {
      setPendingAction(null);
    }
  }, [desktopState.widgetVisible, pendingAction, t.windowActionFailed]);

  const handleAutostart = useCallback(async () => {
    if (pendingAction) return;
    setPendingAction("autostart");
    setNotice(null);
    try {
      const autostartEnabled = await setAutostart(!desktopState.autostartEnabled);
      setDesktopState((current) => ({ ...current, autostartEnabled }));
    } catch {
      setNotice(t.settingsActionFailed);
    } finally {
      setPendingAction(null);
    }
  }, [desktopState.autostartEnabled, pendingAction, t.settingsActionFailed]);

  const snapshot = useMemo(() => {
    const selected = desktopState.preferences.pinnedProvider
      ? desktopState.snapshots.find((item) => item.provider === desktopState.preferences.pinnedProvider)
      : desktopState.snapshots[0];
    if (selected) return selected;
    return emptySnapshot(initialized && !desktopState.refreshing ? "unavailable" : "loading", null);
  }, [desktopState.preferences.pinnedProvider, desktopState.refreshing, desktopState.snapshots, initialized]);
  const refreshing = desktopState.refreshing || manualRefreshing;

  if (view === "panel") {
    return (
      <MenuPanel
        snapshot={snapshot}
        preferences={desktopState.preferences}
        widgetVisible={desktopState.widgetVisible}
        autostartEnabled={desktopState.autostartEnabled}
        refreshing={refreshing}
        usageStats={usageStats}
        usageLoading={usageLoading}
        usageFailed={usageFailed}
        pendingAction={pendingAction}
        nextRefreshAt={desktopState.nextRefreshAt}
        notice={notice}
        onRefresh={() => { void handleRefreshAll(); }}
        onLoadUsageStats={() => { void handleUsageStats(false); }}
        onToggleWidget={() => { void handleWidgetVisibility(); }}
        onToggleAlwaysOnTop={() => { void runPreferenceAction("alwaysOnTop", () => setAlwaysOnTop(!desktopState.preferences.alwaysOnTop)); }}
        onToggleAutostart={() => { void handleAutostart(); }}
        onToggleLanguage={() => { void runPreferenceAction("language", () => setLanguage(nextLanguage(language))); }}
        onSetRefreshInterval={(seconds) => { void runPreferenceAction("refreshInterval", () => setQuotaRefreshInterval(seconds)); }}
        onToggleClickThrough={() => { void runPreferenceAction("clickThrough", () => setClickThrough(!desktopState.preferences.locked)); }}
        onQuit={() => { void quitApp().catch(() => setNotice(t.windowActionFailed)); }}
      />
    );
  }

  return (
    <FloatingWidget
      snapshot={snapshot}
      preferences={desktopState.preferences}
      refreshing={refreshing}
      notice={notice}
      onRefresh={() => { void handleRefresh(); }}
      onToggleAlwaysOnTop={() => { void runPreferenceAction("alwaysOnTop", () => setAlwaysOnTop(!desktopState.preferences.alwaysOnTop)); }}
    />
  );
}
