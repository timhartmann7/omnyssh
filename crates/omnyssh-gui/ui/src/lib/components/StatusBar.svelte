<script lang="ts">
  // Bottom region (tech-gui.md §2): context + host summary, and where background
  // errors surface (§3.5). The summary counts total / online / alert / offline
  // (§4.1); colour lives only in the status dots, per the brandbook.
  import { lastError } from '$lib/stores/notifications';
  import { hostSummary } from '$lib/stores/hostSummary';
  import { StatusDot } from '$lib/theme';
</script>

<footer
  class="col-span-2 col-start-1 row-start-2 flex items-center justify-between gap-4 border-t border-default bg-surface px-5 py-2 text-xs text-muted"
>
  {#if $lastError}
    <!-- A long error (a hint to act on) is cut to one line; hover shows all of it. -->
    <span class="min-w-0 truncate text-status-crit" title={$lastError}>{$lastError}</span>
  {:else}
    <span class="min-w-0 truncate">Ready</span>
  {/if}
  <div class="flex shrink-0 items-center gap-3">
    <span>{$hostSummary.total} {$hostSummary.total === 1 ? 'host' : 'hosts'}</span>
    <span class="flex items-center gap-1.5">
      <StatusDot status="ok" label="online" />{$hostSummary.online} online
    </span>
    <span class="flex items-center gap-1.5">
      <StatusDot status="warn" label="alert" />{$hostSummary.alert} alert
    </span>
    <span class="flex items-center gap-1.5">
      <StatusDot status="off" label="offline" />{$hostSummary.offline} offline
    </span>
  </div>
</footer>
