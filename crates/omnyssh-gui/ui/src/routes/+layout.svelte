<script lang="ts">
  import '../app.css';
  import { onMount } from 'svelte';
  import { startEventBridge } from '$lib/ipc/subscribe';
  import { reloadHosts, refreshMetrics, setTrayBehavior } from '$lib/ipc/commands';
  import { theme } from '$lib/stores/theme';
  import { sidebarCollapsed } from '$lib/stores/ui';
  import { streamerMode } from '$lib/stores/streamer';
  import { refreshInterval, driveMetricsRefresh } from '$lib/stores/settings';
  import { trayBehavior, driveTray } from '$lib/stores/tray';
  import { dashboardView } from '$lib/stores/dashboardView';
  import { lastError } from '$lib/stores/notifications';

  let { children } = $props();

  onMount(() => {
    let stop: (() => void) | undefined;
    let disposed = false;
    // Reconcile the persisted prefs with their canonical tauri-plugin-store values;
    // the synchronous localStorage mirrors already seeded the first paint (§5.1, §2).
    void theme.hydrate();
    void sidebarCollapsed.hydrate();
    void streamerMode.hydrate();
    void refreshInterval.hydrate();
    void trayBehavior.hydrate();
    void dashboardView.hydrate();
    // Force a metric refresh on the user's interval; re-arms when the interval changes.
    const stopRefresh = driveMetricsRefresh(() => {
      void refreshMetrics().catch(() => {});
    });
    // The backend knows nothing of the tray until told, so a close before this
    // lands still quits — never a hidden window with no icon to bring it back.
    const stopTray = driveTray(
      (b) => setTrayBehavior(b.minimizeToTray, b.closeToTray),
      (message) => lastError.set(message)
    );
    // No-op outside Tauri (e.g. a plain `vite preview`); the shell still mounts.
    // Dispose even if the layout unmounts before the subscription resolves. Start
    // the pollers only once listeners are attached, so no status event is missed.
    startEventBridge()
      .then((off) => {
        if (disposed) return off();
        stop = off;
        reloadHosts().catch((err) => lastError.set(err instanceof Error ? err.message : String(err)));
      })
      .catch(() => {});
    return () => {
      disposed = true;
      stop?.();
      stopRefresh();
      stopTray();
    };
  });
</script>

{@render children()}
