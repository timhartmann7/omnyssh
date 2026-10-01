// Pure event->store routing (tech-gui.md §3.5). Kept free of the Tauri runtime
// so it is unit-testable; `subscribe.ts` wires these to the generated events.

import { get } from 'svelte/store';
import type {
  ConnectionStatusDto,
  FilePreview,
  HostDto,
  KeyPassphraseRequired,
  KeySetupComplete,
  KeySetupFailed,
  KeySetupProgress,
  KeySetupRollback,
  MetricsDto,
  PasswordRequired,
  ServiceDto,
  SftpConnected,
  SftpDirListed,
  SftpDisconnected,
  SftpOpDone,
  SnippetResult,
  TransferProgressDto,
  TunnelStatusChanged
} from '$lib/bindings';
import { hosts } from '$lib/stores/hosts';
import { statuses } from '$lib/stores/statuses';
import { metrics, mergeMetrics } from '$lib/stores/metrics';
import { services } from '$lib/stores/services';
import { tunnels } from '$lib/stores/tunnels';
import { snippetRun, reduceRunResult } from '$lib/stores/snippets';
import { sessions } from '$lib/stores/sessions';
import { sftp } from '$lib/stores/sftp';
import { closeSession } from '$lib/stores/navigation';
import { lastError } from '$lib/stores/notifications';
import {
  keySetup,
  reduceComplete,
  reduceFailed,
  reduceProgress,
  reduceRollback
} from '$lib/stores/keySetup';
import { enqueuePassphrase, passphraseQueue } from '$lib/stores/passphrase';
import { passwordQueue } from '$lib/stores/password';
import { offerUpdate } from '$lib/stores/update';
import type { UpdateAvailable } from '$lib/bindings';

export function applyHostsLoaded(payload: HostDto[]): void {
  hosts.set(payload);
  // Drop status/metrics for hosts that are gone, so a name reused by a new host
  // never inherits the old host's stale sample.
  const names = new Set(payload.map((h) => h.name));
  const prune = <V>(m: Map<string, V>) => new Map([...m].filter(([name]) => names.has(name)));
  statuses.update(prune);
  metrics.update(prune);
  services.update(prune);
  tunnels.update(prune);
}

export function applyHostStatusChanged(payload: {
  hostName: string;
  status: ConnectionStatusDto;
}): void {
  statuses.update((m) => new Map(m).set(payload.hostName, payload.status));
}

export function applyMetricsUpdated(payload: { hostName: string; metrics: MetricsDto }): void {
  metrics.update((m) => new Map(m).set(payload.hostName, mergeMetrics(m.get(payload.hostName), payload.metrics)));
}

export function applyServicesDetected(payload: { hostName: string; services: ServiceDto[] }): void {
  services.update((m) =>
    new Map(m).set(payload.hostName, { kind: 'detected', services: payload.services })
  );
}

export function applyServicesFailed(payload: { hostName: string; message: string }): void {
  services.update((m) => new Map(m).set(payload.hostName, { kind: 'failed', message: payload.message }));
}

// A stopped tunnel leaves no entry, so a host that is renamed or deleted while its
// tunnel winds down does not keep a stale one.
export function applyTunnelStatusChanged(payload: TunnelStatusChanged): void {
  tunnels.update((m) => {
    const next = new Map(m);
    if (payload.status.kind === 'stopped') next.delete(payload.hostName);
    else next.set(payload.hostName, payload.status);
    return next;
  });
}

export function applySnippetResult(payload: SnippetResult): void {
  snippetRun.update((run) => reduceRunResult(run, payload));
}

// A terminal's remote shell exited or its connection dropped (tech-gui.md §3.4). The
// backend already tore down its session. A tab that showed output stays, marked
// closed, so whatever the server said last can still be read; one that never did (a
// failed connect, whose reason is in the status bar) is dropped. A user-initiated
// close never emits this, so there is no double-teardown.
//
// An instant-fail connect can emit terminal-exited before terminalOpen resolves, so
// the tab has no termId yet: park the id and let the tab reconcile once it records
// its backend id (`terminalDidExit`), rather than stranding a dead tab open.
const exitedBeforeMapped = new Map<number, boolean>();

export function applyTerminalExited(sessionId: number, hadOutput: boolean): void {
  const target = get(sessions).find((s) => s.termId === sessionId);
  if (!target) exitedBeforeMapped.set(sessionId, hadOutput);
  else if (hadOutput) sessions.setStatus(target.id, 'closed');
  else closeSession(target.id);
}

/** Whether backend session `termId` exited before its tab recorded it (the fast-fail
 *  race): its `hadOutput`, or `undefined` if it did not. Consumes the pending entry;
 *  called right after a tab sets termId. */
export function terminalDidExit(termId: number): boolean | undefined {
  const hadOutput = exitedBeforeMapped.get(termId);
  exitedBeforeMapped.delete(termId);
  return hadOutput;
}

// SFTP events (tech-gui.md §3.4/§4.3). Each carries the backend session id the sftp
// store is keyed by, so the per-session forwarder's stamping routes it to the right
// tab with no path-based guessing. `SftpView` mirrors the store status to the sidebar.
export function applySftpConnected(payload: SftpConnected): void {
  sftp.setStatus(payload.sessionId, 'connected');
}

export function applySftpDirListed(payload: SftpDirListed): void {
  sftp.listing(payload.sessionId, 'remote', payload.path, payload.entries);
}

export function applySftpOpDone(payload: SftpOpDone): void {
  sftp.opDone(payload.sessionId, payload.ok, payload.error ?? undefined);
}

// The core emits `sftp-disconnected` on a listing error, not a hard teardown (§4.3);
// surface the reason but keep the tab — the user closes it explicitly.
export function applySftpDisconnected(payload: SftpDisconnected): void {
  sftp.sessionError(payload.sessionId, payload.reason);
}

export function applyFilePreview(payload: FilePreview): void {
  sftp.setPreview(payload.sessionId, { path: payload.path, content: payload.content });
}

export function applyTransferProgress(payload: TransferProgressDto): void {
  sftp.progress(payload.sessionId, payload);
}

// Auto key-setup events (tech-gui.md §4.2/§4.3). Progress advances only the active
// host's run; a terminal outcome always shows for its host. The card refresh on
// completion is driven by the progress panel component (an ipc call), so this stays a
// pure store update.
export function applyKeySetupProgress(payload: KeySetupProgress): void {
  keySetup.update((run) => reduceProgress(run, payload.hostName, payload.step));
}

export function applyKeySetupComplete(payload: KeySetupComplete): void {
  keySetup.set(reduceComplete(payload.hostName, payload.keyPath));
}

export function applyKeySetupFailed(payload: KeySetupFailed): void {
  keySetup.set(reduceFailed(payload.hostName, payload.error));
}

export function applyKeySetupRollback(payload: KeySetupRollback): void {
  keySetup.set(reduceRollback(payload.hostName, payload.result));
}

// A newer release found by the startup check (tech-gui.md §4.3) → the update banner.
export function applyUpdateAvailable(payload: UpdateAvailable): void {
  offerUpdate(payload.info);
}

export function applyError(message: string): void {
  lastError.set(message);
}

// An encrypted key the core could not use (tech-gui.md §4.3). Queued per key, so
// the hosts sharing it wait on one dialog.
export function applyKeyPassphraseRequired(payload: KeyPassphraseRequired): void {
  passphraseQueue.update((queue) => enqueuePassphrase(queue, payload));
}

// A login waiting for its password (tech-gui.md §4.3); answered by `answer_password`.
export function applyPasswordRequired(payload: PasswordRequired): void {
  passwordQueue.update((queue) => [...queue, payload]);
}
