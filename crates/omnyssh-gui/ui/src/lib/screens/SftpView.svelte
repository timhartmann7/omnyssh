<script lang="ts">
  // A live SFTP tab (tech-gui.md §3.2). One instance per SFTP session, kept mounted for
  // the session's life — hidden, not destroyed, when another entity is active — so pane
  // state survives tab switches. Opens the session on mount, drives both panes via the
  // sftp_* commands, and reads its per-session state from the sftp store (fed by the
  // `sftp-*` events, §3.4). Local browsing uses list_local_dir (returns directly);
  // remote uses sftp_list (arrives as an event). Semantic tokens only (§5.1).
  import { onMount, onDestroy } from 'svelte';
  import { homeDir } from '@tauri-apps/api/path';
  import { Icon } from '$lib/theme';
  import Modal from '$lib/components/Modal.svelte';
  import Select from '$lib/components/Select.svelte';
  import SftpPane from './SftpPane.svelte';
  import type { FileEntryDto } from '$lib/bindings';
  import { sessions, type Session } from '$lib/stores/sessions';
  import { sftp, markedEntries, formatBytes, rootOf, type PaneSide } from '$lib/stores/sftp';
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
      sftpCancel,
      listLocalDir,
      localDelete,
      localMkdir,
      localReadFile,
      localRename,
      localWriteFile,
      listLocalRoots,
      previewLocalFile,
      localPrepareExternalEdit,
      sftpPrepareExternalEdit,
      sftpOpenExternalEditor,
      sftpRemoveTempFile,
      sftpUploadDir,
      sftpDownloadDir
    } from '$lib/ipc/commands';

  import FileEditor from './FileEditor.svelte';
  import { fileEditors } from '$lib/stores/fileEditor';
  import { generalConfigStore } from '$lib/stores/generalConfig';

  let { session, active }: { session: Session; active: boolean } = $props();

  let backendId = $state<number | undefined>(undefined);
  let openError = $state<string | undefined>(undefined);
  let destroyed = false;
  let mirrored: string | undefined;
  let activeEditorPath = $state<string | null>(null);
  let infoPrompt = $state<string | null>(null);
  let deleteDialog = $state<{
  side: 'local' | 'remote';
  items: FileEntryDto[];
  } | null>(null);

  let localDeleteProgress = $state<{
    currentFile: string;
    filesDone: number;
    filesTotal: number;
  } | null>(null);
  let localDeleteCancelRequested = false;

  let activeEditorSide = $state<PaneSide>('remote');

  let mkdirDialog = $state<{
  side: 'local' | 'remote';
  parent: string;
  name: string;
  } | null>(null);

  let largeFilePrompt = $state<{
  entry: FileEntryDto;
  side: PaneSide;
  } | null>(null);

  let renameDialog = $state<{
  side: 'local' | 'remote';
  item: FileEntryDto;
  name: string;
  } | null>(null);

  let editorOpen = $state(false);
  let split = $state(100); // Browser starts full width
  let draggingSplit = $state(false);
  type RightPaneMode = 'preview' | 'editor';
  let rightPaneMode = $state<RightPaneMode>('preview');

  let externalEdit = $state<{  side: PaneSide;  path: string;  tempPath: string;} | null>(null);
  let uploadPrompt = $state<{  side: PaneSide;  path: string;  tempPath: string;} | null>(null);

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

  // Drive letters on Windows, where `..` stops at the drive the pane is on. A single
  // root elsewhere, and then there is nothing to switch.
  let roots = $state<string[]>([]);
  const drive = $derived(view ? rootOf(view.local.path, roots) : '');
  // What the selector shows: the drive the pane is on, back again after a drive that
  // failed to list (an empty card reader), so picking that one again still fires.
  let selectedDrive = $state('');
  $effect(() => {
    selectedDrive = drive;
  });

  async function switchDrive(root: string): Promise<void> {
    await refreshLocal(root);
    selectedDrive = drive;
  }
  const transfer = $derived(view?.transfer);

  const localMarked = $derived(
  view ? markedEntries(view.local) : []
  );

  const remoteMarked = $derived(
  view ? markedEntries(view.remote) : []
  );
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

  // Only the latest local listing lands: a network drive left for another can take
  // a long time to fail, and must not then replace the pane the user is on.
  let localSeq = 0;

  async function refreshLocal(path: string): Promise<void> {
    const id = backendId;
    if (id == null) return;
    const seq = ++localSeq;
    sftp.beginLoading(id, 'local');
    try {
      const entries = await listLocalDir(path);
      if (seq === localSeq) sftp.listing(id, 'local', path, entries);
    } catch (err) {
      if (seq === localSeq) sftp.paneError(id, 'local', errMsg(err));
    }
  }

  async function loadRoots(): Promise<void> {
    try {
      roots = await listLocalRoots();
    } catch {
      roots = [];
    }
  }

  function refreshRemote(path: string): void {
    const id = backendId;
    if (id == null) return;
    sftp.beginLoading(id, 'remote');
    void sftpList(id, path).catch((err) => sftp.paneError(id, 'remote', errMsg(err)));
  }

  function requestMkdir(side: 'local' | 'remote') {
  if (!view) return;

  mkdirDialog = {
    side,
    parent: side === 'local' ? view.local.path : view.remote.path,
    name: ''
  };
  }

  async function confirmMkdir() {
  if (!view || !mkdirDialog) return;

  const { side, parent, name } = mkdirDialog;
  if (!name.trim()) return;

  const path =
    side === 'local'
      ? joinLocal(parent, name)
      : joinRemote(parent, name);

  if (side === 'local') {
    await localMkdir(path);
    await refreshLocal(view.local.path);
  } else {
    sftp.pushOp(session.id, {
      kind: 'mkdir',
      name,
      refresh: 'remote'
    });

    await sftpMkdir(session.id, path);
  }

  mkdirDialog = null;
  }

  function requestRename(side: 'local' | 'remote') {
  if (!view) return;

  const pane = side === 'local' ? view.local : view.remote;
  const items = markedEntries(pane);

  if (items.length !== 1) return;

  renameDialog = {
    side,
    item: items[0],
    name: items[0].name
  };
  }

  async function confirmRename() {
  if (!view || !renameDialog) return;

  const { side, item, name } = renameDialog;

  let parent: string;
  let dest: string;

  if (side === 'local') {
    const separator =
      item.path.includes('\\') && !item.path.includes('/')
        ? '\\'
        : '/';
    const separatorIndex = item.path.lastIndexOf(separator);

    if (separatorIndex < 0) {
      return;
    }

    parent = item.path.substring(0, separatorIndex);
    dest = joinLocal(parent, name);
  } else {
    parent = item.path.substring(0, item.path.lastIndexOf('/'));
    dest = joinRemote(parent, name);
  }

  if (side === 'local') {
    await localRename(item.path, dest);
    await refreshLocal(view.local.path);
  } else {
    sftp.pushOp(session.id, {
      kind: 'rename',
      name,
      refresh: 'remote'
    });

    await sftpRename(session.id, item.path, dest);
  }

  renameDialog = null;
  }

  function requestDelete(side: 'local' | 'remote') {
  if (!view) return;

  const pane = side === 'local' ? view.local : view.remote;
  const items = markedEntries(pane);

  if (items.length === 0) return;

  deleteDialog = { side, items };
  }

  async function collectLocalDeleteTargets(
    path: string,
    targets: Array<{ path: string; name: string; isDir: boolean }>,
  ): Promise<void> {
    const entries = await listLocalDir(path);

    for (const entry of entries) {
      if (localDeleteCancelRequested) return;
      if (entry.name === '..') continue;

      if (entry.isDir) {
        await collectLocalDeleteTargets(entry.path, targets);
      } else {
        targets.push({ path: entry.path, name: entry.name, isDir: false });
      }
    }

    // Remove directories after their children so cancellation can stop between entries.
    targets.push({
      path,
      name: path.split(/[\\/]/).filter(Boolean).pop() ?? path,
      isDir: true
    });
  }

  async function deleteLocalCancellable(items: FileEntryDto[]): Promise<void> {
    const targets: Array<{ path: string; name: string; isDir: boolean }> = [];

    localDeleteProgress = {
      currentFile: 'Preparing deletion…',
      filesDone: 0,
      filesTotal: 0
    };

    for (const item of items) {
      if (localDeleteCancelRequested) return;

      if (item.isDir) {
        await collectLocalDeleteTargets(item.path, targets);
      } else {
        targets.push({ path: item.path, name: item.name, isDir: false });
      }
    }

    if (targets.length === 0) return;

    localDeleteProgress = {
      currentFile: targets[0].name,
      filesDone: 0,
      filesTotal: targets.length
    };

    for (const target of targets) {
      if (localDeleteCancelRequested) break;

      localDeleteProgress = {
        currentFile: target.name,
        filesDone: localDeleteProgress?.filesDone ?? 0,
        filesTotal: targets.length
      };

      await localDelete(target.path);

      localDeleteProgress = {
        currentFile: target.name,
        filesDone: (localDeleteProgress?.filesDone ?? 0) + 1,
        filesTotal: targets.length
      };
    }
  }

  async function confirmDelete() {
    if (!view || !deleteDialog) return;

    const { side, items } = deleteDialog;

    if (side === 'local') {
      deleteDialog = null;
      localDeleteCancelRequested = false;

      try {
        await deleteLocalCancellable(items);
        await refreshLocal(view.local.path);
      } catch (err) {
        lastError.set(errMsg(err));
      } finally {
        localDeleteProgress = null;
      }
      return;
    }

    const id = backendId;
    if (id == null) return;

    for (const item of items) {
      sftp.pushOp(id, {
        kind: 'delete',
        name: item.name,
        refresh: 'remote'
      });

      await sftpDelete(id, item.path);
    }

    deleteDialog = null;
  }

  function cancelDelete() {
    if (localDeleteProgress) {
      localDeleteCancelRequested = true;
      return;
    }

    deleteDialog = null;
  }

  function startSplitDrag() {
  draggingSplit = true;
  }

  function stopSplitDrag() {
  draggingSplit = false;
  }

  function dragSplit(event: PointerEvent) {
  if (!draggingSplit) return;

  const root = (event.currentTarget as HTMLElement).getBoundingClientRect();
  const pct = ((event.clientX - root.left) / root.width) * 100;

  split = Math.max(25, Math.min(75, pct));
  }



  onMount(() => {
    void (async () => {
    try {
      await generalConfigStore.load();
    } catch {
      // Fall back to defaults if the config cannot be loaded.
    }
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
      void loadRoots();
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


  // When the active download completes, launch the external editor.
  $effect(() => {
    if (!view || !externalEdit || !view.transfer) return;

    const transfer = view.transfer;

    if (
      transfer.kind !== 'download' ||
      transfer.bytesTotal <= 0 ||
      transfer.bytesDone < transfer.bytesTotal
    ) {
      return;
    }

    const pending = externalEdit;
    externalEdit = null;

    void (async () => {
      const modified = await sftpOpenExternalEditor(
        pending.tempPath,
        generalConfigStore.value?.externalEditor ?? "vim",
      );

      if (!modified) {
        await sftpRemoveTempFile(pending.tempPath);
        infoPrompt = "The file was closed without any modifications.";
        return;
      }

      const prompt = {
        side: pending.side,
        path: pending.path,
        tempPath: pending.tempPath,
      };

      if (generalConfigStore.value?.autoUploadExternal) {
        uploadPrompt = prompt;
        await uploadExternalEdit();
      } else {
        uploadPrompt = prompt;
      }
    })();
  });

  function navigate(side: PaneSide, entry: FileEntryDto): void {
    if (side === 'local') void refreshLocal(entry.path);
    else refreshRemote(entry.path);
  }

  function toggleMark(side: PaneSide, path: string): void {
    if (backendId != null) sftp.toggleMark(backendId, side, path);
  }

async function preview(side: PaneSide, entry: FileEntryDto): Promise<void> {
  if (entry.isDir) {
    editorOpen = false;
    return;
  }

  editorOpen = true;
  rightPaneMode = 'preview';
  split = 55;

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
    try {
      await sftpPreview(id, entry.path);
    } catch (err) {
      lastError.set(errMsg(err));
    }
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
    ...markedEntries(view.local).map((entry) => () => {
      sftp.pushOp(id, {
        kind: 'upload',
        name: entry.name,
        refresh: 'remote'
      });

      if (entry.isDir) {
        void sftpUploadDir(
          id,
          entry.path,
          joinRemote(dir, entry.name)
        ).catch(onDispatchError(id));
      } else {
        void sftpUpload(
          id,
          entry.path,
          joinRemote(dir, entry.name)
        ).catch(onDispatchError(id));
      }
    })
  );
}

function download(): void {
  const id = backendId;
  if (id == null || !view) return;

  const dir = view.local.path;

  enqueue(
    ...markedEntries(view.remote).map((entry) => () => {
      sftp.pushOp(id, {
        kind: 'download',
        name: entry.name,
        refresh: 'local'
      });

      if (entry.isDir) {
        void sftpDownloadDir(
          id,
          entry.path,
          joinLocal(dir, entry.name)
        ).catch(onDispatchError(id));
      } else {
        void sftpDownload(
          id,
          joinLocal(dir, entry.name),
          entry.path
        ).catch(onDispatchError(id));
      }
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

  function shouldUseExternal(entry: FileEntryDto): boolean {
  const limitMb = generalConfigStore.value?.largeFileMb ?? 5;
  return entry.size >= limitMb * 1024 * 1024;
}

async function editEntry(entry: FileEntryDto, side: PaneSide) {
  if (entry.isDir) return;

  // Large REMOTE files → external editor prompt
  if (shouldUseExternal(entry)) {
    largeFilePrompt = { entry, side };
    return;
  }

  activeEditorPath = entry.path;
  activeEditorSide = side;

  if (side === 'local') {
    fileEditors.openLocal(entry.path);
  } else {
    if (backendId == null) return;
    fileEditors.open(backendId, entry.path);
  }

  rightPaneMode = 'editor';
  editorOpen = true;
  split = 55;
}

function openBuiltinEditor(): void {
  // Large files are external only.
}

async function openExternalEditor(): Promise<void> {
  if (!largeFilePrompt) return;

  const { entry, side } = largeFilePrompt;

  if (side === "remote" && backendId == null) return;

  const tempPath =
    side === "local"
      ? await localPrepareExternalEdit(entry.path)
      : await sftpPrepareExternalEdit(entry.path);

  externalEdit = {
    side,
    path: entry.path,
    tempPath,
  };

  largeFilePrompt = null;

  // Remote: download first, editor opens after download completes
  if (side === "remote") {
    const id = backendId;

    if (id == null) return;

    sftp.pushOp(id, {
      kind: "download",
      name: entry.path.split("/").pop() ?? entry.path,
      refresh: "local",
    });

    await sftpDownload(id, tempPath, entry.path);
    return;
  }

  // Local: open immediately
  const editor = generalConfigStore.value?.externalEditor ?? "vim";
  const modified = await sftpOpenExternalEditor(tempPath, editor);

  if (!modified) {
    await sftpRemoveTempFile(tempPath);
    return;
  }

  const prompt = {
    side,
    path: entry.path,
    tempPath,
  };

  if (generalConfigStore.value?.autoUploadExternal) {
    uploadPrompt = prompt;
    await uploadExternalEdit();
  } else {
    uploadPrompt = prompt;
  }
}

async function uploadExternalEdit(): Promise<void> {
  if (!uploadPrompt) return;

  const { side, path, tempPath } = uploadPrompt;

  uploadPrompt = null;

  // Local file: write back directly
  if (side === 'local') {
    const text = await localReadFile(tempPath);
    await localWriteFile(path, text);
    await sftpRemoveTempFile(tempPath);
    externalEdit = null;
    return;
  }

  // Remote file requires an active session
  if (backendId == null) return;

  sftp.pushOp(backendId, {
    kind: 'upload',
    name: path.split('/').pop() ?? path,
    refresh: 'remote',
  });

  await sftpUpload(backendId, tempPath, path);
  await sftpRemoveTempFile(tempPath);
  externalEdit = null;
}



  function transferPercent(bytesDone: number, bytesTotal: number): number {
  return bytesTotal > 0
    ? Math.min(100, Math.round((bytesDone / bytesTotal) * 100))
    : 0;
  }

  const toolBtn =
    'inline-flex items-center gap-1 rounded-full border border-default px-2 py-1 text-xs ' +
    'font-medium text-muted transition hover:border-strong hover:bg-accent hover:text-accent-fg ' +
    'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus ' +
    'disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent ' +
    'disabled:hover:text-muted disabled:hover:border-default';
  const driveSelect =
    'rounded-full border border-default bg-surface py-1 pl-2.5 text-xs font-medium text-muted ' +
    'transition hover:border-strong hover:text-fg ' +
    'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus';
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
  <!-- Complete SFTP workspace -->
  <div class="flex min-h-0 flex-1 flex-col">
    <!-- Browser + shared Preview/Editor side pane -->
    <div
      class="flex min-h-0 flex-1 overflow-hidden"
      onpointermove={dragSplit}
      onpointerup={stopSplitDrag}
      onpointerleave={stopSplitDrag}
      role="group"
      aria-label="SFTP file browser"
    >
      <!-- Local + Remote browser -->
      <div
        class="grid min-h-0 divide-x divide-default"
        style="width: {editorOpen ? `${split}%` : '100%'}; grid-template-columns: repeat(2, minmax(0, 1fr));"
      >
        <!-- Local -->
        <SftpPane
          title="Local"
          pane={view.local}
          onNavigate={(e) => navigate('local', e)}
          onToggleMark={(p) => toggleMark('local', p)}
          onPreview={(e) => preview('local', e)}
          onEdit={(entry) => editEntry(entry, 'local')}
        >
          {#snippet toolbar()}
            {#if roots.length > 1}
              <Select
                bind:value={selectedDrive}
                class={driveSelect}
                aria-label="Local drive"
                title="Switch drive"
                onchange={(e) => switchDrive(e.currentTarget.value)}
              >
                {#if !drive}
                  <option value="" disabled>Drive</option>
                {/if}

                {#each roots as root (root)}
                  <option value={root}>{root.replace(/[\\/]$/, '')}</option>
                {/each}
              </Select>
            {/if}

            <button
              type="button"
              class={toolBtn}
              title="Upload marked files"
              disabled={localMarked.length === 0}
              onclick={upload}
            >
              <Icon name="upload" size={13}/>
              Upload
            </button>

            <button
              type="button"
              class={toolBtn}
              title="Edit selected file"
              disabled={localMarked.length !== 1 || localMarked[0].isDir}
              onclick={() => editEntry(localMarked[0], 'local')}
            >
              <Icon name="edit" size={13}/>
              Edit
            </button>

            <button
              type="button"
              class={toolBtn}
              title="New folder"
              onclick={() => requestMkdir('local')}
            >
              <Icon name="plus" size={13}/>
              Folder
            </button>

            <button
              type="button"
              class={toolBtn}
              title="Rename"
              disabled={localMarked.length !== 1}
              onclick={() => requestRename('local')}
            >
              <Icon name="edit" size={13}/>
            </button>

            <button
              type="button"
              class={toolBtn}
              title="Delete"
              disabled={localMarked.length === 0}
              onclick={() => requestDelete('local')}
            >
              <Icon name="trash" size={13}/>
            </button>

            <button
              type="button"
              class={toolBtn}
              title="Refresh"
              onclick={() => {
                void loadRoots();
                void refreshLocal(view.local.path);
              }}
            >
              <Icon name="refresh" size={13}/>
            </button>
          {/snippet}
        </SftpPane>

        <!-- Remote -->
        <SftpPane
          title={session.hostName}
          pane={view.remote}
          onNavigate={(e) => navigate('remote', e)}
          onToggleMark={(p) => toggleMark('remote', p)}
          onPreview={(e) => preview('remote', e)}
          onEdit={(entry) => editEntry(entry, 'remote')}
        >
          {#snippet toolbar()}
            <button
              type="button"
              class={toolBtn}
              title="Download marked files"
              disabled={remoteMarked.length === 0}
              onclick={download}
            >
              <Icon name="download" size={13}/>
              Download
            </button>

            <button
              type="button"
              class={toolBtn}
              title="Edit selected file"
              disabled={!singleRemoteMark || singleRemoteMark.isDir}
              onclick={() => {
                if (!singleRemoteMark || singleRemoteMark.isDir) return;
                editEntry(singleRemoteMark, 'remote');
              }}
            >
              <Icon name="edit" size={13}/>
              Edit
            </button>

            <button
              type="button"
              class={toolBtn}
              title="New folder"
              onclick={() => openPrompt('mkdir')}
            >
              <Icon name="plus" size={13}/>
              Folder
            </button>

            <button
              type="button"
              class={toolBtn}
              title="Rename"
              disabled={!singleRemoteMark}
              onclick={() => openPrompt('rename')}
            >
              <Icon name="edit" size={13}/>
            </button>

            <button
              type="button"
              class={toolBtn}
              title="Delete"
              disabled={remoteMarked.length === 0}
              onclick={() => requestDelete('remote')}
            >
              <Icon name="trash" size={13}/>
            </button>

            <button
              type="button"
              class={toolBtn}
              title="Refresh"
              onclick={() => refreshRemote(view.remote.path)}
            >
              <Icon name="refresh" size={13}/>
            </button>
          {/snippet}
        </SftpPane>
      </div>

      {#if editorOpen}
        <!-- Resizable splitter -->
        <div
          class="flex w-2 shrink-0 cursor-col-resize items-center justify-center bg-border hover:bg-border-strong"
          onpointerdown={startSplitDrag}
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize preview or editor pane"
        >
          <div class="space-y-1">
            <div class="h-1 w-1 rounded-full bg-text-faint"></div>
            <div class="h-1 w-1 rounded-full bg-text-faint"></div>
            <div class="h-1 w-1 rounded-full bg-text-faint"></div>
          </div>
        </div>

        <!-- Shared Preview / Editor pane -->
        <div class="relative flex min-w-0 flex-1 flex-col border-l border-default bg-surface">
          <!-- Pane header -->
          <div class="flex shrink-0 items-center justify-between border-b border-default px-3 py-2">
            <div class="min-w-0">
              <div class="truncate font-mono text-sm">
                {#if rightPaneMode === 'preview'}
                  {view.preview?.path?.split('/').pop() ?? 'Preview'}
                {:else}
                  {activeEditorPath?.split('/').pop() ?? 'Editor'}
                {/if}
              </div>

              <div class="truncate text-xs text-muted">
                {#if rightPaneMode === 'preview'}
                  {view.preview?.path ?? 'No file selected'}
                {:else}
                  {activeEditorPath ?? 'No file selected'}
                {/if}
              </div>
            </div>

            <div class="flex shrink-0 items-center gap-1">
              <button
                type="button"
                class={toolBtn}
                title="Close pane"
                onclick={() => {
                  editorOpen = false;
                  split = 100;
                  if (activeEditorPath) {
                    if (activeEditorSide === 'local') {
                      fileEditors.close(
                        { kind: 'local' },
                        activeEditorPath
                      );
                    } else if (backendId != null) {
                      fileEditors.close(
                        { kind: 'remote', sessionId: backendId },
                        activeEditorPath
                      );
                    }
                  }

                  activeEditorPath = null;
                  closePreview();
                }}
              >
                <Icon name="close" size={14}/>
              </button>
            </div>
          </div>

          <!-- Pane content -->
          <div class="min-h-0 flex-1 overflow-hidden">
            {#if rightPaneMode === 'preview'}
              {#if view.preview}
                <div class="flex h-full min-h-0 flex-col">
                  <div class="min-h-0 flex-1 overflow-auto px-4 py-4">
                    {#if view.preview.content.length === 0}
                      <div class="flex h-full items-center justify-center">
                        <p class="text-sm text-faint">Empty file.</p>
                      </div>
                    {:else}
                      <pre class="select-text whitespace-pre-wrap break-words font-mono text-xs leading-relaxed text-fg">{view.preview.content}</pre>
                    {/if}
                  </div>
                </div>
              {:else}
                <div class="flex h-full items-center justify-center p-10 text-center">
                  <p class="text-sm text-muted">Select a file to preview.</p>
                </div>
              {/if}
            {:else}
              {#if activeEditorPath}
                <div class="h-full min-h-0 overflow-hidden">
                  {#if activeEditorSide === 'local'}
                    <FileEditor
                      path={activeEditorPath}
                      local={true}
                    />
                  {:else if backendId != null}
                    <FileEditor
                      sessionId={backendId}
                      path={activeEditorPath}
                    />
                  {/if}
                </div>
              {:else}
                <div class="flex h-full items-center justify-center p-10 text-center">
                  <p class="text-sm text-muted">Select a file to edit.</p>
                </div>
              {/if}
            {/if}
          </div>
        </div>
      {/if}
    </div>

    <!-- Transfer progress -->

      <!-- Transfer progress -->
      {#if transfer}
        <div class="shrink-0 border-t border-default px-4 py-2.5" aria-label="transfer progress">
          <div class="flex items-start justify-between gap-3">
            <div class="min-w-0 flex-1">
              {#if transfer.stage === 'preparing'}
                <div class="truncate text-xs text-muted">
                  {transfer.kind === 'delete' ? 'Preparing delete…' : `Preparing ${transfer.kind}…`}
                </div>

                <div class="truncate font-mono text-sm text-fg">
                  {transfer.rootName}
                </div>

                <div class="truncate text-xs text-muted">
                  {transfer.currentFile}
                </div>
              {:else}
                <div class="truncate text-xs text-muted">
                  {transfer.kind === 'upload'
                    ? 'Uploading'
                    : transfer.kind === 'download'
                      ? 'Downloading'
                      : 'Deleting'}
                  <span class="ml-1 font-mono text-fg">{transfer.rootName}</span>
                </div>

                <div class="truncate font-mono text-sm text-fg">
                  {transfer.currentFile}
                </div>
              {/if}
            </div>

            <div class="shrink-0 text-right">
              {#if transfer.stage === 'preparing'}
                <div class="text-xs text-muted">Scanning…</div>
              {:else if transfer.kind === 'delete'}
                <div class="text-xs tabular-nums text-muted">
                  {transfer.filesDone} / {transfer.filesTotal} items
                </div>
              {:else}
                <div class="text-xs tabular-nums text-muted">
                  {transfer.filesDone} / {transfer.filesTotal} files
                </div>

                <div class="text-xs tabular-nums text-muted">
                  {formatBytes(transfer.bytesDone)} / {formatBytes(transfer.bytesTotal)}
                </div>
              {/if}

              <button
                class="mt-2 w-full rounded border border-red-500 px-2 py-1 text-xs text-red-500 hover:bg-red-500/10"
                onclick={async () => {
                  const id = backendId;
                  if (id == null || !transfer?.transferId) return;
                  await sftpCancel(id, transfer.transferId);
                }}
              >
                Cancel
              </button>
            </div>
          </div>

          <div class="mt-2 h-1.5 overflow-hidden rounded-full bg-surface-inset">
            <div
              class="h-full rounded-full bg-accent transition-[width]"
              style="width: {transferPercent(transfer.bytesDone, transfer.bytesTotal)}%"
            ></div>
          </div>
        </div>
      {:else if view.error}
        <div class="shrink-0 border-t border-default px-4 py-2 text-xs text-status-crit">
          {view.error}
        </div>
      {/if}
    </div>
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


{#if active && largeFilePrompt}
  <Modal
    label="Large file"
    onClose={() => (largeFilePrompt = null)}
  >
    <header class="border-b border-default px-5 py-3.5">
      <h2 class="text-sm font-semibold">Open large file?</h2>
    </header>

    <div class="space-y-3 px-5 py-4">
      <p class="font-medium">{largeFilePrompt.entry.name}</p>
      <p class="text-sm text-muted">Size: {formatBytes(largeFilePrompt.entry.size)}</p>
      <p class="text-sm text-muted">
        This exceeds your configured threshold of {generalConfigStore.value?.largeFileMb ?? 5} MB.
      </p>
      <p class="text-sm text-muted">
        {#if largeFilePrompt.side === 'remote'}
          This file will be downloaded to a temporary location and opened in your configured external editor.
        {:else}
          This file will be copied to a temporary location and opened in your configured external editor.
        {/if}
      </p>
    </div>

    <footer class="flex justify-end gap-2 border-t border-default px-5 py-3">
      <button type="button" class="rounded-full px-4 py-2 text-sm hover:bg-surface-inset" onclick={() => (largeFilePrompt = null)}>Cancel</button>
      <button type="button" class="rounded-full bg-accent px-5 py-2 text-sm font-medium text-accent-fg" onclick={openExternalEditor}>Open External</button>
    </footer>
  </Modal>
{/if}

{#if active && uploadPrompt}
  <Modal label="Save changes?" onClose={() => (uploadPrompt = null)}>
    <header class="border-b border-default px-5 py-3.5">
      <h2 class="text-sm font-semibold">
        {#if uploadPrompt.side === 'local'}
          Save modified file?
        {:else}
          Upload modified file?
        {/if}
      </h2>
    </header>

    <div class="space-y-2 px-5 py-4">
      <p class="font-medium">{uploadPrompt.path.split('/').pop()}</p>

      {#if uploadPrompt.side === 'local'}
        <p class="text-sm text-muted">
          The file was modified in your external editor and will be saved back to its original location.
        </p>
      {:else}
        <p class="text-sm text-muted">
          The file was modified in your external editor and will be uploaded back to the server.
        </p>
      {/if}
    </div>

    <footer class="flex justify-end gap-2 border-t border-default px-5 py-3">
      <button
        type="button"
        class="rounded-full px-4 py-2 text-sm hover:bg-surface-inset"
        onclick={() => (uploadPrompt = null)}
      >
        Discard
      </button>

      <button
        type="button"
        class="rounded-full bg-accent px-5 py-2 text-sm font-medium text-accent-fg"
        onclick={uploadExternalEdit}
      >
        {#if uploadPrompt.side === 'local'}
          Save
        {:else}
          Upload
        {/if}
      </button>
    </footer>
  </Modal>
{/if}

{#if active && infoPrompt}
  <Modal
    label="No changes detected"
    onClose={() => (infoPrompt = null)}
  >
    <div class="space-y-3 px-5 py-4">
      <p class="text-sm text-muted">{infoPrompt}</p>
    </div>

    <footer class="flex justify-end border-t border-default px-5 py-3">
      <button
        type="button"
        class="rounded-full bg-accent px-5 py-2 text-sm font-medium text-accent-fg"
        onclick={() => (infoPrompt = null)}
      >
        OK
      </button>
    </footer>
  </Modal>
{/if}

{#if deleteDialog}
  <div class="absolute inset-0 z-50 flex items-center justify-center bg-black/60">
    <div class="w-[420px] rounded-xl border border-default bg-surface p-5 shadow-xl">
      <h2 class="text-lg font-semibold text-fg">
        Confirm deletion
      </h2>

      {#if deleteDialog.items.length === 1}
        <p class="mt-3 text-sm text-muted">
          {#if deleteDialog.items[0].isDir}
            Delete folder
          {:else}
            Delete file
          {/if}
        </p>

        <p class="mt-1 truncate font-mono text-sm text-fg">
          {deleteDialog.items[0].name}
        </p>

        {#if deleteDialog.items[0].isDir}
          <p class="mt-3 text-sm text-red-400">
            This will permanently remove the folder and all of its contents.
          </p>
        {/if}
      {:else}
        <p class="mt-3 text-sm text-muted">
          Permanently delete these items?
        </p>

        <p class="mt-2 text-lg font-semibold text-fg">
          {deleteDialog.items.length} selected items
        </p>
      {/if}

	<button
	  type="button"
	  class="rounded-md border border-default px-3 py-1.5 text-sm hover:bg-surface-hover"
	  onclick={cancelDelete}
	>
	  Cancel
	</button>

	<button
	  type="button"
	  class="rounded-md px-3 py-1.5 text-sm font-medium text-white transition-opacity hover:opacity-90"
	  style="background: var(--status-crit);"
	  onclick={confirmDelete}
	>
	  Delete
	</button>
      </div>
    </div>
{/if}

{#if localDeleteProgress}
  <div class="absolute inset-0 z-50 flex items-center justify-center bg-black/60">
    <div class="w-[min(32rem,calc(100vw-2rem))] rounded-xl border border-default bg-surface p-5 shadow-xl">
      <h3 class="text-lg font-semibold text-fg">Deleting local files</h3>
      <p class="mt-2 truncate text-sm text-muted">{localDeleteProgress.currentFile}</p>

      {#if localDeleteProgress.filesTotal > 0}
        <div class="mt-4 flex items-center justify-between text-xs text-muted">
          <span>{localDeleteProgress.filesDone} / {localDeleteProgress.filesTotal}</span>
          <span>{Math.round((localDeleteProgress.filesDone / localDeleteProgress.filesTotal) * 100)}%</span>
        </div>
        <div class="mt-2 h-1.5 overflow-hidden rounded-full bg-surface-inset">
          <div
            class="h-full transition-[width]"
            style={`width: ${Math.min(100, (localDeleteProgress.filesDone / localDeleteProgress.filesTotal) * 100)}%`}
          ></div>
        </div>
      {:else}
        <p class="mt-4 text-sm text-muted">Scanning directory contents…</p>
      {/if}

      <div class="mt-5 flex justify-end">
        <button
          type="button"
          class="rounded-md border border-default px-3 py-1.5 text-sm hover:bg-surface-hover"
          onclick={cancelDelete}
        >
          Cancel
        </button>
      </div>
    </div>
  </div>
{/if}

{#if mkdirDialog}
  <div class="absolute inset-0 z-50 flex items-center justify-center bg-black/60">
    <div class="w-[400px] rounded-xl border border-default bg-surface p-5 shadow-xl">
      <h2 class="text-lg font-semibold text-fg">New Folder</h2>

      <div class="mt-4">
          <label for="mkdir-folder-name" class="mb-1 block text-xs text-muted">
            Folder name
          </label>
        <input
          id="mkdir-folder-name"
          class="w-full rounded-md border border-default bg-surface-inset px-3 py-2 text-sm text-fg outline-none focus:ring-2"
          bind:value={mkdirDialog.name}
          placeholder="New Folder"
        />
      </div>

      <div class="mt-5 flex justify-end gap-2">
        <button
          type="button"
          class="rounded-md border border-default px-3 py-1.5 text-sm hover:bg-surface-inset"
          onclick={() => (mkdirDialog = null)}
        >
          Cancel
        </button>

        <button
          type="button"
          class="rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-accent-fg hover:opacity-90"
          onclick={confirmMkdir}
        >
          Create
        </button>
      </div>
    </div>
  </div>
{/if}

{#if renameDialog}
  <div class="absolute inset-0 z-50 flex items-center justify-center bg-black/60">
    <div class="w-[400px] rounded-xl border border-default bg-surface p-5 shadow-xl">
      <h2 class="text-lg font-semibold text-fg">Rename</h2>

      <div class="mt-4">
          <label for="rename-name" class="mb-1 block text-xs text-muted">
            Name
          </label>
          <input
          id="rename-name"
          class="w-full rounded-md border border-default bg-surface-inset px-3 py-2 text-sm text-fg outline-none focus:ring-2"
          bind:value={renameDialog.name}
        />
      </div>

      <div class="mt-5 flex justify-end gap-2">
        <button
          type="button"
          class="rounded-md border border-default px-3 py-1.5 text-sm hover:bg-surface-inset"
          onclick={() => (renameDialog = null)}
        >
          Cancel
        </button>

        <button
          type="button"
          class="rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-accent-fg hover:opacity-90"
          onclick={confirmRename}
        >
          Rename
        </button>
      </div>
    </div>
  </div>
{/if}
