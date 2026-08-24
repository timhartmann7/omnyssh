<script lang="ts">
  // Per-host snippet results (tech-gui.md §2.2), driven by the `snippetRun` store:
  // each target host shows pending → ok/failed with its output. Colour lives only in
  // the status dot, per the brandbook. Rendered whenever a run is active.
  import { StatusDot, Button, type Status } from '$lib/theme';
  import Modal from '$lib/components/Modal.svelte';
  import { snippetRun, clearRun, type SnippetResultEntry } from '$lib/stores/snippets';
  import { streamerMode, displayHostTitle } from '$lib/stores/streamer';
  import { t } from '$lib/i18n';

  function dot(entry: SnippetResultEntry): Status {
    if (entry.pending) return 'unknown';
    return entry.ok ? 'ok' : 'crit';
  }

  function state(entry: SnippetResultEntry): string {
    if (entry.pending) return 'running';
    return entry.ok ? 'ok' : 'failed';
  }
</script>

{#if $snippetRun}
  {@const run = $snippetRun}
  <Modal label={$t('snippet_results.title', { name: run.snippetName })} onClose={clearRun}>
    <header class="border-b border-default px-5 py-3.5">
      <h2 class="truncate text-sm font-semibold">{$t('snippet_results.title', { name: run.snippetName })}</h2>
    </header>

    <div class="min-h-0 flex-1 space-y-3 overflow-y-auto px-5 py-4">
      {#each run.entries as entry (entry.hostName)}
        <div class="rounded-lg border border-default">
          <div class="flex items-center gap-2.5 border-b border-default px-3 py-2">
            <StatusDot status={dot(entry)} label="{displayHostTitle(entry.hostName, $streamerMode)} {state(entry)}" />
            <span class="min-w-0 flex-1 truncate text-sm font-medium">{displayHostTitle(entry.hostName, $streamerMode)}</span>
            <span class="shrink-0 text-[11px] uppercase tracking-wider text-faint">{state(entry)}</span>
          </div>
          {#if entry.pending}
            <p class="px-3 py-2 text-xs text-faint">{$t('snippet_results.running')}</p>
          {:else if entry.output.trim()}
            <pre
              class="max-h-52 select-text overflow-auto whitespace-pre-wrap break-words px-3 py-2 font-mono text-xs {entry.ok
                ? 'text-muted'
                : 'text-status-crit'}">{entry.output}</pre>
          {:else}
            <p class="px-3 py-2 text-xs text-faint">{$t('snippet_results.no_output')}</p>
          {/if}
        </div>
      {/each}
    </div>

    <footer class="flex justify-end border-t border-default px-5 py-3">
      <Button variant="secondary" onclick={clearRun}>{$t('snippet_results.close')}</Button>
    </footer>
  </Modal>
{/if}
