<script lang="ts">
  // Bottom region (tech-gui.md §2): context + host summary, and where background
  // errors surface (§3.5). The summary counts total / online / alert / offline
  // (§4.1); colour lives only in the status dots, per the brandbook.
  import { lastError } from '$lib/stores/notifications';
  import { hostSummary } from '$lib/stores/hostSummary';
  import { streamerMode } from '$lib/stores/streamer';
  import { StatusDot } from '$lib/theme';
  import { t } from '$lib/i18n';
</script>

<footer
  class="col-span-2 col-start-1 row-start-2 flex items-center justify-between gap-4 border-t border-default bg-surface px-5 py-2 text-xs text-muted"
>
  {#if $lastError}
    <span class="min-w-0 truncate text-status-crit">{$lastError}</span>
  {:else if $streamerMode}
    <span class="min-w-0 truncate text-accent">{$t('statusbar.streamer_active')}</span>
  {:else}
    <span class="min-w-0 truncate">{$t('statusbar.ready')}</span>
  {/if}
  <div class="flex shrink-0 items-center gap-3">
    <span>{$hostSummary.total} {$hostSummary.total === 1 ? $t('statusbar.host_singular') : $t('statusbar.host_plural')}</span>
    <span class="flex items-center gap-1.5">
      <StatusDot status="ok" label={$t('statusbar.online')} />{$hostSummary.online} {$t('statusbar.online')}
    </span>
    <span class="flex items-center gap-1.5">
      <StatusDot status="warn" label={$t('statusbar.alert')} />{$hostSummary.alert} {$t('statusbar.alert')}
    </span>
    <span class="flex items-center gap-1.5">
      <StatusDot status="off" label={$t('statusbar.offline')} />{$hostSummary.offline} {$t('statusbar.offline')}
    </span>
  </div>
</footer>
