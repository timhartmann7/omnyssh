<script lang="ts">
  // Server-card grid (tech-gui.md §2, 2.1): one card per host with live health and
  // detected services. Colour is reserved for semantic state — the header dot and
  // the metric fills read from `statusToken`; everything else is ink-on-paper. The
  // per-card `sh`/`files` buttons are the host-first spawn path (§2). Host management
  // (add/edit/delete, §4.1) lives here — there is no separate Hosts screen (§2).
  // Editing an SSH-config host adopts it into hosts.toml; the file itself is never
  // written, so only Delete stays manual-only.
  import { onDestroy, tick } from 'svelte';
  import { get } from 'svelte/store';
  import type { HostDto, HostInputDto } from '$lib/bindings';
  import { Surface, Chip, StatusDot, Icon, Button, statusToken } from '$lib/theme';
  import {
    serverCards,
    filterHosts,
    groupByTag,
    sortCards,
    moveCard,
    orderKeys,
    forwardListen,
    forwardTarget,
    QUICK_ACTIONS,
    type ServerCard
  } from './serverCard';
  import { dashboardView, type CardSort } from '$lib/stores/dashboardView';
  import { spawnSession } from '$lib/stores/navigation';
  import { streamerMode, displayHostname } from '$lib/stores/streamer';
  import { hosts } from '$lib/stores/hosts';
  import { lastError } from '$lib/stores/notifications';
  import {
    saveHost,
    deleteHost,
    reloadHosts,
    startKeySetup,
    refreshMetrics,
    tunnelStart,
    tunnelStop
  } from '$lib/ipc/commands';
  import { isGroupHotkey, isRefreshHotkey } from '$lib/stores/ui';
  import { beginKeySetup, dismissKeySetup } from '$lib/stores/keySetup';
  import { emptyForm, formFromHost } from './hostForm';
  import { clampToScroller, dropTarget, scrollParent, scrollStep, swallowClick } from './cardDrag';
  import HostEditor from './HostEditor.svelte';
  import Modal from '$lib/components/Modal.svelte';

  type Dialog = { kind: 'add' } | { kind: 'edit'; host: HostDto } | { kind: 'delete'; host: HostDto };

  let dialog = $state<Dialog | null>(null);

  // Host search (task 6): a round toggle slides a filter field out to its left and the
  // grid filters live. Frontend-only, like the snippet search — the core stays untouched.
  let query = $state('');
  let searchOpen = $state(false);
  let searchInput = $state<HTMLInputElement>();
  // A search unfolds every section until it clears, so no match hides behind a fold.
  const searching = $derived(query.trim() !== '');

  // Sort, group-by-tag (mirrors the TUI's `g`) and the folded sections, persisted in
  // `dashboardView` so they survive leaving the dashboard and restarts. Grouping keeps
  // the sort inside each section. While a drag runs, the grid holds the order it started
  // in (`frozen`), so a live status sort can't reshuffle cards under the pointer and a
  // cancelled drag saves nothing. It holds hosts, not names, which can repeat; raw, since
  // a deep proxy would break the identity match.
  let frozen = $state.raw<HostDto[] | null>(null);
  const keyOf = $derived(orderKeys($hosts));
  const visibleCards = $derived(onScreen(filterHosts($serverCards, query)));
  const groups = $derived($dashboardView.groupByTag ? groupByTag(visibleCards) : []);

  function onScreen(cards: ServerCard[]): ServerCard[] {
    if (!frozen) return sortCards(cards, $dashboardView.sort, $dashboardView.order, keyOf);
    const rank = new Map(frozen.map((host, i) => [host, i]));
    const at = (c: ServerCard): number => rank.get(c.host) ?? rank.size;
    return [...cards].sort((a, b) => at(a) - at(b));
  }

  // View options popover. Focus moves in on open and back to the trigger on Escape or a
  // press outside; the dot keeps a changed view from going unnoticed.
  let viewOpen = $state(false);
  let viewButton = $state<HTMLButtonElement>();
  let viewPanel = $state<HTMLElement>();
  const viewChanged = $derived($dashboardView.groupByTag || $dashboardView.sort !== 'custom');

  const SORTS: { id: CardSort; label: string }[] = [
    { id: 'custom', label: 'Custom' },
    { id: 'name', label: 'Name' },
    { id: 'status', label: 'Status' }
  ];

  // Radio group keys: arrows pick the previous/next option and move focus with it.
  async function onSortKey(e: KeyboardEvent): Promise<void> {
    const step = ({ ArrowLeft: -1, ArrowUp: -1, ArrowRight: 1, ArrowDown: 1 } as Record<string, number>)[e.key];
    if (!step) return;
    e.preventDefault();
    const group = e.currentTarget as HTMLElement;
    const at = SORTS.findIndex((o) => o.id === $dashboardView.sort);
    dashboardView.setSort(SORTS[(at + step + SORTS.length) % SORTS.length].id);
    await tick();
    group.querySelector<HTMLElement>('[aria-checked="true"]')?.focus();
  }

  async function openView(): Promise<void> {
    viewOpen = true;
    await tick();
    viewPanel?.querySelector<HTMLElement>('button')?.focus();
  }

  function closeView(refocus: boolean): void {
    viewOpen = false;
    if (refocus) viewButton?.focus();
  }

  function onPointerDown(e: PointerEvent): void {
    const target = e.target as Node;
    if (viewOpen && !viewPanel?.contains(target) && !viewButton?.contains(target)) closeView(true);
  }

  // Drag to reorder, by a card's grip. Pointer events with capture, not HTML5 drag and
  // drop, which Tauri's native file-drop handler swallows on Windows. A card moves only
  // within its run (its section, or the whole grid). A move under a name or status sort
  // turns it into the custom order, seeded from the screen, so the move is relative to
  // what the user sees.
  type Drag = {
    run: ServerCard[];
    grid: string | null;
    from: number;
    pointerId: number;
    handle: HTMLElement;
    cards: HTMLElement;
    scroller: HTMLElement;
    x0: number;
    y0: number;
    scroll0: number;
    x: number;
    y: number;
    started: boolean;
    to: number | null;
  };
  let drag: Drag | null = null;
  let frame = 0;
  // `grid` names the run: a section's tag key, or null for the ungrouped grid.
  let lifted = $state<{ grid: string | null; index: number; dx: number; dy: number } | null>(null);
  let marker = $state<{ left: number; top: number; width: number; height: number } | null>(null);
  let announcement = $state('');

  function gripDown(e: PointerEvent, run: ServerCard[], from: number, grid: string | null): void {
    const handle = e.currentTarget as HTMLElement;
    const cards = handle.closest<HTMLElement>('[data-cards]');
    if (e.button !== 0 || drag || !cards) return;
    e.preventDefault(); // no text selection
    handle.setPointerCapture(e.pointerId);
    const scroller = scrollParent(cards);
    drag = {
      run,
      grid,
      from,
      pointerId: e.pointerId,
      handle,
      cards,
      scroller,
      x0: e.clientX,
      y0: e.clientY,
      scroll0: scroller.scrollTop,
      x: e.clientX,
      y: e.clientY,
      started: false,
      to: null
    };
  }

  function gripMove(e: PointerEvent): void {
    if (e.pointerId !== drag?.pointerId) return;
    drag.x = e.clientX;
    drag.y = e.clientY;
    if (!drag.started) {
      // A small threshold keeps a plain press from lifting the card.
      if (Math.hypot(drag.x - drag.x0, drag.y - drag.y0) < 4) return;
      drag.started = true;
      frozen = onScreen($serverCards).map((c) => c.host);
      frame = requestAnimationFrame(edgeScroll);
    }
    track();
  }

  function gripUp(e: PointerEvent): void {
    if (e.pointerId !== drag?.pointerId) return;
    if (drag.started) {
      drag.x = e.clientX;
      drag.y = e.clientY;
      track();
      swallowClick();
      const { run, from, to } = drag;
      if (to !== null && to !== from) commitMove(run, from, to);
    }
    endDrag();
  }

  function endDrag(): void {
    if (!drag) return;
    cancelAnimationFrame(frame);
    if (drag.handle.hasPointerCapture(drag.pointerId)) drag.handle.releasePointerCapture(drag.pointerId);
    drag = null;
    frozen = null;
    lifted = null;
    marker = null;
  }

  // The lifted card follows the pointer, scroll included; the marker shows the landing gap.
  function track(): void {
    if (!drag) return;
    const d = drag;
    lifted = { grid: d.grid, index: d.from, dx: d.x - d.x0, dy: d.y - d.y0 + d.scroller.scrollTop - d.scroll0 };
    const drop = dropTarget(d.cards, d.from, d.x, clampToScroller(d.scroller, d.y));
    d.to = drop?.to ?? null;
    marker = drop && drop.to !== d.from ? drop.marker : null;
  }

  function edgeScroll(): void {
    if (!drag) return;
    if (!drag.handle.isConnected) return endDrag();
    const step = scrollStep(drag.scroller, drag.cards, drag.y);
    if (step) {
      drag.scroller.scrollTop += step;
      track();
    }
    frame = requestAnimationFrame(edgeScroll);
  }

  function commitMove(run: ServerCard[], from: number, to: number): void {
    dashboardView.setOrder(moveCard(onScreen($serverCards), run, from, to, keyOf));
    announcement = `${run[from].host.name} moved to position ${to + 1} of ${run.length}`;
  }

  // Keyboard reorder on the grip; focus follows the card to its new place.
  async function gripKey(e: KeyboardEvent, run: ServerCard[], from: number): Promise<void> {
    const last = run.length - 1;
    const keys: Record<string, number> = {
      ArrowUp: from - 1,
      ArrowLeft: from - 1,
      ArrowDown: from + 1,
      ArrowRight: from + 1,
      Home: 0,
      End: last
    };
    const to = keys[e.key];
    if (to === undefined || drag) return;
    e.preventDefault();
    if (to < 0 || to > last || to === from) return;
    const cards = (e.currentTarget as HTMLElement).closest('[data-cards]');
    commitMove(run, from, to);
    await tick();
    cards?.querySelectorAll<HTMLElement>('[data-grip]')[to]?.focus();
  }

  onDestroy(endDrag);

  function toggleSearch(): void {
    searchOpen = !searchOpen;
    if (searchOpen) requestAnimationFrame(() => searchInput?.focus());
    else query = '';
  }

  const message = (e: unknown): string => (e instanceof Error ? e.message : String(e));

  // Force an immediate metric poll of every host (tech-gui.md §4.2), shared by the
  // refresh button and the `r` hotkey (mirrors the TUI). The command returns before the
  // fresh metrics arrive (they land via `metrics-updated` events), so a short minimum
  // spin gives the click/keypress visible feedback.
  let refreshing = $state(false);
  async function refresh(): Promise<void> {
    if (refreshing) return;
    refreshing = true;
    try {
      await refreshMetrics();
    } catch (e) {
      lastError.set(message(e));
    }
    setTimeout(() => (refreshing = false), 500);
  }

  // Dashboard hotkeys (tech-gui.md §2): `r` refreshes metrics, `g` toggles group-by-tag.
  // This listener only exists while the dashboard is mounted (the selector unmounts when
  // a session is active), so it never reaches terminal input.
  function onKeydown(e: KeyboardEvent): void {
    if (drag) {
      if (e.key === 'Escape') {
        e.preventDefault();
        endDrag();
      }
      return;
    }
    if (viewOpen && e.key === 'Escape') {
      closeView(true);
    } else if (isRefreshHotkey(e)) {
      e.preventDefault();
      void refresh();
    } else if (isGroupHotkey(e)) {
      e.preventDefault();
      void toggleGrouping();
    }
  }

  // Regrouping rebuilds every card, so put focus back on the same control of the same
  // host. Cards sit in the DOM in screen order, folded sections included.
  async function toggleGrouping(): Promise<void> {
    const shown = (): ServerCard[] => ($dashboardView.groupByTag ? groups.flatMap((g) => g.cards) : visibleCards);
    const cardEls = (): HTMLElement[] => [...document.querySelectorAll<HTMLElement>('[data-card]')];
    const focused = document.activeElement;
    const cardEl = focused?.closest<HTMLElement>('[data-card]');
    const host = cardEl && shown()[cardEls().indexOf(cardEl)]?.host;
    const control = cardEl ? [...cardEl.querySelectorAll('button')].indexOf(focused as HTMLButtonElement) : -1;
    dashboardView.toggleGroupByTag();
    if (!host) return;
    await tick();
    const next = cardEls()[shown().findIndex((c) => c.host === host)];
    const target = next?.querySelectorAll('button')[control];
    target?.focus();
    // A folded section hides the card: land on its header instead.
    if (target && document.activeElement !== target) {
      next?.closest('section')?.querySelector<HTMLElement>('h2 button')?.focus();
    }
  }

  // Persist an add/edit, then reload so the merged cache + pollers pick it up
  // (`reload_hosts` broadcasts `hosts-loaded`). Throws propagate to the editor so a
  // failed save surfaces inline and keeps the form open.
  async function submit(input: HostInputDto, previousName: string | undefined): Promise<void> {
    // Adding: refuse a name already taken (a save would silently overwrite it). An
    // edit keeps its name (the name field is immutable, §4.1), so it can't collide.
    if (!previousName && get(hosts).some((h) => h.name === input.name)) {
      throw new Error(`A host named "${input.name}" already exists`);
    }
    await saveHost(input);
    await reloadHosts();
    dialog = null;
  }

  // Host-first auto key-setup (tech-gui.md §4.2). Open the progress panel immediately,
  // then kick the backend flow; its progress/outcome arrive as `key-setup-*` events.
  // A synchronous reject (unknown host) closes the panel and surfaces the error.
  async function setupKey(host: HostDto): Promise<void> {
    beginKeySetup(host.name);
    try {
      await startKeySetup(host.name);
    } catch (e) {
      dismissKeySetup();
      lastError.set(message(e));
    }
  }

  // The outcome arrives as `tunnel-status-changed`; only a rejected command lands here.
  async function toggleTunnel(name: string, running: boolean): Promise<void> {
    try {
      await (running ? tunnelStop(name) : tunnelStart(name));
    } catch (e) {
      lastError.set(message(e));
    }
  }

  async function confirmDelete(name: string): Promise<void> {
    try {
      await deleteHost(name);
      await reloadHosts();
    } catch (e) {
      lastError.set(message(e));
    }
    dialog = null;
  }

  // Shared pill used by the header/empty-state "Add host" and the per-card quick actions.
  const pill =
    'inline-flex items-center gap-1.5 rounded-full border border-default px-2.5 py-1 text-xs ' +
    'font-medium text-muted transition hover:border-strong hover:bg-accent hover:text-accent-fg ' +
    'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus';
  const iconBtn =
    'grid h-7 w-7 place-items-center rounded-lg text-muted transition hover:bg-surface-inset ' +
    'hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus';
  const roundBtn =
    'grid h-8 w-8 shrink-0 place-items-center rounded-full border border-default text-muted transition ' +
    'hover:border-strong hover:bg-accent hover:text-accent-fg ' +
    'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus';
  const seg =
    'rounded-lg px-3 py-1.5 text-sm transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus';
  const segState = (active: boolean): string =>
    active ? 'bg-accent text-accent-fg' : 'text-muted hover:bg-surface-inset hover:text-fg';
  const search =
    'min-w-0 rounded-full bg-surface-inset py-1.5 text-sm text-fg outline-none transition-all duration-200 ' +
    'placeholder:text-faint focus-visible:ring-2 focus-visible:ring-focus';
</script>

<svelte:window onkeydown={onKeydown} onpointerdown={onPointerDown} />

<section class="min-h-full px-6 pb-8 pt-3">
  <div class="mb-5 flex items-center gap-3">
    <h1 class="text-lg font-semibold tracking-tight">Dashboard</h1>
    <div class="ml-auto flex items-center gap-2">
      <!-- Host search: a round toggle that slides a live filter field out to its left. -->
      <div class="flex items-center">
        <input
          bind:this={searchInput}
          bind:value={query}
          type="text"
          placeholder="Search hosts…"
          aria-label="Search hosts"
          disabled={!searchOpen}
          class="{search} {searchOpen
            ? 'mr-2 w-52 px-3 opacity-100'
            : 'pointer-events-none w-0 px-0 opacity-0'}"
          onkeydown={(e) => {
            if (e.key === 'Escape') toggleSearch();
          }}
        />
        <button
          type="button"
          class={roundBtn}
          title={searchOpen ? 'Close search' : 'Search hosts'}
          aria-label={searchOpen ? 'Close search' : 'Search hosts'}
          aria-expanded={searchOpen}
          onclick={toggleSearch}
        >
          <Icon name={searchOpen ? 'close' : 'search'} size={15} />
        </button>
      </div>
      <div class="relative">
        <button
          bind:this={viewButton}
          type="button"
          class="{roundBtn} relative"
          title="View options"
          aria-label="View options"
          aria-haspopup="dialog"
          aria-expanded={viewOpen}
          onclick={() => (viewOpen ? closeView(true) : openView())}
        >
          <Icon name="layers" size={15} />
          {#if viewChanged}
            <span class="absolute right-0 top-0 h-2 w-2 rounded-full bg-accent ring-2 ring-bg"></span>
          {/if}
        </button>
        {#if viewOpen}
          <div
            bind:this={viewPanel}
            role="dialog"
            aria-label="View options"
            tabindex="-1"
            class="absolute right-0 top-10 z-20 w-72 space-y-4 rounded-2xl border border-default bg-surface-raised p-4 shadow-soft"
            onfocusout={(e) => {
              const next = e.relatedTarget as Node | null;
              if (next && !viewPanel?.contains(next) && next !== viewButton) closeView(false);
            }}
          >
            <div class="flex items-center justify-between gap-4">
              <div class="min-w-0">
                <p class="text-sm">Group by tag</p>
                <p class="text-xs text-muted">Each host goes under its first tag.</p>
              </div>
              <button
                type="button"
                role="switch"
                aria-checked={$dashboardView.groupByTag}
                aria-label="Group by tag"
                title="Group by tag (G)"
                onclick={() => dashboardView.toggleGroupByTag()}
                class="relative h-6 w-11 shrink-0 rounded-full transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus {$dashboardView.groupByTag
                  ? 'bg-accent'
                  : 'bg-surface-inset'}"
              >
                <span
                  class="absolute top-0.5 h-5 w-5 rounded-full bg-surface shadow-soft transition-[left] {$dashboardView.groupByTag
                    ? 'left-[1.375rem]'
                    : 'left-0.5'}"
                ></span>
              </button>
            </div>
            <div class="border-t border-default pt-4">
              <p id="dashboard-sort-label" class="mb-2 text-sm">Sort</p>
              <div
                role="radiogroup"
                aria-labelledby="dashboard-sort-label"
                tabindex="-1"
                class="flex gap-1 rounded-xl bg-surface-inset p-1"
                onkeydown={onSortKey}
              >
                {#each SORTS as option (option.id)}
                  {@const on = $dashboardView.sort === option.id}
                  <button
                    type="button"
                    role="radio"
                    aria-checked={on}
                    tabindex={on ? 0 : -1}
                    class="{seg} flex-1 {segState(on)}"
                    onclick={() => dashboardView.setSort(option.id)}
                  >
                    {option.label}
                  </button>
                {/each}
              </div>
              <p class="mt-2 text-xs text-muted">Drag a card's handle to set your own order.</p>
            </div>
          </div>
        {/if}
      </div>
      <!-- Force an immediate metric refresh of every host, like the TUI's `r` (also the
           `r` hotkey). Spins while in flight for feedback. -->
      <button
        type="button"
        class="{roundBtn} disabled:opacity-60"
        title="Refresh metrics (R)"
        aria-label="Refresh metrics"
        disabled={refreshing}
        onclick={() => refresh()}
      >
        <span class="inline-flex {refreshing ? 'animate-spin' : ''}">
          <Icon name="refresh" size={15} />
        </span>
      </button>
      <button type="button" class={pill} onclick={() => (dialog = { kind: 'add' })}>
        <Icon name="plus" size={13} />
        Add host
      </button>
    </div>
  </div>

  {#if $serverCards.length === 0}
    <div class="flex flex-col items-center justify-center gap-2 py-20 text-center">
      <p class="font-medium">No servers yet</p>
      <p class="text-sm text-muted">Add a host, or import one from your SSH config, to see it here.</p>
      <button type="button" class="{pill} mt-2" onclick={() => (dialog = { kind: 'add' })}>
        <Icon name="plus" size={13} />
        Add host
      </button>
    </div>
  {:else if visibleCards.length === 0}
    <div class="flex flex-col items-center justify-center gap-2 py-20 text-center">
      <p class="text-sm text-muted">No hosts match “{query}”.</p>
    </div>
  {:else if $dashboardView.groupByTag}
    <div class="flex flex-col gap-6">
      {#each groups as group, g (group.tag ?? '')}
        {@const key = group.tag ?? ''}
        {@const open = searching || !$dashboardView.collapsed.includes(key)}
        <section aria-label="{group.tag ?? 'Untagged'} hosts">
          <h2 class="mb-3">
            <button
              type="button"
              class="flex w-full items-center gap-2 rounded-lg text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus"
              aria-expanded={open}
              aria-controls="dashboard-section-{g}"
              disabled={searching}
              onclick={() => dashboardView.toggleCollapsed(key)}
            >
              <span class="inline-flex text-faint transition-transform {open ? '' : '-rotate-90'}">
                <Icon name="chevron" size={14} />
              </span>
              <span class="truncate text-sm font-semibold {group.tag === null ? 'italic text-muted' : ''}">
                {group.tag ?? 'Untagged'}
              </span>
              <span class="text-xs tabular-nums text-faint">{group.cards.length}</span>
              <span class="flex-1 border-t border-default"></span>
            </button>
          </h2>
          <div
            id="dashboard-section-{g}"
            data-cards
            class="{open ? 'grid' : 'hidden'} gap-4 [grid-template-columns:repeat(auto-fill,minmax(19rem,1fr))]"
          >
            {#each group.cards as card, i (card.host)}
              {@render serverCard(card, group.cards, i, key)}
            {/each}
          </div>
        </section>
      {/each}
    </div>
  {:else}
    <div data-cards class="grid gap-4 [grid-template-columns:repeat(auto-fill,minmax(19rem,1fr))]">
      {#each visibleCards as card, i (card.host)}
        {@render serverCard(card, visibleCards, i, null)}
      {/each}
    </div>
  {/if}

  <p id="dashboard-reorder-help" class="sr-only">
    Drag, or press the arrow keys, Home or End, to move the card.
  </p>
  <p class="sr-only" aria-live="polite">{announcement}</p>
  {#if marker}
    <div
      class="pointer-events-none fixed z-30 rounded-full bg-accent"
      style:left="{marker.left}px"
      style:top="{marker.top}px"
      style:width="{marker.width}px"
      style:height="{marker.height}px"
    ></div>
  {/if}
</section>

<!-- `run` is the card's section (or the whole grid), which a reorder stays within. -->
{#snippet serverCard(card: ServerCard, run: ServerCard[], i: number, grid: string | null)}
  {@const lift = lifted && lifted.grid === grid && lifted.index === i ? lifted : null}
  <div
    data-card
    class="group/card relative {lift ? 'z-20 opacity-90' : ''}"
    style:transform={lift ? `translate(${lift.dx}px, ${lift.dy}px)` : undefined}
  >
    <Surface class="flex h-full flex-col gap-4 p-5 {lift ? 'shadow-soft' : ''}">
      <!-- Identity, then the actions on their own row so the name and address
           stay readable at any card width (a full row instead of sharing it). -->
      <div class="flex flex-col gap-3">
        <div class="flex min-w-0 items-start gap-2.5">
          <span class="mt-1 shrink-0">
            <StatusDot status={card.overall} size={9} label="{card.host.name} status" />
          </span>
          <div class="min-w-0">
            <div class="flex min-w-0 items-center gap-2">
              <span class="truncate font-medium" title={card.host.name}>{card.host.name}</span>
              {#if card.host.source === 'sshConfig'}
                <span
                  class="shrink-0 rounded-full border border-default px-1.5 py-0.5 text-[10px] text-faint"
                  title="Imported from ~/.ssh/config — editing saves your own copy, which takes priority"
                >
                  ssh config
                </span>
              {/if}
              <!-- Auth-state reflection (tech-gui.md §4.2): key-only once password
                   auth is disabled, otherwise a plain key badge when a key exists. -->
              {#if card.host.passwordAuthDisabled}
                <span
                  class="inline-flex shrink-0 items-center gap-1 rounded-full border border-default px-1.5 py-0.5 text-[10px] text-faint"
                  title="Password authentication disabled — key only"
                >
                  <Icon name="shield" size={10} />
                  key-only
                </span>
              {:else if card.host.hasKey}
                <span
                  class="inline-flex shrink-0 items-center gap-1 rounded-full border border-default px-1.5 py-0.5 text-[10px] text-faint"
                  title="Key authentication configured"
                >
                  <Icon name="key" size={10} />
                  key
                </span>
              {/if}
            </div>
            <div class="truncate font-mono text-xs text-faint">
              {card.host.user}@{displayHostname(card.host.hostname, $streamerMode)}:{card.host
                .port}
            </div>
          </div>
          <button
            type="button"
            data-grip
            class="{iconBtn} -mr-2 -mt-1 ml-auto shrink-0 touch-none focus-visible:opacity-100 group-hover/card:opacity-100 {lift
              ? 'cursor-grabbing opacity-100'
              : 'cursor-grab opacity-0'}"
            title="Drag to reorder"
            aria-label="Move {card.host.name}"
            aria-describedby="dashboard-reorder-help"
            onpointerdown={(e) => gripDown(e, run, i, grid)}
            onpointermove={gripMove}
            onpointerup={gripUp}
            onpointercancel={endDrag}
            onlostpointercapture={endDrag}
            onkeydown={(e) => gripKey(e, run, i)}
          >
            <Icon name="grip" size={14} />
          </button>
        </div>
        <div class="flex flex-wrap items-center gap-1.5">
          {#each QUICK_ACTIONS as action (action.id)}
            <button
              type="button"
              class={pill}
              title="{action.label} on {card.host.name}"
              onclick={() => spawnSession(action.kind, card.host.name)}
            >
              <Icon name={action.kind} size={13} />
              {action.label}
            </button>
          {/each}
          <!-- Key setup stays manual-only even though edit no longer is: it records
               `key_setup_date`/`password_auth_disabled` through `save_hosts`, which
               keeps manual entries only — on an import that outcome would be dropped.
               Adopt the host first, then set up its key. -->
          {#if card.host.source === 'manual' && !card.host.hasKey}
            <button
              type="button"
              class={iconBtn}
              title="Set up an SSH key for {card.host.name}"
              aria-label="Set up an SSH key for {card.host.name}"
              onclick={() => setupKey(card.host)}
            >
              <Icon name="key" size={14} />
            </button>
          {/if}
          <!-- Editing an import adopts it into hosts.toml (§4.2); ~/.ssh/config is
               never written, so the action is offered whatever the source. Delete
               stays manual-only: there is nothing of an import to remove here. -->
          <button
            type="button"
            class={iconBtn}
            title="Edit {card.host.name}"
            aria-label="Edit {card.host.name}"
            onclick={() => (dialog = { kind: 'edit', host: card.host })}
          >
            <Icon name="edit" size={14} />
          </button>
          {#if card.host.source === 'manual'}
            <button
              type="button"
              class={iconBtn}
              title="Delete {card.host.name}"
              aria-label="Delete {card.host.name}"
              onclick={() => (dialog = { kind: 'delete', host: card.host })}
            >
              <Icon name="trash" size={14} />
            </button>
          {/if}
        </div>
      </div>

      <!-- Reachability, live metrics, or an offline state -->
      {#if card.reachability}
        <div
          class="rounded-lg bg-surface-inset px-3 py-3 text-center text-xs"
          style="color: {statusToken(card.overall)};"
        >
          {card.reachability}{card.host.monitorPort ? ` · port ${card.host.monitorPort}` : ''}
        </div>
      {:else if card.offline}
        <div class="rounded-lg bg-surface-inset px-3 py-3 text-center text-xs text-faint">offline</div>
      {:else}
        <div class="space-y-2">
          {#each card.metricRows as row (row.label)}
            <div class="flex items-center gap-3">
              <span class="w-9 shrink-0 text-[11px] uppercase tracking-wider text-faint">{row.label}</span>
              <div class="h-1.5 flex-1 overflow-hidden rounded-full bg-surface-inset">
                {#if row.percent != null}
                  <div
                    class="h-full rounded-full"
                    style="width: {Math.min(row.percent, 100)}%; background-color: {statusToken(row.status)};"
                  ></div>
                {/if}
              </div>
              <span
                class="w-10 shrink-0 text-right text-xs tabular-nums {row.percent == null
                  ? 'text-faint'
                  : 'text-muted'}"
              >
                {row.percent != null ? `${Math.round(row.percent)}%` : '—'}
              </span>
            </div>
          {/each}
        </div>

        {#if card.uptime || card.osInfo}
          <div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted">
            {#if card.uptime}<span>up {card.uptime}</span>{/if}
            {#if card.uptime && card.osInfo}<span class="text-faint">·</span>{/if}
            {#if card.osInfo}<span class="min-w-0 truncate">{card.osInfo}</span>{/if}
          </div>
        {/if}

        {#if card.topProcesses.length}
          <ul class="space-y-1">
            {#each card.topProcesses as proc, p (p)}
              <li class="flex items-center justify-between gap-3 text-xs">
                <span class="min-w-0 truncate font-mono text-muted">{proc.name}</span>
                <span class="shrink-0 tabular-nums text-faint">{Math.round(proc.cpuPercent)}%</span>
              </li>
            {/each}
          </ul>
        {/if}
      {/if}

      <!-- Why it is down. It can name hosts and addresses, so streamer mode keeps it off screen.
           Selectable: a refused host key comes with a command to copy. -->
      {#if card.failure}
        <p class="select-text break-words text-xs text-status-crit">
          {$streamerMode ? 'Details hidden in streamer mode' : card.failure}
        </p>
      {/if}

      <!-- Detected services -->
      {#if card.detectedServices.length}
        <div class="flex flex-wrap gap-1.5">
          {#each card.detectedServices as service (service.kind)}
            <Chip>{service.detail ? `${service.name} · ${service.detail}` : service.name}</Chip>
          {/each}
        </div>
      {:else if card.servicesError}
        <div class="text-xs text-faint">Service scan unavailable</div>
      {/if}

      <!-- Port forwarding: one tunnel per host carries every forward. -->
      {#if card.tunnel}
        {@const tunnel = card.tunnel}
        <div class="space-y-2 border-t border-default pt-3">
          <div class="flex items-center gap-2">
            <span class="text-faint"><Icon name="tunnel" size={13} /></span>
            <StatusDot status={tunnel.dot} size={7} label="{card.host.name} tunnel {tunnel.label}" />
            <span class="text-xs text-muted">{tunnel.label}</span>
            {#if tunnel.autostart}
              <span
                class="shrink-0 rounded-full border border-default px-1.5 py-0.5 text-[10px] text-faint"
                title="Starts when OmnySSH opens"
              >
                auto
              </span>
            {/if}
            <button
              type="button"
              class="{pill} ml-auto"
              title="{tunnel.running ? 'Stop' : 'Start'} the tunnel to {card.host.name}"
              aria-label="{tunnel.running ? 'Stop' : 'Start'} the tunnel to {card.host.name}"
              onclick={() => toggleTunnel(card.host.name, tunnel.running)}
            >
              <Icon name={tunnel.running ? 'close' : 'play'} size={12} />
              {tunnel.running ? 'Stop' : 'Start'}
            </button>
          </div>
          <ul class="space-y-1">
            {#each tunnel.forwards as forward, f (f)}
              <li class="flex min-w-0 items-center gap-1.5 font-mono text-xs text-muted">
                <span class="truncate">{forwardListen(forward, $streamerMode)}</span>
                <span class="shrink-0 text-faint">→</span>
                <span class="truncate">{forwardTarget(forward, $streamerMode)}</span>
              </li>
            {/each}
          </ul>
          <!-- The reason names hosts and addresses, so streamer mode keeps it off screen. -->
          {#if tunnel.message}
            <p class="break-words text-xs {tunnel.running ? 'text-faint' : 'text-status-crit'}">
              {$streamerMode ? 'Details hidden in streamer mode' : tunnel.message}
            </p>
          {/if}
        </div>
      {/if}
    </Surface>
  </div>
{/snippet}

{#if dialog?.kind === 'add'}
  <HostEditor mode="add" initial={emptyForm()} onSubmit={submit} onCancel={() => (dialog = null)} />
{:else if dialog?.kind === 'edit'}
  {@const host = dialog.host}
  <HostEditor
    mode="edit"
    initial={formFromHost(host)}
    previousName={host.name}
    imported={host.source === 'sshConfig'}
    onSubmit={submit}
    onCancel={() => (dialog = null)}
  />
{:else if dialog?.kind === 'delete'}
  {@const host = dialog.host}
  <Modal label="Delete host" onClose={() => (dialog = null)}>
    <div class="space-y-3 px-5 py-4">
      <h2 class="text-sm font-semibold">Delete host</h2>
      <p class="text-sm text-muted">
        Delete “{host.name}”? This removes it from <span class="font-mono">hosts.toml</span>. If
        your SSH config defines the same name, it comes back as an import.
      </p>
      <div class="flex justify-end gap-2 pt-1">
        <Button variant="ghost" onclick={() => (dialog = null)}>Cancel</Button>
        <Button variant="primary" onclick={() => confirmDelete(host.name)}>Delete</Button>
      </div>
    </div>
  </Modal>
{/if}
