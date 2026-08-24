<script lang="ts">
  // Left region (tech-gui.md §2): header (logo + collapse), the four entry points
  // (two selectors that hold a highlight, two spawners that open sessions), the
  // sessions list, and the footer (palette + theme toggle, §5.1). The active
  // highlight is the brand's accent inversion, so exactly one filled row — a
  // selector or a session — is visible at any moment (the §2 invariant, made legible).
  import Logo from './Logo.svelte';
  import ThemeToggle from './ThemeToggle.svelte';
  import { Button, Icon, StatusDot, type IconName } from '$lib/theme';
  import { activeEntity } from '$lib/stores/activeEntity';
  import {
    sessions,
    sessionLabel,
    sessionTitle,
    sessionStatusDot,
    type SessionKind
  } from '$lib/stores/sessions';
  import { sidebarCollapsed } from '$lib/stores/ui';
  import { spawnSession, closeSession } from '$lib/stores/navigation';
  import { palette } from '$lib/stores/palette';
  import { support } from '$lib/stores/support';
  import { streamerMode, displayHostTitle } from '$lib/stores/streamer';
  import { t, locale, type TranslationKey } from '$lib/i18n';

  // Action-first spawn (tech-gui.md §2): a spawner opens the host-picker, then creates
  // a session of its kind for the chosen host. A dismissed picker spawns nothing.
  async function pickAndSpawn(kind: SessionKind): Promise<void> {
    const host = await palette.pickHost();
    if (host) spawnSession(kind, host.name);
  }

  type Selector = { kind: 'dashboard' | 'snippets'; key: TranslationKey; icon: IconName };
  type Spawner = { kind: SessionKind; key: TranslationKey; icon: IconName };

  const selectors: Selector[] = [
    { kind: 'dashboard', key: 'sidebar.dashboard', icon: 'dashboard' },
    { kind: 'snippets', key: 'sidebar.snippets', icon: 'snippets' }
  ];
  const spawners: Spawner[] = [
    { kind: 'sftp', key: 'sidebar.sftp', icon: 'sftp' },
    { kind: 'terminal', key: 'sidebar.terminal', icon: 'terminal' }
  ];

  const rowBase = 'flex w-full items-center gap-3 rounded-lg px-3 py-2 text-sm transition';
  // The ring belongs on the focusable element, so it is applied to buttons only —
  // never the session-row wrapper div, where :focus-visible can never match.
  const focusRing = 'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus';
  const rowState = (active: boolean): string =>
    active ? 'bg-accent text-accent-fg' : 'text-muted hover:bg-surface-inset hover:text-fg';
</script>

<aside
  class="col-start-1 row-start-1 flex h-full flex-col overflow-hidden border-r border-default bg-surface pt-[var(--titlebar-h)]"
>
  <header
    class="flex items-center gap-2.5 px-3 py-4 {$sidebarCollapsed ? 'justify-center' : ''}"
  >
    {#if !$sidebarCollapsed}
      <Logo size={22} />
      <span class="flex-1 truncate text-sm font-bold tracking-wide">OmnySSH</span>
    {/if}
    <Button
      variant="icon"
      title={$sidebarCollapsed ? $t('sidebar.expand') : $t('sidebar.collapse')}
      onclick={() => sidebarCollapsed.toggle()}
    >
      <Icon name={$sidebarCollapsed ? 'expand' : 'collapse'} />
    </Button>
  </header>

  <!-- Entry points stay pinned; only the sessions list scrolls (tech-gui.md §2). -->
  <nav class="flex min-h-0 flex-1 flex-col px-2 py-2">
    <ul class="shrink-0 space-y-1">
      {#each selectors as sel (sel.kind)}
        <li>
          <button
            type="button"
            class="{rowBase} {focusRing} {rowState($activeEntity.kind === sel.kind)} {$sidebarCollapsed
              ? 'justify-center'
              : ''}"
            title={$t(sel.key)}
            aria-current={$activeEntity.kind === sel.kind ? 'page' : undefined}
            onclick={() =>
              sel.kind === 'dashboard' ? activeEntity.selectDashboard() : activeEntity.selectSnippets()}
          >
            <Icon name={sel.icon} />
            {#if !$sidebarCollapsed}<span class="truncate">{$t(sel.key)}</span>{/if}
          </button>
        </li>
      {/each}
      {#each spawners as sp (sp.kind)}
        <li>
          <button
            type="button"
            class="{rowBase} {focusRing} {rowState(false)} {$sidebarCollapsed ? 'justify-center' : ''}"
            title={$t(sp.key)}
            onclick={() => pickAndSpawn(sp.kind)}
          >
            <Icon name={sp.icon} />
            {#if !$sidebarCollapsed}<span class="truncate">{$t(sp.key)}</span>{/if}
          </button>
        </li>
      {/each}
    </ul>

    {#if $sessions.length > 0}
      <ul class="mt-2 min-h-0 flex-1 space-y-1 overflow-y-auto border-t border-default pt-2">
        {#each $sessions as s (s.id)}
          {@const active = $activeEntity.kind === 'session' && $activeEntity.id === s.id}
          <li>
            <div
              class="{rowBase} {rowState(active)} {$sidebarCollapsed ? 'justify-center' : 'pr-1'}"
            >
              <button
                type="button"
                class="flex min-w-0 items-center gap-2.5 rounded text-left {focusRing} {$sidebarCollapsed
                  ? ''
                  : 'flex-1'}"
                title={$streamerMode ? `${displayHostTitle(s.hostName, true)} · ${s.kind}` : sessionTitle(s)}
                aria-label={$streamerMode ? `${displayHostTitle(s.hostName, true)} · ${s.kind}` : sessionTitle(s)}
                aria-current={active ? 'true' : undefined}
                onclick={() => activeEntity.activateSession(s.id)}
              >
                {#if $sidebarCollapsed}
                  <span class="relative inline-flex shrink-0">
                    <Icon name={s.kind} />
                    <span class="absolute -right-1 -top-1">
                      <StatusDot status={sessionStatusDot[s.status]} size={7} />
                    </span>
                  </span>
                {:else}
                  <StatusDot status={sessionStatusDot[s.status]} />
                  <Icon name={s.kind} size={16} />
                  <span class="min-w-0 flex-1 truncate">{displayHostTitle(sessionLabel(s), $streamerMode)}</span>
                {/if}
              </button>
              {#if !$sidebarCollapsed}
                <button
                  type="button"
                  class="shrink-0 rounded p-1 opacity-60 transition hover:opacity-100 {focusRing}"
                  title="{$t('sidebar.close_session')} ({displayHostTitle(sessionLabel(s), $streamerMode)})"
                  aria-label="{$t('sidebar.close_session')} ({displayHostTitle(sessionLabel(s), $streamerMode)})"
                  onclick={() => closeSession(s.id)}
                >
                  <Icon name="close" size={14} />
                </button>
              {/if}
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  </nav>

  <footer
    class="border-t border-default px-2 py-3 {$sidebarCollapsed
      ? 'flex flex-col items-center gap-1'
      : 'flex items-center gap-1'}"
  >
    <Button variant="icon" title={$t('sidebar.palette')} onclick={() => palette.open()}>
      <Icon name="command" />
    </Button>
    <ThemeToggle />
    <!-- Support/about overlay: free + open-source note and the two ways to help.
         Opens a modal, not a screen, so it holds no highlight and never becomes the
         active entity (§2). Sits left of the gear, icon-only so it survives collapse. -->
    <Button variant="icon" title={$t('support.title')} onclick={() => support.open()}>
      <Icon name={$locale === 'zh-CN' ? 'star' : 'telegram'} />
    </Button>
    <!-- Settings is a selector-like screen; the gear holds the active highlight like
         Dashboard/Snippets do, and stays icon-only so it survives collapse (§5.1). -->
    <button
      type="button"
      class="grid h-9 w-9 place-items-center rounded-full transition {focusRing} {$activeEntity.kind ===
      'settings'
        ? 'bg-accent text-accent-fg'
        : 'text-muted hover:bg-surface-inset hover:text-fg'}"
      title={$t('sidebar.settings')}
      aria-label={$t('sidebar.settings')}
      aria-current={$activeEntity.kind === 'settings' ? 'page' : undefined}
      onclick={() => activeEntity.selectSettings()}
    >
      <Icon name="settings" />
    </button>
  </footer>
</aside>
