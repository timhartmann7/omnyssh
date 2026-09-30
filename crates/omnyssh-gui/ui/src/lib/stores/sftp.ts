import { writable } from "svelte/store";
import type { FileEntryDto, TransferProgressDto } from "$lib/bindings";

// Per-session SFTP state for the dual-pane browser (tech-gui.md §3.2, §3.5), keyed by
// the backend public session id — the same id the `sftp-*` events carry, so the router
// routes each event to the right tab. Navigation/marking/transfer logic lives here as
// pure reducers so it is unit-testable; `SftpView.svelte` is a thin view + dispatcher.

export type PaneSide = "local" | "remote";
type SftpStatus = "connecting" | "connected" | "failed";
type OpKind = "upload" | "download" | "mkdir" | "rename" | "delete";

/** One side's browsing state: current directory, its entries, and the marked set. */
export interface Pane {
  path: string;
  entries: FileEntryDto[];
  loading: boolean;
  /** Marked entry paths — the batch transfer/delete targets. */
  marked: Set<string>;
  error?: string;
}

/** The transfer currently reporting progress (one at a time — the core is sequential). */
interface Transfer {
  transferId: number;
  stage: "preparing" | "transferring";
  kind: "upload" | "download";
  rootName: string;
  currentFile: string;
  bytesDone: number;
  bytesTotal: number;
  filesDone: number;
  filesTotal: number;
}

/** A file preview (a remote `file-preview` event, or a local read) shown in a modal. */
interface Preview {
  path: string;
  content: string;
}

// A mutating op awaiting its `sftp-op-done`. The core processes commands sequentially,
// so op-done events arrive in issue order — this FIFO correlates each op-done to the op
// that produced it (the contract carries no op id, §4.3). `refresh` is the pane whose
// listing the op invalidates.
interface PendingOp {
  kind: OpKind;
  name?: string;
  refresh: PaneSide;
}

export interface SftpSession {
  hostName: string;
  status: SftpStatus;
  local: Pane;
  remote: Pane;
  pending: PendingOp[];
  transfer?: Transfer;
  preview?: Preview;
  /** The last operation error, surfaced in the UI until the next successful action. */
  error?: string;
  /** Pane(s) to re-list once `pending` drains (a mutation changed the FS); the
   *  component performs the listing and clears this. */
  refresh?: PaneSide | "both";
}

function emptyPane(): Pane {
  return { path: "", entries: [], loading: true, marked: new Set() };
}

/** A fresh session in the connecting state, both panes empty. */
export function newSession(hostName: string): SftpSession {
  return {
    hostName,
    status: "connecting",
    local: emptyPane(),
    remote: emptyPane(),
    pending: [],
  };
}

/** A directory listing landed for a pane: replace entries at `path`, clear marks. */
export function applyListing(
  pane: Pane,
  path: string,
  entries: FileEntryDto[],
): Pane {
  return {
    ...pane,
    path,
    entries,
    loading: false,
    marked: new Set(),
    error: undefined,
  };
}

/** Toggle an entry's marked state (the batch transfer/delete set). */
export function toggleMark(pane: Pane, path: string): Pane {
  const marked = new Set(pane.marked);
  if (marked.has(path)) marked.delete(path);
  else marked.add(path);
  return { ...pane, marked };
}

/** The marked entries in listing order — the stable sequence a batch transfer follows. */
export function markedEntries(pane: Pane): FileEntryDto[] {
  return pane.entries.filter((e) => pane.marked.has(e.path));
}

/** The root in `roots` that `path` lies under, or '' for none (a network share).
 *  Drive letters compare case-insensitively, as Windows does. */
export function rootOf(path: string, roots: string[]): string {
  const lower = path.toLowerCase();
  return roots.find((root) => lower.startsWith(root.toLowerCase())) ?? "";
}

/** Human-readable byte size for a listing row or a transfer bar. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}

/** Widen the pending refresh target: two different sides collapse to `both`. */
export function mergeRefresh(
  current: PaneSide | "both" | undefined,
  next: PaneSide,
): PaneSide | "both" {
  if (!current || current === next) return next;
  return "both";
}

/** Fold a `transfer-progress` tick in. The transfer's kind/name come from the front
 *  pending op — which, because the core is sequential, is always the op now running —
 *  so the frontend never needs to know the backend-allocated transfer id in advance. */
export function applyProgress(
  session: SftpSession,
  p: TransferProgressDto,
): SftpSession {
  const front = session.pending[0];

  if (!front || (front.kind !== "upload" && front.kind !== "download")) {
    return session;
  }

  return {
    ...session,
    transfer: {
      transferId: p.transferId,
      stage: p.stage as "preparing" | "transferring",
      kind: front.kind,
      rootName: p.rootName,
      currentFile: p.currentFile,
      bytesDone: p.bytesDone,
      bytesTotal: p.bytesTotal,
      filesDone: p.filesDone,
      filesTotal: p.filesTotal,
    },
  };
}

/** Fold an `sftp-op-done` in: pop the front pending op (FIFO), record its refresh
 *  target, clear the transfer display if it was a transfer, and surface any error. */
export function applyOpDone(
  session: SftpSession,
  ok: boolean,
  error?: string,
): SftpSession {
  if (session.pending.length === 0) return session;
  const [front, ...rest] = session.pending;
  const wasTransfer = front.kind === "upload" || front.kind === "download";
  return {
    ...session,
    pending: rest,
    refresh: mergeRefresh(session.refresh, front.refresh),
    // A later op's success must NOT wipe an earlier op's failure in the same batch — that
    // silently masks e.g. a non-empty-folder delete beside a deleted sibling. The error
    // persists until the next batch clears it (`clearError`, called on enqueue).
    error: ok ? session.error : (error ?? "Operation failed"),
    transfer: wasTransfer ? undefined : session.transfer,
  };
}

function createSftp() {
  const { subscribe, update } = writable<Map<number, SftpSession>>(new Map());

  /** Apply `fn` to one session, no-op if the id is unknown (a closed tab). */
  function mut(id: number, fn: (s: SftpSession) => SftpSession): void {
    update((m) => {
      const session = m.get(id);
      if (!session) return m;
      const next = new Map(m);
      next.set(id, fn(session));
      return next;
    });
  }

  return {
    subscribe,
    /** Register a freshly opened session (called once `sftp_open` resolves). */
    open(id: number, hostName: string): void {
      update((m) => new Map(m).set(id, newSession(hostName)));
    },
    setStatus(id: number, status: SftpStatus): void {
      mut(id, (s) => ({ ...s, status }));
    },
    /** Mark a pane as loading before a listing request goes out. */
    beginLoading(id: number, side: PaneSide): void {
      mut(id, (s) => ({ ...s, [side]: { ...s[side], loading: true } }));
    },
    listing(
      id: number,
      side: PaneSide,
      path: string,
      entries: FileEntryDto[],
    ): void {
      mut(id, (s) => ({ ...s, [side]: applyListing(s[side], path, entries) }));
    },
    paneError(id: number, side: PaneSide, error: string): void {
      mut(id, (s) => ({ ...s, [side]: { ...s[side], loading: false, error } }));
    },
    toggleMark(id: number, side: PaneSide, path: string): void {
      mut(id, (s) => ({ ...s, [side]: toggleMark(s[side], path) }));
    },
    pushOp(id: number, op: PendingOp): void {
      mut(id, (s) => ({ ...s, pending: [...s.pending, op] }));
    },
    progress(id: number, p: TransferProgressDto): void {
      mut(id, (s) => applyProgress(s, p));
    },
    opDone(id: number, ok: boolean, error?: string): void {
      mut(id, (s) => applyOpDone(s, ok, error));
    },
    setPreview(id: number, preview: Preview): void {
      mut(id, (s) => ({ ...s, preview }));
    },
    clearPreview(id: number): void {
      mut(id, (s) => ({ ...s, preview: undefined }));
    },
    clearRefresh(id: number): void {
      mut(id, (s) => ({ ...s, refresh: undefined }));
    },
    /** Drop the surfaced op error — called when a new batch is enqueued, so a fresh
     *  action starts clean while a finished batch's error still lingered until now. */
    clearError(id: number): void {
      mut(id, (s) => ({ ...s, error: undefined }));
    },
    /** A soft error (e.g. a failed listing reported as `sftp-disconnected`, §4.3). The
     *  core emits it only for a remote `ListDir`, so clear just the remote pane's loading
     *  (leaving it set strands it on "Loading…"); a legit in-flight local listing keeps
     *  its own spinner. */
    sessionError(id: number, error: string): void {
      mut(id, (s) => ({
        ...s,
        error,
        remote: { ...s.remote, loading: false },
      }));
    },
    remove(id: number): void {
      update((m) => {
        if (!m.has(id)) return m;
        const next = new Map(m);
        next.delete(id);
        return next;
      });
    },
  };
}

export const sftp = createSftp();
