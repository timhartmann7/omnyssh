<script lang="ts">
  // The ⌘K overlay and the host-picker, one component in two modes (tech-gui.md §2,
  // 1.3). Navigator: jump to an open session or open a host (default: a shell). Picker:
  // hand a chosen host back to its caller (the spawner buttons). Keyboard-first — type
  // to filter, ↑/↓ to move, ↵ to select, esc to dismiss. Glass/blur per the brandbook.
  import { tick } from 'svelte';
  import { Icon, StatusDot } from '$lib/theme';
  import { palette, paletteItems, paletteSignature, nextIndex, hostStatusDot } from '$lib/stores/palette';
  import { hosts } from '$lib/stores/hosts';
  import { statuses } from '$lib/stores/statuses';
  import { sessions, sessionLabel, sessionStatusDot } from '$lib/stores/sessions';
  import { activeEntity } from '$lib/stores/activeEntity';
  import { spawnSession } from '$lib/stores/navigation';
  import { streamerMode, displayHostname, displayUser, displayHostTitle } from '$lib/stores/streamer';
  import { isPaletteChord } from '$lib/stores/ui';
  import { t } from '$lib/i18n';

  let inputEl = $state<HTMLInputElement>();
  let listEl = $state<HTMLUListElement>();
  let query = $state('');
  let selected = $state(0);

  const items = $derived(paletteItems($palette.mode, $hosts, $sessions, query));
  // A value-stable key over the result set: unchanged by a background status flip (same
  // ids, new objects), so the reset effect below can ignore those (see the effect).
  const itemsSignature = $derived(paletteSignature(items));
  const firstSession = $derived(items.findIndex((it) => it.kind === 'session'));
  const firstHost = $derived(items.findIndex((it) => it.kind === 'host'));

  const placeholder = $derived(
    $palette.mode === 'pickHost' ? $t('palette.pick_host_placeholder') : $t('palette.search_placeholder')
  );
  const emptyMessage = $derived(
    $palette.mode === 'pickHost'
      ? query
        ? $t('palette.no_matching_hosts')
        : $t('palette.no_hosts_configured')
      : query
        ? $t('palette.no_matches')
        : $t('palette.no_hosts_or_sessions')
  );

  // Focus returns here when the overlay closes, so a keyboard user is not dropped to
  // <body> after picking a host.
  let restoreFocus: HTMLElement | null = null;

  // On open: remember the trigger, clear the query, focus the input. On close: hand
  // focus back. Re-runs on a mode switch too, since the store emits a fresh object.
  $effect(() => {
    if ($palette.open) {
      // Capture once per open session, so a mode switch does not overwrite the trigger
      // with the palette's own input.
      restoreFocus ??= document.activeElement as HTMLElement | null;
      query = '';
      selected = 0;
      void tick().then(() => inputEl?.focus());
    } else if (restoreFocus) {
      if (restoreFocus.isConnected) restoreFocus.focus();
      restoreFocus = null;
    }
  });

  // Reset the highlight to the top match whenever the result *set* changes — typing, a
  // mode switch, an added/removed/reordered row — so arrow-nav and Enter never target a
  // stale row. Keyed on the signature, not `items`: a background status flip re-derives
  // `items` (a new array) but not the signature, so it no longer snaps the selection.
  $effect(() => {
    void itemsSignature;
    selected = 0;
  });

  // Keep the highlighted row visible as arrow-nav moves it.
  $effect(() => {
    listEl?.querySelector<HTMLElement>(`[data-index="${selected}"]`)?.scrollIntoView({
      block: 'nearest'
    });
  });

  function selectAt(i: number): void {
    const item = items[i];
    if (!item) return;
    if (item.kind === 'session') {
      activeEntity.activateSession(item.session.id);
      palette.close();
    } else if ($palette.mode === 'pickHost') {
      palette.choose(item.host);
    } else {
      // Navigator default action for a host: open a shell (the primary connect path).
      spawnSession('terminal', item.host.name);
      palette.close();
    }
  }

  // Chromium re-fires mousemove when the list scrolls under a stationary pointer, which
  // would snap the highlight back during arrow-nav. Only a real move (changed
  // coordinates) is allowed to drive the selection.
  let pointer = { x: -1, y: -1 };
  function hover(e: MouseEvent, i: number): void {
    if (e.clientX === pointer.x && e.clientY === pointer.y) return;
    pointer = { x: e.clientX, y: e.clientY };
    selected = i;
  }

  function onKeydown(e: KeyboardEvent): void {
    if (isPaletteChord(e)) {
      // ⌘K toggles: open the navigator when closed, dismiss when already open.
      e.preventDefault();
      if ($palette.open) palette.close();
      else palette.open();
      return;
    }
    if (!$palette.open) return;
    // Enter/Escape that commit or cancel an IME composition must reach the input, and
    // Home/End/←/→ belong to the search field's caret — only ↑/↓, Enter, Escape drive
    // the list.
    if (e.isComposing) return;
    switch (e.key) {
      case 'Escape':
        e.preventDefault();
        palette.close();
        break;
      case 'ArrowDown':
        e.preventDefault();
        selected = nextIndex(selected, 1, items.length);
        break;
      case 'ArrowUp':
        e.preventDefault();
        selected = nextIndex(selected, -1, items.length);
        break;
      case 'Enter':
        e.preventDefault();
        selectAt(selected);
        break;
    }
  }

  const rowBase = 'flex w-full items-center gap-3 rounded-lg px-3 py-2 text-left text-sm transition';
  const rowState = (active: boolean): string =>
    active ? 'bg-accent text-accent-fg' : 'text-muted hover:bg-surface-inset hover:text-fg';
  const sectionHead = 'px-3 pb-1 pt-2 text-[11px] font-medium uppercase tracking-[0.18em] text-faint';
</script>

<svelte:window onkeydown={onKeydown} />

{#if $palette.open}
  <div
    class="fixed inset-0 z-50 flex items-start justify-center px-4 pt-[14vh]"
    role="dialog"
    aria-modal="true"
    aria-label={$palette.mode === 'pickHost' ? $t('palette.pick_host_placeholder') : $t('sidebar.palette')}
  >
    <button
      type="button"
      tabindex="-1"
      aria-label={$t('common.close')}
      class="absolute inset-0 bg-overlay"
      onclick={() => palette.close()}
    ></button>

    <div
      class="relative w-full max-w-xl overflow-hidden rounded-2xl border border-default bg-glass shadow-soft backdrop-blur-md"
    >
      <div class="flex items-center gap-3 border-b border-default px-4">
        <span class="text-faint"><Icon name="search" /></span>
        <input
          bind:this={inputEl}
          bind:value={query}
          type="text"
          {placeholder}
          aria-label={placeholder}
          autocomplete="off"
          spellcheck="false"
          class="w-full bg-transparent py-3.5 text-sm text-fg outline-none placeholder:text-faint"
        />
      </div>

      <ul bind:this={listEl} class="max-h-[min(24rem,50vh)] overflow-y-auto p-2">
        {#if items.length === 0}
          <li class="px-3 py-6 text-center text-sm text-muted">{emptyMessage}</li>
        {:else}
          <!-- Keyed by position: rows are stateless, and host names are not guaranteed
               unique, so a name key could throw each_key_duplicate. -->
          {#each items as item, i (i)}
            {#if $palette.mode === 'navigate' && i === firstSession}
              <li class={sectionHead}>{$t('palette.sessions')}</li>
            {/if}
            {#if $palette.mode === 'navigate' && i === firstHost}
              <li class={sectionHead}>{$t('palette.hosts')}</li>
            {/if}
            <li>
              <button
                type="button"
                data-index={i}
                aria-current={selected === i ? 'true' : undefined}
                class="{rowBase} {rowState(selected === i)}"
                onclick={() => selectAt(i)}
                onmousemove={(e) => hover(e, i)}
              >
                {#if item.kind === 'session'}
                  <StatusDot status={sessionStatusDot[item.session.status]} />
                  <Icon name={item.session.kind} size={16} />
                  <span class="min-w-0 flex-1 truncate">{displayHostTitle(sessionLabel(item.session), $streamerMode)}</span>
                {:else}
                  <StatusDot status={hostStatusDot($statuses.get(item.host.name))} />
                  <span class="min-w-0 flex-1 truncate font-medium">{displayHostTitle(item.host.name, $streamerMode, item.host.hostname)}</span>
                  <span class="shrink-0 truncate font-mono text-xs {selected === i ? '' : 'text-faint'}">
                    {displayUser(item.host.user, $streamerMode)}@{displayHostname(item.host.hostname, $streamerMode)}
                  </span>
                {/if}
              </button>
            </li>
          {/each}
        {/if}
      </ul>

      <div
        class="flex items-center gap-4 border-t border-default px-4 py-2 font-mono text-[11px] text-faint"
      >
        <span>{$t('palette.shortcuts')}</span>
      </div>
    </div>
  </div>
{/if}
