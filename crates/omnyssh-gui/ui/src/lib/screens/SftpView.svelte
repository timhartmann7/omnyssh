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
  import { Button, Icon } from '$lib/theme';
  import Modal from '$lib/components/Modal.svelte';
  import Select from '$lib/components/Select.svelte';
  import SftpPane from './SftpPane.svelte';
  import type { FileEntryDto } from '$lib/bindings';
  import { sessions, type Session } from '$lib/stores/sessions';
  import {
    sftp,
    markedEntries,
    dragPayload,
    clashCount,
    repeatedName,
    formatBytes,
    rootOf,
    baseName,
    isPlainName,
    transferState,
    CANCELLED,
    type PaneSide
  } from '$lib/stores/sftp';
  import { lastError } from '$lib/stores/notifications';
  import { maskText } from '$lib/stores/streamer';
  import { dropPoint, isMac, isWindows } from '$lib/platform';
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
    sftpCancelTransfer,
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
    sftpRemoveTempFile
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

  let externalEdit = $state<{ side: PaneSide; path: string; tempPath: string; queued?: boolean } | null>(null);
  let pendingUploadTemp = $state<string | null>(null);
  let uploadPrompt = $state<{  side: PaneSide;  path: string;  tempPath: string;} | null>(null);

  // Queued mutations, dispatched one at a time (see the pump effect). The core's SFTP
  // command channel is bounded and drops on overflow, so a large batch fired at once
  // would silently lose commands and wedge the op-done FIFO; gating on the previous
  // op's completion keeps at most one command outstanding. Transfers are flagged so a
  // cancel drops the queued ones too.
  let outbox = $state<Array<{ transfer: boolean; run: () => void }>>([]);

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
  const transfer = $derived(view ? transferState(view) : undefined);

  // Folders transfer whole, so the Upload/Download buttons take them like files.
  const localMarked = $derived(view ? markedEntries(view.local) : []);
  const remoteMarked = $derived(view ? markedEntries(view.remote) : []);
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
  const id = backendId;
  if (id == null || !view || !mkdirDialog) return;

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
    enqueue([
      () => {
        sftp.pushOp(id, { kind: 'mkdir', name, refresh: 'remote' });
        void sftpMkdir(id, path).catch(onDispatchError(id));
      }
    ]);
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
  const id = backendId;
  if (id == null || !view || !renameDialog) return;

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
    enqueue([
      () => {
        sftp.pushOp(id, { kind: 'rename', name, refresh: 'remote' });
        void sftpRename(id, item.path, dest).catch(onDispatchError(id));
      }
    ]);
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

    // Keep remote operations in the existing queue so their completion
    // events remain in FIFO order, even when multiple entries are selected.
    enqueue(items.map((item) => () => {
      sftp.pushOp(id, {
        kind: 'delete',
        name: item.name,
        refresh: 'remote'
      });
      void sftpDelete(id, item.path).catch(onDispatchError(id));
    }));

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
    next.run();
  });

  // Re-list the affected pane once every queued mutation has drained — the FS changed
  // (§3.2). Gated on an empty outbox so a batch re-lists once at the end, not per op.
  $effect(() => {
    const id = backendId;
    if (id == null || !view || view.pending.length > 0 || outbox.length > 0 || !view.refresh) return;
    const target = view.refresh;
    sftp.clearRefresh(id);
    // A listing asked for meanwhile is where the user went; the old path again would
    // take the pane back. A remote one also waited in the core's queue behind the
    // transfer, so it shows the change.
    if ((target === 'local' || target === 'both') && !view.local.loading) {
      void refreshLocal(view.local.path);
    }
    if ((target === 'remote' || target === 'both') && !view.remote.loading) {
      refreshRemote(view.remote.path);
    }
  });


  // When the active download completes, launch the external editor.
  $effect(() => {
    if (!view || !externalEdit || externalEdit.queued || !view.transfer) return;

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

  // If a queued external download is cancelled before dispatch, or fails before it
  // produces a complete progress event, discard its temporary copy.
  $effect(() => {
    if (!view || !externalEdit || view.pending.length > 0 || outbox.length > 0) return;
    if (externalEdit.queued || !view.transfer) {
      const tempPath = externalEdit.tempPath;
      externalEdit = null;
      void sftpRemoveTempFile(tempPath).catch((err) => lastError.set(errMsg(err)));
    }
  });

  // A remote upload reads its temp file asynchronously. Remove it only after the
  // matching queued operation has reported completion, never immediately after IPC.
  $effect(() => {
    if (!view || !pendingUploadTemp || view.pending.length > 0 || outbox.length > 0) return;
    const tempPath = pendingUploadTemp;
    pendingUploadTemp = null;
    void sftpRemoveTempFile(tempPath).catch((err) => lastError.set(errMsg(err)));
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

  function enqueue(actions: Array<() => void>, transfer = false): void {
    if (!actions.length) return;
    // Clear the prior batch's lingering error only when starting from idle. Piling onto a
    // batch that is still draining must not wipe a failure it already recorded (that error
    // stays visible until the next fresh action — see applyOpDone).
    const draining = outbox.length > 0 || (view?.pending.length ?? 0) > 0;
    if (backendId != null && !draining) sftp.clearError(backendId);
    outbox = [...outbox, ...actions.map((run) => ({ transfer, run }))];
  }

  // Stops the running transfer and drops the queued ones; the core cancels what was
  // sent before this, so a transfer started afterwards runs.
  function cancelTransfers(): void {
    const id = backendId;
    if (id == null) return;
    outbox = outbox.filter((op) => !op.transfer);
    void sftpCancel(id).catch((err) => lastError.set(errMsg(err)));
  }

  // A transfer held back until the user agrees to replace what is there.
  let overwrite = $state<{ count: number; dir: string; run: () => void } | null>(null);

  // Runs a transfer of `names` into `dir`, asking first when the listing on show there
  // already has some of them. A folder row's listing is unknown, so a drop on one goes.
  // Two items bound for one name do not go: the second would replace the first unasked.
  function unlessClashing(side: PaneSide, names: string[], dir: string, run: () => void): void {
    const pane = view?.[side];
    const caseless = side === 'local' && (isWindows || isMac);
    const twice = repeatedName(names, caseless);
    if (twice) {
      lastError.set(`Two of the items would both be '${twice}' in ${dir}.`);
      return;
    }
    const count = pane?.path === dir ? clashCount(names, pane.entries, caseless) : 0;
    if (count > 0) overwrite = { count, dir, run };
    else run();
  }

  function replace(): void {
    const run = overwrite?.run;
    overwrite = null;
    run?.();
  }

  // `dir` defaults to the other pane's current directory; a drop onto a folder row
  // passes that folder instead.
  function upload(files: Array<Pick<FileEntryDto, 'name' | 'path'>>, dir = view?.remote.path): void {
    const id = backendId;
    if (id == null || !view || dir == null) return;
    unlessClashing('remote', files.map((file) => file.name), dir, () =>
      enqueue(
        files.map((file) => () => {
          sftp.pushOp(id, { kind: 'upload', name: file.name, refresh: 'remote' });
          void sftpUpload(id, file.path, joinRemote(dir, file.name)).catch(onDispatchError(id));
        }),
        true
      )
    );
  }

  // The local path is the listed name joined onto `dir`, so only a plain name goes.
  function download(files: FileEntryDto[], dir = view?.local.path): void {
    const id = backendId;
    if (id == null || !view || dir == null) return;
    const odd = files.find((file) => !isPlainName(file.name, isWindows));
    if (odd) lastError.set(`'${odd.name}' is not a name that can be created here.`);
    const plain = files.filter((file) => isPlainName(file.name, isWindows));
    unlessClashing('local', plain.map((file) => file.name), dir, () =>
      enqueue(
        plain.map((file) => () => {
          sftp.pushOp(id, { kind: 'download', name: file.name, refresh: 'local' });
          void sftpDownload(id, joinLocal(dir, file.name), file.path).catch(onDispatchError(id));
        }),
        true
      )
    );
  }

  // --- Drag and drop -----------------------------------------------------------------
  // Pane to pane uses pointer events, not HTML5 drag and drop: Tauri's native file-drop
  // handler (on by default, and needed for drops from the OS below) swallows HTML5 drag
  // events in WebView2. A press becomes a drag only past DRAG_THRESHOLD px, so a click
  // still navigates or previews.
  const DRAG_THRESHOLD = 5;

  interface DropTarget {
    side: PaneSide;
    /** The directory the drop lands in: a folder row under the pointer, else the
     *  pane's current directory. */
    dir: string;
    /** Set when `dir` is a folder row, for its highlight. */
    row?: string;
  }

  let root = $state<HTMLElement>();
  let press: { side: PaneSide; entry: FileEntryDto; x: number; y: number } | null = null;
  let drag = $state<{
    from: PaneSide;
    files: FileEntryDto[];
    x: number;
    y: number;
    target: DropTarget | null;
  } | null>(null);
  // The press went past the threshold, carrying something or not (`..`): the release
  // must not click the row it ends on, even after Escape.
  let moved = false;
  let swallowClick = false;
  // A drag from the OS hovering this view, for the remote pane's highlight.
  let osDrop = $state<DropTarget | null>(null);

  function endGesture(): void {
    press = null;
    drag = null;
    moved = false;
  }

  // A hidden view lets go of what it held (§2): no ghost over the entity now shown, no
  // transfer from a release there, and no Replace prompt to come back with a stale count.
  $effect(() => {
    if (!active) {
      endGesture();
      osDrop = null;
      overwrite = null;
    }
  });

  /** This view's pane (and folder row, if any) under a viewport point. Below the panes,
   *  on the transfer strip, a point goes to the pane above it. */
  function dropTargetAt(x: number, y: number): DropTarget | null {
    if (!view || !root) return null;
    const el = document.elementFromPoint(x, y);
    if (!el || !root.contains(el)) return null;
    const paneEl =
      el.closest<HTMLElement>('[data-pane]') ??
      [...root.querySelectorAll<HTMLElement>('[data-pane]')].find((pane) => {
        const r = pane.getBoundingClientRect();
        return x >= r.left && x < r.right;
      });
    const side = paneEl?.dataset.pane;
    if (side !== 'local' && side !== 'remote') return null;
    const row = el.closest<HTMLElement>('[data-dir-path]')?.dataset.dirPath;
    return { side, dir: row ?? view[side].path, row };
  }

  function startPress(side: PaneSide, entry: FileEntryDto, e: PointerEvent): void {
    if (e.button !== 0 || !view || !active) return;
    press = { side, entry, x: e.clientX, y: e.clientY };
    moved = false;
  }

  function onPointerMove(e: PointerEvent): void {
    if (!press && !drag) return;
    // The button came up where no pointerup reached this window.
    if ((e.buttons & 1) === 0) {
      endGesture();
      return;
    }
    if (drag) {
      const target = dropTargetAt(e.clientX, e.clientY);
      drag = {
        ...drag,
        x: e.clientX,
        y: e.clientY,
        target: target && target.side !== drag.from ? target : null
      };
      return;
    }
    if (!press || !view) return;
    if (Math.hypot(e.clientX - press.x, e.clientY - press.y) < DRAG_THRESHOLD) return;
    moved = true;
    const files = dragPayload(view[press.side], press.entry);
    const from = press.side;
    press = null;
    if (files.length === 0) return;
    drag = { from, files, x: e.clientX, y: e.clientY, target: null };
  }

  function onPointerUp(): void {
    const done = drag;
    if (moved) {
      swallowClick = true;
      setTimeout(() => (swallowClick = false), 0);
    }
    endGesture();
    if (!active || !done?.target) return;
    if (done.from === 'local') upload(done.files, done.target.dir);
    else download(done.files, done.target.dir);
  }

  function onKeyDown(e: KeyboardEvent): void {
    if (e.key === 'Escape' && drag) drag = null;
  }

  // Drops from the OS file manager, with absolute paths and a position in the webview's
  // own pixels. Only the remote pane accepts them; every SFTP view stays mounted, so
  // only the visible one reacts.
  onMount(() => {
    let unlisten: (() => void) | undefined;
    let webview: ReturnType<typeof getCurrentWebview>;
    try {
      webview = getCurrentWebview();
    } catch {
      // Off the Tauri runtime there is no webview to drop onto; pane to pane still works.
      return;
    }
    // Read again as each drag comes in: the window may have moved to a screen of
    // another scale.
    let scale: number | null = null;
    const readScale = () =>
      void webview.window
        .scaleFactor()
        .then((s) => (scale = s))
        .catch(() => {});
    readScale();
    void webview
      .onDragDropEvent((event) => {
        if (!active || !view) return;
        const p = event.payload;
        if (p.type === 'leave') {
          osDrop = null;
          return;
        }
        if (p.type === 'enter') readScale();
        const at = dropPoint(p.position, scale);
        const target = dropTargetAt(at.x, at.y);
        const remote = target?.side === 'remote' ? target : null;
        if (p.type === 'drop') {
          osDrop = null;
          if (remote) uploadDropped(p.paths, remote.dir);
        } else {
          osDrop = remote;
        }
      })
      .then((fn) => {
        if (destroyed) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => unlisten?.();
  });

  // Dropped paths upload as they are: the core tells files from folders and walks the
  // folders.
  function uploadDropped(paths: string[], dir: string): void {
    const items = paths.map((path) => ({ path, name: baseName(path) }));
    if (items.some((item) => !item.name)) lastError.set('A drive or volume cannot be uploaded.');
    upload(items.filter((item) => item.name), dir);
  }

  function guarded<T>(fn: (arg: T) => void): (arg: T) => void {
    return (arg) => {
      if (!swallowClick) fn(arg);
    };
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
      enqueue([
        () => {
          sftp.pushOp(id, { kind: 'mkdir', refresh: 'remote' });
          void sftpMkdir(id, joinRemote(dir, value)).catch(onDispatchError(id));
        }
      ]);
    } else if (prompt.target) {
      const from = prompt.target.path;
      enqueue([
        () => {
          sftp.pushOp(id, { kind: 'rename', refresh: 'remote' });
          void sftpRename(id, from, joinRemote(dir, value)).catch(onDispatchError(id));
        }
      ]);
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

  // Large files use the configured external editor. For remote files the user can
  // disable the pre-download confirmation in Settings.
  if (shouldUseExternal(entry)) {
    largeFilePrompt = { entry, side };
    if (side === 'remote' && generalConfigStore.value?.promptLargeFiles === false) {
      try {
        await openExternalEditor();
      } catch (err) {
        lastError.set(errMsg(err));
      }
    }
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

async function openExternalEditor(): Promise<void> {
  if (!largeFilePrompt) return;

  const { entry, side } = largeFilePrompt;

  if (side === "remote" && backendId == null) return;

  const tempPath =
    side === "local"
      ? await localPrepareExternalEdit(entry.path)
      : await sftpPrepareExternalEdit(entry.path);

  largeFilePrompt = null;

  // Remote: queue the download and only mark it as the external-edit transfer when
  // the queue dispatches it. This prevents an earlier download from opening the editor.
  if (side === "remote") {
    const id = backendId;
    if (id == null) {
      await sftpRemoveTempFile(tempPath);
      return;
    }

    externalEdit = { side, path: entry.path, tempPath, queued: true };
    enqueue([
      () => {
        externalEdit = { side, path: entry.path, tempPath, queued: false };
        sftp.pushOp(id, {
          kind: "download",
          name: entry.path.split("/").pop() ?? entry.path,
          refresh: "local",
        });
        void sftpDownload(id, tempPath, entry.path).catch(onDispatchError(id));
      }
    ], true);
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

async function discardExternalEdit(): Promise<void> {
  const pending = uploadPrompt;
  uploadPrompt = null;
  externalEdit = null;
  if (pending) {
    try {
      await sftpRemoveTempFile(pending.tempPath);
    } catch (err) {
      lastError.set(errMsg(err));
    }
  }
}

async function uploadExternalEdit(): Promise<void> {
  if (!uploadPrompt) return;

  const { side, path, tempPath } = uploadPrompt;

  uploadPrompt = null;

  // Local file: write back directly
  if (side === 'local') {
    try {
      const text = await localReadFile(tempPath);
      await localWriteFile(path, text);
    } catch (err) {
      lastError.set(errMsg(err));
    } finally {
      try {
        await sftpRemoveTempFile(tempPath);
      } catch (err) {
        lastError.set(errMsg(err));
      }
      externalEdit = null;
    }
    return;
  }

  // Remote file requires an active session
  if (backendId == null) {
    await sftpRemoveTempFile(tempPath).catch((err) => lastError.set(errMsg(err)));
    return;
  }

  const id = backendId;
  pendingUploadTemp = tempPath;
  enqueue([
    () => {
      sftp.pushOp(id, {
        kind: 'upload',
        name: path.split('/').pop() ?? path,
        refresh: 'remote',
      });
      void sftpUpload(id, tempPath, path).catch(onDispatchError(id));
    }
  ], true);
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

<svelte:window
  onpointermove={onPointerMove}
  onpointerup={onPointerUp}
  onpointercancel={endGesture}
  onkeydown={onKeyDown}
/>

{#if active && drag}
  <!-- Follows the pointer; pointer-events-none so the pane under it stays hit-testable. -->
  <div
    class="pointer-events-none fixed z-50 flex items-center gap-1.5 rounded-full border border-default
      bg-surface px-3 py-1.5 text-xs font-medium text-fg shadow-soft
      {drag.target ? '' : 'opacity-70'}"
    style="left: {drag.x + 14}px; top: {drag.y + 14}px"
    aria-hidden="true"
  >
    <Icon name={drag.from === 'local' ? 'upload' : 'download'} size={13} />
    {drag.files.length === 1 ? drag.files[0].name : `${drag.files.length} items`}
  </div>
{/if}

<!-- bg-surface fills behind the macOS traffic lights (no seam); the pt insets the
     panes below them. The select-none while dragging keeps the drag from selecting text. -->
<div
  bind:this={root}
  class="absolute inset-0 flex flex-col bg-surface pt-[var(--titlebar-h)]
    {active ? '' : 'hidden'} {drag ? 'cursor-grabbing select-none' : ''}"
>
  {#if openError}
    <div class="flex flex-1 flex-col items-center justify-center gap-2 p-10 text-center">
      <p class="font-medium">Could not open SFTP on {session.hostName}</p>
      <p class="max-w-md text-sm text-muted">{$maskText(openError)}</p>
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
          side="local"
          pane={view.local}
          dropActive={drag?.target?.side === 'local'}
          dropDir={drag?.target?.side === 'local' ? drag.target.row : undefined}
          onNavigate={guarded((e) => navigate('local', e))}
          onToggleMark={(p) => toggleMark('local', p)}
          onPreview={guarded((e) => preview('local', e))}
          onDragStart={(entry, e) => startPress('local', entry, e)}
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
              onclick={() => upload(localMarked)}
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
          side="remote"
          pane={view.remote}
          dropActive={drag?.target?.side === 'remote' || osDrop != null}
          dropDir={drag?.target?.side === 'remote' ? drag.target.row : osDrop?.row}
          onNavigate={guarded((e) => navigate('remote', e))}
          onToggleMark={(p) => toggleMark('remote', p)}
          onPreview={guarded((e) => preview('remote', e))}
          onDragStart={(entry, e) => startPress('remote', entry, e)}
        >
          {#snippet toolbar()}
            <button
              type="button"
              class={toolBtn}
              title="Download marked files"
              disabled={remoteMarked.length === 0}
              onclick={() => download(remoteMarked)}
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

      {#if transfer}
        <div class="shrink-0 border-t border-default px-4 py-2.5" aria-label="transfer progress">
          <div class="flex items-start justify-between gap-3">
            <div class="min-w-0 flex-1">
              {#if transfer.preparing}
                <div class="truncate text-xs text-muted">
                  {transfer.kind === 'delete' ? 'Preparing delete…' : `Preparing ${transfer.kind}…`}
                </div>

                <div class="truncate font-mono text-sm text-fg">
                  {transfer.name}
                </div>

                {#if transfer.currentFile}
                  <div class="truncate text-xs text-muted">
                    {transfer.currentFile}
                  </div>
                {/if}
              {:else}
                <div class="truncate text-xs text-muted">
                  {transfer.kind === 'upload'
                    ? 'Uploading'
                    : transfer.kind === 'download'
                      ? 'Downloading'
                      : 'Deleting'}
                  <span class="ml-1 font-mono text-fg">{transfer.rootName ?? transfer.name}</span>
                </div>

                {#if transfer.currentFile}
                  <div class="truncate font-mono text-sm text-fg">
                    {transfer.currentFile}
                  </div>
                {/if}
              {/if}
            </div>

            <div class="shrink-0 text-right">
              {#if transfer.preparing}
                <div class="text-xs text-muted">Scanning…</div>
              {:else if transfer.kind === 'delete'}
                <div class="text-xs tabular-nums text-muted">
                  {transfer.filesDone} / {transfer.filesTotal} items
                </div>
              {:else}
                {#if transfer.filesTotal > 0}
                  <div class="text-xs tabular-nums text-muted">
                    {transfer.filesDone} / {transfer.filesTotal} files
                  </div>
                {/if}

                <div class="text-xs tabular-nums text-muted">
                  {formatBytes(transfer.bytesDone)} / {formatBytes(transfer.bytesTotal)}
                </div>
              {/if}

              {#if transfer.transferId != null}
                <button
                  class="mt-2 w-full rounded border border-red-500 px-2 py-1 text-xs text-red-500 hover:bg-red-500/10"
                  onclick={() => {
                    const id = backendId;
                    if (id == null || transfer.transferId == null) return;
                    void sftpCancelTransfer(id, transfer.transferId).catch((err) =>
                      lastError.set(errMsg(err))
                    );
                  }}
                >
                  Cancel this transfer
                </button>
              {/if}
              <button
                class="mt-1 w-full rounded border border-default px-2 py-1 text-xs text-muted hover:bg-surface-inset"
                onclick={cancelTransfers}
              >
                Cancel all queued transfers
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
      <!-- A cancel the user asked for is news, not a failure. -->
      <div
        class="shrink-0 border-t border-default px-4 py-2 text-xs
          {view.error === CANCELLED ? 'text-muted' : 'text-status-crit'}"
      >
        {$maskText(view.error)}
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

{#if active && overwrite}
  <Modal label="Replace existing items" onClose={() => (overwrite = null)}>
    <div class="space-y-3 px-5 py-4">
      <h2 class="text-sm font-semibold">Replace existing items</h2>
      <p class="text-sm text-muted">
        {overwrite.count} item(s) already exist in <span class="font-mono">{overwrite.dir}</span>.
        Folders are merged and files with the same name are replaced.
      </p>
      <div class="flex justify-end gap-2 pt-1">
        <Button variant="ghost" onclick={() => (overwrite = null)}>Cancel</Button>
        <Button variant="primary" onclick={replace}>Replace</Button>
      </div>
    </div>
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
      <button
        type="button"
        class="rounded-full bg-accent px-5 py-2 text-sm font-medium text-accent-fg"
        onclick={() => void openExternalEditor().catch((err) => lastError.set(errMsg(err)))}
      >Open External</button>
    </footer>
  </Modal>
{/if}

{#if active && uploadPrompt}
  <Modal label="Save changes?" onClose={discardExternalEdit}>
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
        onclick={discardExternalEdit}
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