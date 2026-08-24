<!-- Content follows the active entity (tech-gui.md §2): exactly one of Dashboard,
     Snippets, or one session. Terminal and SFTP tabs live in a persistent layer so
     their scrollback / byte stream (terminal) and pane state (SFTP) survive switching
     away — only visibility toggles. The list_hosts call feeds the status-bar count. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { listHosts } from '$lib/ipc/commands';
  import { hosts } from '$lib/stores/hosts';
  import { lastError } from '$lib/stores/notifications';
  import { activeEntity } from '$lib/stores/activeEntity';
  import { sessions } from '$lib/stores/sessions';
  import AppShell from '$lib/components/AppShell.svelte';
  import Dashboard from '$lib/screens/Dashboard.svelte';
  import Snippets from '$lib/screens/Snippets.svelte';
  import Settings from '$lib/screens/Settings.svelte';
  import TerminalView from '$lib/screens/TerminalView.svelte';
  import SftpView from '$lib/screens/SftpView.svelte';

  onMount(async () => {
    if (typeof window !== 'undefined' && !('__TAURI_INTERNALS__' in window)) return;
    try {
      hosts.set(await listHosts());
    } catch (err) {
      lastError.set(err instanceof Error ? err.message : String(err));
    }
  });

  const activeSessionId = $derived($activeEntity.kind === 'session' ? $activeEntity.id : null);
  // A selector (Dashboard/Snippets) owns the overlay; a session owns the persistent
  // layer. The two are mutually exclusive — the §2 exactly-one-active invariant.
  const selectorActive = $derived(
    $activeEntity.kind === 'dashboard' ||
      $activeEntity.kind === 'snippets' ||
      $activeEntity.kind === 'settings'
  );
</script>

<AppShell>
  <div class="relative h-full">
    {#each $sessions as s (s.id)}
      {#if s.kind === 'terminal'}
        <TerminalView session={s} active={activeSessionId === s.id} />
      {:else}
        <SftpView session={s} active={activeSessionId === s.id} />
      {/if}
    {/each}

    {#if selectorActive}
      <!-- Inset the scroll container (not the content) below the macOS title-bar strip,
           so selector content scrolls within its pane and never under the traffic lights. -->
      <div class="absolute inset-0 pt-[var(--titlebar-h)]">
        <div class="h-full overflow-auto overscroll-contain">
          {#if $activeEntity.kind === 'dashboard'}
            <Dashboard />
          {:else if $activeEntity.kind === 'snippets'}
            <Snippets />
          {:else if $activeEntity.kind === 'settings'}
            <Settings />
          {/if}
        </div>
      </div>
    {/if}
  </div>
</AppShell>
