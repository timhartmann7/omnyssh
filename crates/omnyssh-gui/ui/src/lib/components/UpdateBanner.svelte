<script lang="ts">
  // Update banner (tech-gui.md §4.3). Floats above the status bar whenever an update is
  // available — from the startup `update-available` event or a manual check. It opens the
  // release page rather than self-updating: `plugins.updater` still ships with no endpoints
  // (§3.7, Stage 5), so `install_update` cannot succeed on any platform, and a button that
  // only ever reports a failure is worse than a download link. Skip persists the version to
  // the shared config so it is never offered again; Dismiss hides it for this session only.
  import { Icon } from '$lib/theme';
  import { availableUpdate, dismissUpdate } from '$lib/stores/update';
  import { loadUpdateConfig, saveUpdateConfig } from '$lib/ipc/commands';
  import { openExternal } from '$lib/ipc/openExternal';
  import { lastError } from '$lib/stores/notifications';
  import { t } from '$lib/i18n';

  const message = (e: unknown): string => (e instanceof Error ? e.message : String(e));
  let busy = $state(false);

  async function download(url: string): Promise<void> {
    try {
      await openExternal(url);
    } catch (e) {
      lastError.set(message(e));
    }
  }

  // Persist the skip against the current config so `check_on_startup` is preserved.
  async function skip(version: string): Promise<void> {
    busy = true;
    try {
      const config = await loadUpdateConfig();
      await saveUpdateConfig({ ...config, skipVersion: version });
      dismissUpdate();
    } catch (e) {
      lastError.set(message(e));
    } finally {
      busy = false;
    }
  }

  const action =
    'rounded-full px-3 py-1.5 text-sm transition disabled:opacity-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus';
</script>

{#if $availableUpdate}
  {@const info = $availableUpdate}
  <div class="pointer-events-none fixed inset-x-0 bottom-14 z-40 flex justify-center px-4">
    <div
      class="pointer-events-auto flex w-full max-w-xl items-center gap-3 rounded-2xl border border-default bg-surface-raised px-4 py-3 shadow-soft"
      role="status"
    >
      <span class="shrink-0 text-muted"><Icon name="download" size={18} /></span>
      <div class="min-w-0">
        <p class="text-sm font-medium">{$t('update_banner.available', { version: info.version })}</p>
        <p class="truncate text-xs text-muted">{$t('update_banner.desc')}</p>
      </div>
      <div class="ml-auto flex shrink-0 items-center gap-1.5">
        <button
          type="button"
          class="{action} bg-accent text-accent-fg hover:opacity-90"
          disabled={busy}
          onclick={() => download(info.url)}
        >
          {$t('update_banner.download')}
        </button>
        <button
          type="button"
          class="{action} text-muted hover:bg-surface-inset hover:text-fg"
          disabled={busy}
          onclick={() => skip(info.version)}
        >
          {$t('update_banner.skip')}
        </button>
        <button
          type="button"
          class="grid h-8 w-8 place-items-center rounded-full text-muted transition hover:bg-surface-inset hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus"
          title={$t('update_banner.dismiss')}
          aria-label={$t('update_banner.dismiss')}
          onclick={dismissUpdate}
        >
          <Icon name="close" size={15} />
        </button>
      </div>
    </div>
  </div>
{/if}
