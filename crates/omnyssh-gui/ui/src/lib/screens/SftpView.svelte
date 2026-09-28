<script lang="ts">
  // A live SFTP tab (tech-gui.md §3.2). One instance per SFTP session, kept mounted for
  // the session's life — hidden, not destroyed, when another entity is active — so pane
  // state survives tab switches. Opens the session on mount, drives both panes via the
  // sftp_* commands, and reads its per-session state from the sftp store (fed by the
  // `sftp-*` events, §3.4). Local browsing uses list_local_dir (returns directly);
  // remote uses sftp_list (arrives as an event). Semantic tokens only (§5.1).
  import { onMount, onDestroy } from 'svelte';
  import { homeDir } from '@tauri-apps/api/path';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { Icon } from '$lib/theme';
  import Modal from '$lib/components/Modal.svelte';
  import SftpPane from './SftpPane.svelte';
  import type { FileEntryDto } from '$lib/bindings';
  import { sessions, type Session } from '$lib/stores/sessions';
  import { sftp, markedEntries, formatBytes, type PaneSide } from '$lib/stores/sftp';
  import { lastError } from '$lib/stores/notifications';
  import {
    sftpOpen,
    sftpList,
    sftpClose,
    sftpUpload,
    sftpDownload,
    sftpMkdir,
    sftpRename,
    sftpDelete,
    sftpPreview,
    listLocalDir,
    previewLocalFile
  } from '$lib/ipc/commands';

  let { session, active }: { session: Session; active: boolean } = $props();

  let backendId = $state<number | undefined>(undefined);
  let openError = $state<string | undefined>(undefined);
  let destroyed = false;
  let mirrored: string | undefined;
  let dragged: { side: PaneSide; entry: FileEntryDto } | undefined;
  let stopExternalDrop: (() => void) | undefined;

  // Queued mutations, dispatched one at a time (see the pump effect). The core's SFTP
  // command channel is bounded and drops on overflow, so a large batch fired at once
  // would silently lose commands and wedge the op-done FIFO; gating on the previous
  // op's completion keeps at most one command outstanding.
  let outbox = $state<Array<() => void>>([]);

  // A pending mkdir/rename input. Rename carries the entry being renamed.
  let prompt = $state<{ kind: 'mkdir' | 'rename'; value: string; target?: FileEntryDto } | null>(
    null
  );

  const view = $derived(backendId != null ? $sftp.get(backendId) : undefined);
  const transfer = $derived(view?.transfer);

  const localMarkedFiles = $derived(view ? markedEntries(view.local).filter((e) => !e.isDir) : []);
  const remoteMarked = $derived(view ? markedEntries(view.remote) : []);
  const remoteMarkedFiles = $derived(remoteMarked.filter((e) => !e.isDir));
  const singleRemoteMark = $derived(remoteMarked.length === 1 ? remoteMarked[0] : undefined);

  function errMsg(err: unknown): string {
    return err instanceof Error ? err.message : String(err);
  }

  function joinRemote(dir: string, name: string): string {
    return dir.endsWith('/') ? `${dir}${name}` : `${dir}/${name}`;
  }

  function joinLocal(dir: string, name: string): string {
    const sep = dir.includes('\\') && !dir.includes('/') ? '\\' : '/';
    return dir.endsWith(sep) ? `${dir}${name}` : `${dir}${sep}${name}`;
  }

  async function refreshLocal(path: string): Promise<void> {
    const id = backendId;
    if (id == null) return;
    sftp.beginLoading(id, 'local');
    try {
      const entries = await listLocalDir(path);
      sftp.listing(id, 'local', path, entries);
    } catch (err) {
      sftp.paneError(id, 'local', errMsg(err));
    }
  }

  function refreshRemote(path: string): void {
    const id = backendId;
    if (id == null) return;
    sftp.beginLoading(id, 'remote');
    void sftpList(id, path).catch((err) => sftp.paneError(id, 'remote', errMsg(err)));
  }

  onMount(() => {
    void (async () => {
      let home = '/';
      try {
        home = await homeDir();
      } catch {
        home = '/';
      }
      let id: number;
      try {
        id = await sftpOpen(session.hostName);
      } catch (err) {
        sessions.setStatus(session.id, 'failed');
        openError = errMsg(err);
        lastError.set(errMsg(err));
        return;
      }
      if (destroyed) {
        void sftpClose(id).catch(() => {});
        return;
      }
      backendId = id;
      sftp.open(id, session.hostName);
      stopExternalDrop = await getCurrentWebview().onDragDropEvent((event) => {
        if (active && event.payload.type === 'drop' && event.payload.paths.length > 0) {
          uploadExternal(event.payload.paths);
        }
      });
      if (destroyed) {
        stopExternalDrop();
        stopExternalDrop = undefined;
      }
      void refreshLocal(home);
      refreshRemote('/');
    })();
  });

  onDestroy(() => {
    destroyed = true;
    if (backendId != null) {
      void sftpClose(backendId).catch(() => {});
      sftp.remove(backendId);
    }
    stopExternalDrop?.();
  });

  // Mirror the store connection status to the sidebar dot (the sessions store is the
  // sidebar's source of truth); only on change, to avoid churning the sessions list.
  $effect(() => {
    if (view && view.status !== mirrored) {
      mirrored = view.status;
      sessions.setStatus(session.id, view.status);
    }
  });

  // Dispatch the next queued mutation once the previous one is acked (pending empty),
  // so at most one command is outstanding and the bounded core channel never overflows.
  $effect(() => {
    if (!view || view.pending.length > 0 || outbox.length === 0) return;
    const [next, ...rest] = outbox;
    outbox = rest;
    next();
  });

  // Re-list the affected pane once every queued mutation has drained — the FS changed
  // (§3.2). Gated on an empty outbox so a batch re-lists once at the end, not per op.
  $effect(() => {
    const id = backendId;
    if (id == null || !view || view.pending.length > 0 || outbox.length > 0 || !view.refresh) return;
    const target = view.refresh;
    sftp.clearRefresh(id);
    if (target === 'local' || target === 'both') void refreshLocal(view.local.path);
    if (target === 'remote' || target === 'both') refreshRemote(view.remote.path);
  });

  function navigate(side: PaneSide, entry: FileEntryDto): void {
    if (side === 'local') void refreshLocal(entry.path);
    else refreshRemote(entry.path);
  }

  function toggleMark(side: PaneSide, path: string): void {
    if (backendId != null) sftp.toggleMark(backendId, side, path);
  }

  async function preview(side: PaneSide, entry: FileEntryDto): Promise<void> {
    const id = backendId;
    if (id == null) return;
    if (side === 'local') {
      try {
        const content = await previewLocalFile(entry.path);
        sftp.setPreview(id, { path: entry.path, content });
      } catch (err) {
        lastError.set(errMsg(err));
      }
    } else {
      void sftpPreview(id, entry.path).catch((err) => lastError.set(errMsg(err)));
    }
  }

  // If a mutating invoke itself rejects (it never does for a normal enqueue, but an IPC
  // failure could), pop its pending op so the dispatch pump does not wedge.
  function onDispatchError(id: number): (err: unknown) => void {
    return (err) => {
      lastError.set(errMsg(err));
      sftp.opDone(id, false, errMsg(err));
    };
  }

  function enqueue(...actions: Array<() => void>): void {
    if (!actions.length) return;
    // Clear the prior batch's lingering error only when starting from idle. Piling onto a
    // batch that is still draining must not wipe a failure it already recorded (that error
    // stays visible until the next fresh action — see applyOpDone).
    const draining = outbox.length > 0 || (view?.pending.length ?? 0) > 0;
    if (backendId != null && !draining) sftp.clearError(backendId);
    outbox = [...outbox, ...actions];
  }

  function upload(): void {
    const id = backendId;
    if (id == null || !view) return;
    const dir = view.remote.path;
    enqueue(
      ...localMarkedFiles.map((file) => () => {
        sftp.pushOp(id, { kind: 'upload', name: file.name, refresh: 'remote' });
        void sftpUpload(id, file.path, joinRemote(dir, file.name)).catch(onDispatchError(id));
      })
    );
  }

  function uploadExternal(paths: string[]): void {
    const id = backendId;
    if (id == null || !view) return;
    const remoteDir = view.remote.path;
    enqueue(
      ...paths
        .map((path) => ({ path, name: path.split(/[\\/]/).pop() ?? '' }))
        .filter((file) => file.name && file.name !== '.' && file.name !== '..')
        .map((file) => () => {
          sftp.pushOp(id, { kind: 'upload', name: file.name, refresh: 'remote' });
          void sftpUpload(id, file.path, joinRemote(remoteDir, file.name)).catch(onDispatchError(id));
        })
    );
  }

  function download(): void {
    const id = backendId;
    if (id == null || !view) return;
    const dir = view.local.path;
    enqueue(
      ...remoteMarkedFiles.map((file) => () => {
        sftp.pushOp(id, { kind: 'download', name: file.name, refresh: 'local' });
        void sftpDownload(id, joinLocal(dir, file.name), file.path).catch(onDispatchError(id));
      })
    );
  }

  function startDrag(side: PaneSide, entry: FileEntryDto): void {
    if (entry.isDir) return;
    dragged = { side, entry };
  }

  function dropOn(side: PaneSide): void {
    const id = backendId;
    const source = dragged;
    dragged = undefined;
    if (id == null || !view || !source || source.side === side || source.entry.isDir) return;

    if (source.side === 'local' && side === 'remote') {
      const remoteDir = view.remote.path;
      enqueue(() => {
        sftp.pushOp(id, { kind: 'upload', name: source.entry.name, refresh: 'remote' });
        void sftpUpload(id, source.entry.path, joinRemote(remoteDir, source.entry.name)).catch(
          onDispatchError(id)
        );
      });
    } else if (source.side === 'remote' && side === 'local') {
      const localDir = view.local.path;
      enqueue(() => {
        sftp.pushOp(id, { kind: 'download', name: source.entry.name, refresh: 'local' });
        void sftpDownload(id, joinLocal(localDir, source.entry.name), source.entry.path).catch(
          onDispatchError(id)
        );
      });
    }
  }

  function remove(): void {
    const id = backendId;
    if (id == null) return;
    enqueue(
      ...remoteMarked.map((entry) => () => {
        sftp.pushOp(id, { kind: 'delete', name: entry.name, refresh: 'remote' });
        void sftpDelete(id, entry.path).catch(onDispatchError(id));
      })
    );
  }

  function openPrompt(kind: 'mkdir' | 'rename'): void {
    if (kind === 'rename' && singleRemoteMark) {
      prompt = { kind, value: singleRemoteMark.name, target: singleRemoteMark };
    } else if (kind === 'mkdir') {
      prompt = { kind, value: '' };
    }
  }

  function submitPrompt(): void {
    const id = backendId;
    if (id == null || !view || !prompt) return;
    const value = prompt.value.trim();
    if (!value) return;
    const dir = view.remote.path;
    if (prompt.kind === 'mkdir') {
      enqueue(() => {
        sftp.pushOp(id, { kind: 'mkdir', refresh: 'remote' });
        void sftpMkdir(id, joinRemote(dir, value)).catch(onDispatchError(id));
      });
    } else if (prompt.target) {
      const from = prompt.target.path;
      enqueue(() => {
        sftp.pushOp(id, { kind: 'rename', refresh: 'remote' });
        void sftpRename(id, from, joinRemote(dir, value)).catch(onDispatchError(id));
      });
    }
    prompt = null;
  }

  function closePreview(): void {
    if (backendId != null) sftp.clearPreview(backendId);
  }

  function transferPercent(done: number, total: number): number {
    return total > 0 ? Math.min(100, Math.round((done / total) * 100)) : 0;
  }

  const toolBtn =
    'inline-flex items-center gap-1 rounded-full border border-default px-2 py-1 text-xs ' +
    'font-medium text-muted transition hover:border-strong hover:bg-accent hover:text-accent-fg ' +
    'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus ' +
    'disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent ' +
    'disabled:hover:text-muted disabled:hover:border-default';
  const field =
    'w-full rounded-lg bg-surface-inset px-3 py-2 text-sm text-fg outline-none ' +
    'focus-visible:ring-2 focus-visible:ring-focus placeholder:text-faint';
</script>

<!-- bg-surface fills behind the macOS traffic lights (no seam); the pt insets the
     panes below them. -->
<div class="absolute inset-0 flex flex-col bg-surface pt-[var(--titlebar-h)] {active ? '' : 'hidden'}">
  {#if openError}
    <div class="flex flex-1 flex-col items-center justify-center gap-2 p-10 text-center">
      <p class="font-medium">Could not open SFTP on {session.hostName}</p>
      <p class="max-w-md text-sm text-muted">{openError}</p>
    </div>
  {:else if !view}
    <div class="flex flex-1 items-center justify-center p-10 text-center">
      <p class="text-sm text-muted">Connecting to {session.hostName}…</p>
    </div>
  {:else}
    <div class="grid min-h-0 flex-1 grid-cols-2 divide-x divide-default">
      <SftpPane
        title="Local"
        pane={view.local}
        onNavigate={(e) => navigate('local', e)}
        onToggleMark={(p) => toggleMark('local', p)}
        onPreview={(e) => preview('local', e)}
        onDragStart={(e) => startDrag('local', e)}
        onDrop={() => dropOn('local')}
      >
        {#snippet toolbar()}
          <button
            type="button"
            class={toolBtn}
            title="Upload marked files to the remote directory"
            disabled={localMarkedFiles.length === 0}
            onclick={upload}
          >
            <Icon name="upload" size={13} />
            Upload
          </button>
          <button
            type="button"
            class={toolBtn}
            title="Refresh"
            aria-label="Refresh local"
            onclick={() => refreshLocal(view.local.path)}
          >
            <Icon name="refresh" size={13} />
          </button>
        {/snippet}
      </SftpPane>

      <SftpPane
        title={session.hostName}
        pane={view.remote}
        onNavigate={(e) => navigate('remote', e)}
        onToggleMark={(p) => toggleMark('remote', p)}
        onPreview={(e) => preview('remote', e)}
        onDragStart={(e) => startDrag('remote', e)}
        onDrop={() => dropOn('remote')}
      >
        {#snippet toolbar()}
          <button
            type="button"
            class={toolBtn}
            title="Download marked files to the local directory"
            disabled={remoteMarkedFiles.length === 0}
            onclick={download}
          >
            <Icon name="download" size={13} />
            Download
          </button>
          <button type="button" class={toolBtn} title="New folder" onclick={() => openPrompt('mkdir')}>
            <Icon name="plus" size={13} />
            Folder
          </button>
          <button
            type="button"
            class={toolBtn}
            title="Rename the marked entry"
            disabled={!singleRemoteMark}
            onclick={() => openPrompt('rename')}
          >
            <Icon name="edit" size={13} />
          </button>
          <button
            type="button"
            class={toolBtn}
            title="Delete marked entries"
            aria-label="Delete marked entries"
            disabled={remoteMarked.length === 0}
            onclick={remove}
          >
            <Icon name="trash" size={13} />
          </button>
          <button
            type="button"
            class={toolBtn}
            title="Refresh"
            aria-label="Refresh remote"
            onclick={() => refreshRemote(view.remote.path)}
          >
            <Icon name="refresh" size={13} />
          </button>
        {/snippet}
      </SftpPane>
    </div>

    {#if transfer}
      <div class="shrink-0 border-t border-default px-4 py-2.5" aria-label="transfer progress">
        <div class="flex items-center justify-between gap-3 text-xs text-muted">
          <span class="min-w-0 truncate">
            {transfer.kind === 'upload' ? 'Uploading' : 'Downloading'}
            <span class="font-mono text-fg">{transfer.name}</span>
          </span>
          <span class="shrink-0 tabular-nums">
            {formatBytes(transfer.done)}{transfer.total > 0
              ? ` / ${formatBytes(transfer.total)}`
              : ''}
          </span>
        </div>
        <div class="mt-1.5 h-1.5 overflow-hidden rounded-full bg-surface-inset">
          <div
            class="h-full rounded-full bg-accent transition-[width]"
            style="width: {transferPercent(transfer.done, transfer.total)}%"
          ></div>
        </div>
      </div>
    {:else if view.error}
      <div class="shrink-0 border-t border-default px-4 py-2 text-xs text-status-crit">
        {view.error}
      </div>
    {/if}
  {/if}
</div>

{#if active && prompt}
  <Modal label={prompt.kind === 'mkdir' ? 'New folder' : 'Rename'} onClose={() => (prompt = null)}>
    <form
      onsubmit={(e) => {
        e.preventDefault();
        submitPrompt();
      }}
    >
      <header class="border-b border-default px-5 py-3.5">
        <h2 class="text-sm font-semibold">
          {prompt.kind === 'mkdir' ? 'New folder' : `Rename ${prompt.target?.name ?? ''}`}
        </h2>
      </header>
      <div class="px-5 py-4">
        <!-- svelte-ignore a11y_autofocus -->
        <input
          autofocus
          bind:value={prompt.value}
          class={field}
          placeholder={prompt.kind === 'mkdir' ? 'Folder name' : 'New name'}
          aria-label={prompt.kind === 'mkdir' ? 'Folder name' : 'New name'}
        />
      </div>
      <footer class="flex justify-end gap-2 border-t border-default px-5 py-3">
        <button
          type="button"
          class="rounded-full px-4 py-2 text-sm text-muted transition hover:bg-surface-inset hover:text-fg"
          onclick={() => (prompt = null)}
        >
          Cancel
        </button>
        <button
          type="submit"
          class="rounded-full bg-accent px-5 py-2 text-sm font-medium text-accent-fg transition hover:opacity-90 disabled:opacity-50"
          disabled={!prompt.value.trim()}
        >
          {prompt.kind === 'mkdir' ? 'Create' : 'Rename'}
        </button>
      </footer>
    </form>
  </Modal>
{/if}

{#if active && view?.preview}
  <Modal label="File preview" onClose={closePreview}>
    <header class="border-b border-default px-5 py-3.5">
      <h2 class="truncate font-mono text-xs text-muted" title={view.preview.path}>
        {view.preview.path}
      </h2>
    </header>
    <div class="min-h-0 flex-1 overflow-auto px-5 py-4">
      {#if view.preview.content.length === 0}
        <p class="text-sm text-faint">Empty file.</p>
      {:else}
        <pre class="select-text whitespace-pre-wrap break-words font-mono text-xs text-fg">{view.preview
            .content}</pre>
      {/if}
    </div>
    <footer class="flex justify-end border-t border-default px-5 py-3">
      <button
        type="button"
        class="rounded-full px-4 py-2 text-sm text-muted transition hover:bg-surface-inset hover:text-fg"
        onclick={closePreview}
      >
        Close
      </button>
    </footer>
  </Modal>
{/if}
