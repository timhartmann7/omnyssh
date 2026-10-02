<script lang="ts">
  // Run dialog (tech-gui.md §2.2): collect any declared param values, pick one or
  // more hosts to run on (broadcast), then execute. Host references show their live
  // status dot via the shared server-state palette. Semantic tokens only.
  import { get } from 'svelte/store';
  import type { SnippetDto } from '$lib/bindings';
  import { Button, StatusDot, Icon } from '$lib/theme';
  import Modal from '$lib/components/Modal.svelte';
  import { hosts, firstOfEachName } from '$lib/stores/hosts';
  import { statuses } from '$lib/stores/statuses';
  import { streamerMode, displayHostname } from '$lib/stores/streamer';
  import { hostStatusDot } from '$lib/stores/palette';
  import { declaredParams } from './snippetForm';

  let {
    snippet,
    onExecute,
    onCancel
  }: {
    snippet: SnippetDto;
    onExecute: (hostNames: string[], params: Record<string, string>) => void;
    onCancel: () => void;
  } = $props();

  // Seeded once from `snippet`; the runner is remounted per open, so the prop never
  // changes under a live instance.
  // svelte-ignore state_referenced_locally
  const params = declaredParams(snippet);
  let values = $state<Record<string, string>>(Object.fromEntries(params.map((p) => [p, ''])));
  // A host-scoped snippet pre-selects its own host — but only if it is actually a
  // configured host, so we never seed a target the backend can't resolve (which would
  // strand the results panel on "Running…"). Broadcast still lets the user add more.
  // svelte-ignore state_referenced_locally
  let selected = $state<Set<string>>(
    new Set(
      snippet.scope === 'host' && snippet.host && get(hosts).some((h) => h.name === snippet.host)
        ? [snippet.host]
        : []
    )
  );

  // One row per name, the first: the backend runs a name on its first host, so a
  // second row would only toggle along with it.
  const choices = $derived(firstOfEachName($hosts));

  /** Focus the first param input on open for a keyboard-first run. */
  function autofocus(node: HTMLInputElement, active: boolean): void {
    if (active) node.focus();
  }

  function toggleHost(name: string): void {
    const next = new Set(selected);
    if (next.has(name)) next.delete(name);
    else next.add(name);
    selected = next;
  }

  function run(): void {
    if (selected.size === 0) return;
    onExecute([...selected], { ...values });
  }

  const field =
    'w-full rounded-lg bg-surface-inset px-3 py-2 text-sm text-fg outline-none ' +
    'focus-visible:ring-2 focus-visible:ring-focus placeholder:text-faint';
</script>

<Modal label="Run snippet" onClose={onCancel}>
  <header class="border-b border-default px-5 py-3.5">
    <h2 class="truncate text-sm font-semibold">Run “{snippet.name}”</h2>
    <p class="mt-0.5 truncate font-mono text-xs text-faint">{snippet.command}</p>
  </header>

  <div class="min-h-0 flex-1 space-y-4 overflow-y-auto px-5 py-4">
    {#if params.length}
      <div class="space-y-3">
        <h3 class="text-[11px] font-medium uppercase tracking-[0.18em] text-faint">Parameters</h3>
        <!-- Keyed by position: params come from snippets.toml unchanged and the core
             never dedups them, so a name key could throw each_key_duplicate. -->
        {#each params as name, i (i)}
          <label class="block space-y-1 text-xs font-medium text-muted">
            <span class="font-mono">{'{{'}{name}{'}}'}</span>
            <input use:autofocus={i === 0} bind:value={values[name]} class={field} placeholder={name} />
          </label>
        {/each}
      </div>
    {/if}

    <div class="space-y-2">
      <h3 class="text-[11px] font-medium uppercase tracking-[0.18em] text-faint">
        Run on {selected.size} of {choices.length}
      </h3>
      {#if $hosts.length === 0}
        <p class="text-sm text-muted">No hosts configured.</p>
      {:else}
        <ul class="space-y-1">
          {#each choices as host (host.name)}
            {@const checked = selected.has(host.name)}
            <li>
              <button
                type="button"
                role="checkbox"
                aria-checked={checked}
                aria-label={host.name}
                class="flex w-full items-center gap-3 rounded-lg px-3 py-2 text-left text-sm transition
                  focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus
                  {checked ? 'bg-accent text-accent-fg' : 'text-muted hover:bg-surface-inset hover:text-fg'}"
                onclick={() => toggleHost(host.name)}
              >
                <span class="grid h-4 w-4 shrink-0 place-items-center rounded border border-current">
                  {#if checked}<Icon name="check" size={11} />{/if}
                </span>
                <StatusDot status={hostStatusDot($statuses.get(host.name))} />
                <span class="min-w-0 flex-1 truncate font-medium">{host.name}</span>
                <span class="shrink-0 truncate font-mono text-xs {checked ? '' : 'text-faint'}">
                  {host.user}@{displayHostname(host.hostname, $streamerMode)}
                </span>
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  </div>

  <footer class="flex justify-end gap-2 border-t border-default px-5 py-3">
    <Button variant="ghost" onclick={onCancel}>Cancel</Button>
    <Button variant="primary" onclick={run} disabled={selected.size === 0}>
      Run on {selected.size} {selected.size === 1 ? 'host' : 'hosts'}
    </Button>
  </footer>
</Modal>
