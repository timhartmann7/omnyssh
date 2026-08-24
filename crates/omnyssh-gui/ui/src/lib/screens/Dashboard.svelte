<script lang="ts">
  // Server-card grid (tech-gui.md §2, 2.1): one card per host with live health and
  // detected services. Colour is reserved for semantic state — the header dot and
  // the metric fills read from `statusToken`; everything else is ink-on-paper. The
  // per-card `sh`/`files` buttons are the host-first spawn path (§2). Host management
  // (add/edit/delete, §4.1) lives here — there is no separate Hosts screen (§2).
  // Editing an SSH-config host adopts it into hosts.toml; the file itself is never
  // written, so only Delete stays manual-only.
  import { get } from 'svelte/store';
  import type { HostDto, HostInputDto } from '$lib/bindings';
  import { Surface, Chip, StatusDot, Icon, Button, statusToken } from '$lib/theme';
  import { serverCards, filterHosts, QUICK_ACTIONS } from './serverCard';
  import { spawnSession } from '$lib/stores/navigation';
  import { streamerMode, displayHostname, displayUser, displayHostTitle } from '$lib/stores/streamer';
  import { hosts } from '$lib/stores/hosts';
  import { lastError } from '$lib/stores/notifications';
  import { saveHost, deleteHost, reloadHosts, startKeySetup, refreshMetrics } from '$lib/ipc/commands';
  import { isRefreshHotkey } from '$lib/stores/ui';
  import { beginKeySetup, dismissKeySetup } from '$lib/stores/keySetup';
  import { emptyForm, formFromHost } from './hostForm';
  import HostEditor from './HostEditor.svelte';
  import Modal from '$lib/components/Modal.svelte';
  import { t } from '$lib/i18n';

  type Dialog = { kind: 'add' } | { kind: 'edit'; host: HostDto } | { kind: 'delete'; host: HostDto };

  let dialog = $state<Dialog | null>(null);

  // Host search (task 6): a round toggle slides a filter field out to its left and the
  // grid filters live. Frontend-only, like the snippet search — the core stays untouched.
  let query = $state('');
  let searchOpen = $state(false);
  let searchInput = $state<HTMLInputElement>();
  const visibleCards = $derived(filterHosts($serverCards, query));

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

  // Dashboard hotkeys (tech-gui.md §2): `r` refreshes metrics. This listener only exists
  // while the dashboard is mounted (the selector unmounts when a session is active), so
  // it never reaches terminal input. Suppressed while a host dialog owns the keyboard.
  function onKeydown(e: KeyboardEvent): void {
    if (dialog) return;
    if (isRefreshHotkey(e)) {
      e.preventDefault();
      void refresh();
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
  const search =
    'min-w-0 rounded-full bg-surface-inset py-1.5 text-sm text-fg outline-none transition-all duration-200 ' +
    'placeholder:text-faint focus-visible:ring-2 focus-visible:ring-focus';
</script>

<svelte:window onkeydown={onKeydown} />

<section class="min-h-full px-6 pb-8 pt-3">
  <div class="mb-5 flex items-center gap-3">
    <h1 class="text-lg font-semibold tracking-tight">{$t('dashboard.title')}</h1>
    <div class="ml-auto flex items-center gap-2">
      <!-- Host search: a round toggle that slides a live filter field out to its left. -->
      <div class="flex items-center">
        <input
          bind:this={searchInput}
          bind:value={query}
          type="text"
          placeholder={$t('dashboard.search_placeholder')}
          aria-label={$t('dashboard.search_hosts')}
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
          title={searchOpen ? $t('dashboard.close_search') : $t('dashboard.search_hosts')}
          aria-label={searchOpen ? $t('dashboard.close_search') : $t('dashboard.search_hosts')}
          aria-expanded={searchOpen}
          onclick={toggleSearch}
        >
          <Icon name={searchOpen ? 'close' : 'search'} size={15} />
        </button>
      </div>
      <!-- Force an immediate metric refresh of every host, like the TUI's `r` (also the
           `r` hotkey). Spins while in flight for feedback. -->
      <button
        type="button"
        class="{roundBtn} disabled:opacity-60"
        title={$t('dashboard.refresh_metrics')}
        aria-label={$t('dashboard.refresh_metrics')}
        disabled={refreshing}
        onclick={() => refresh()}
      >
        <span class="inline-flex {refreshing ? 'animate-spin' : ''}">
          <Icon name="refresh" size={15} />
        </span>
      </button>
      <button type="button" class={pill} onclick={() => (dialog = { kind: 'add' })}>
        <Icon name="plus" size={13} />
        {$t('dashboard.add_host')}
      </button>
    </div>
  </div>

  {#if $serverCards.length === 0}
    <div class="flex flex-col items-center justify-center gap-2 py-20 text-center">
      <p class="font-medium">{$t('dashboard.no_servers')}</p>
      <p class="text-sm text-muted">{$t('dashboard.no_servers_desc')}</p>
      <button type="button" class="{pill} mt-2" onclick={() => (dialog = { kind: 'add' })}>
        <Icon name="plus" size={13} />
        {$t('dashboard.add_host')}
      </button>
    </div>
  {:else if visibleCards.length === 0}
    <div class="flex flex-col items-center justify-center gap-2 py-20 text-center">
      <p class="text-sm text-muted">{$t('dashboard.no_match', { query })}</p>
    </div>
  {:else}
    <div class="grid gap-4 [grid-template-columns:repeat(auto-fill,minmax(19rem,1fr))]">
      {#each visibleCards as card, i (i)}
        <Surface class="flex flex-col gap-4 p-5">
          <!-- Identity, then the actions on their own row so the name and address
               stay readable at any card width (a full row instead of sharing it). -->
          <div class="flex flex-col gap-3">
            <div class="flex min-w-0 items-start gap-2.5">
              <span class="mt-1 shrink-0">
                <StatusDot status={card.overall} size={9} label="{displayHostTitle(card.host.name, $streamerMode, card.host.hostname)} status" />
              </span>
              <div class="min-w-0">
                <div class="flex min-w-0 items-center gap-2">
                  <span class="truncate font-medium" title={displayHostTitle(card.host.name, $streamerMode, card.host.hostname)}>{displayHostTitle(card.host.name, $streamerMode, card.host.hostname)}</span>
                  {#if card.host.source === 'sshConfig'}
                    <span
                      class="shrink-0 rounded-full border border-default px-1.5 py-0.5 text-[10px] text-faint"
                      title={$t('dashboard.ssh_config_title')}
                    >
                      {$t('dashboard.ssh_config_badge')}
                    </span>
                  {/if}
                  <!-- Auth-state reflection (tech-gui.md §4.2): key-only once password
                       auth is disabled, otherwise a plain key badge when a key exists. -->
                  {#if card.host.passwordAuthDisabled}
                    <span
                      class="inline-flex shrink-0 items-center gap-1 rounded-full border border-default px-1.5 py-0.5 text-[10px] text-faint"
                      title={$t('dashboard.key_only_title')}
                    >
                      <Icon name="shield" size={10} />
                      {$t('dashboard.key_only_badge')}
                    </span>
                  {:else if card.host.hasKey}
                    <span
                      class="inline-flex shrink-0 items-center gap-1 rounded-full border border-default px-1.5 py-0.5 text-[10px] text-faint"
                      title={$t('dashboard.key_title')}
                    >
                      <Icon name="key" size={10} />
                      {$t('dashboard.key_badge')}
                    </span>
                  {/if}
                </div>
                <div class="truncate font-mono text-xs text-faint">
                  {displayUser(card.host.user, $streamerMode)}@{displayHostname(card.host.hostname, $streamerMode)}:{card.host
                    .port}
                </div>
              </div>
            </div>
            <div class="flex flex-wrap items-center gap-1.5">
              {#each QUICK_ACTIONS as action (action.id)}
                {@const label = action.kind === 'terminal' ? $t('dashboard.quick_sh') : $t('dashboard.quick_files')}
                <button
                  type="button"
                  class={pill}
                  title="{label} ({displayHostTitle(card.host.name, $streamerMode, card.host.hostname)})"
                  onclick={() => spawnSession(action.kind, card.host.name)}
                >
                  <Icon name={action.kind} size={13} />
                  {label}
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
                  title={$t('dashboard.setup_key_title', { name: displayHostTitle(card.host.name, $streamerMode, card.host.hostname) })}
                  aria-label={$t('dashboard.setup_key_title', { name: displayHostTitle(card.host.name, $streamerMode, card.host.hostname) })}
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
                title={$t('dashboard.edit_host_title', { name: displayHostTitle(card.host.name, $streamerMode, card.host.hostname) })}
                aria-label={$t('dashboard.edit_host_title', { name: displayHostTitle(card.host.name, $streamerMode, card.host.hostname) })}
                onclick={() => (dialog = { kind: 'edit', host: card.host })}
              >
                <Icon name="edit" size={14} />
              </button>
              {#if card.host.source === 'manual'}
                <button
                  type="button"
                  class={iconBtn}
                  title={$t('dashboard.delete_host_title', { name: displayHostTitle(card.host.name, $streamerMode, card.host.hostname) })}
                  aria-label={$t('dashboard.delete_host_title', { name: displayHostTitle(card.host.name, $streamerMode, card.host.hostname) })}
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
              {card.reachability === 'reachable'
                ? $t('dashboard.reachable')
                : card.reachability === 'unreachable'
                  ? $t('dashboard.unreachable')
                  : $t('dashboard.checking')}{card.host.monitorPort
                ? ` · ${$t('dashboard.port_label', { port: card.host.monitorPort })}`
                : ''}
            </div>
          {:else if card.offline}
            <div class="rounded-lg bg-surface-inset px-3 py-3 text-center text-xs text-faint">{$t('dashboard.offline')}</div>
          {:else}
            <div class="space-y-2">
              {#each card.metricRows as row (row.label)}
                {@const label =
                  row.label === 'CPU'
                    ? $t('dashboard.metric_cpu')
                    : row.label === 'RAM'
                      ? $t('dashboard.metric_ram')
                      : row.label === 'Disk'
                        ? $t('dashboard.metric_disk')
                        : row.label}
                <div class="flex items-center gap-3">
                  <span class="w-9 shrink-0 text-[11px] uppercase tracking-wider text-faint">{label}</span>
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
                {#if card.uptime}<span>{$t('dashboard.up', { uptime: card.uptime })}</span>{/if}
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

          <!-- Detected services -->
          {#if card.detectedServices.length}
            <div class="flex flex-wrap gap-1.5">
              {#each card.detectedServices as service (service.kind)}
                {@const detail =
                  service.detail === 'no containers'
                    ? $t('dashboard.docker_no_containers')
                    : service.detail.includes('/') && service.detail.endsWith(' running')
                      ? $t('dashboard.docker_running', {
                          running: service.detail.split('/')[0],
                          total: service.detail.split('/')[1].replace(' running', '')
                        })
                      : service.detail}
                <Chip>{detail ? `${service.name} · ${detail}` : service.name}</Chip>
              {/each}
            </div>
          {:else if card.servicesError}
            <div class="text-xs text-faint">{$t('dashboard.service_scan_unavailable')}</div>
          {/if}
        </Surface>
      {/each}
    </div>
  {/if}
</section>

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
  <Modal label={$t('dashboard.delete_host')} onClose={() => (dialog = null)}>
    <div class="space-y-3 px-5 py-4">
      <h2 class="text-sm font-semibold">{$t('dashboard.delete_host')}</h2>
      <p class="text-sm text-muted">
        {$t('dashboard.delete_confirm', { name: host.name })}
      </p>
      <div class="flex justify-end gap-2 pt-1">
        <Button variant="ghost" onclick={() => (dialog = null)}>{$t('dashboard.cancel')}</Button>
        <Button variant="primary" onclick={() => confirmDelete(host.name)}>{$t('dashboard.delete')}</Button>
      </div>
    </div>
  </Modal>
{/if}
